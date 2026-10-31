import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

/// Where `ratchetd` listens and where it leaves the token of this boot.
public struct DaemonLocation: Sendable, Equatable {
    public var socketPath: String
    public var tokenPath: String

    public init(socketPath: String, tokenPath: String) {
        self.socketPath = socketPath
        self.tokenPath = tokenPath
    }

    /// `~/Library/Application Support/Ratchet/ratchetd.sock` and `ratchetd.token` next to it, as the daemon writes them.
    /// `RATCHET_HOME` moves the directory and `RATCHET_SOCKET` names the socket, as for the daemon itself.
    public static func standard(environment: [String: String] = ProcessInfo.processInfo.environment) -> DaemonLocation {
        let directory: URL
        if let home = environment["RATCHET_HOME"], !home.isEmpty {
            directory = URL(fileURLWithPath: home, isDirectory: true)
        } else {
            directory = FileManager.default.homeDirectoryForCurrentUser
                .appending(path: "Library/Application Support/Ratchet", directoryHint: .isDirectory)
        }
        if let socket = environment["RATCHET_SOCKET"], !socket.isEmpty {
            let parent = URL(fileURLWithPath: socket).deletingLastPathComponent()
            return DaemonLocation(socketPath: socket, tokenPath: parent.appending(path: "ratchetd.token").path)
        }
        return DaemonLocation(
            socketPath: directory.appending(path: "ratchetd.sock").path,
            tokenPath: directory.appending(path: "ratchetd.token").path)
    }

    /// True when a daemon accepts connections at the socket right now.
    public var isReachable: Bool { UnixSocket.canConnect(path: socketPath) }

    /// The token of this boot, read from its file each time (the daemon writes a new one at every start).
    public func readToken() -> String? {
        guard let text = try? String(contentsOfFile: tokenPath, encoding: .utf8) else { return nil }
        let token = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return token.isEmpty ? nil : token
    }
}

/// `RatchetClient` on the real daemon: the generated OpenAPI client over the unix socket, the daemon's view models
/// mapped to the app's models, the event stream turned into `RatchetEvent`s.
///
/// Every group of the API is real: the overview, connectors and environment drafts included. There is no sample-data
/// fallback; a daemon that cannot answer is an error the app shows (and retries).
public final class DaemonRatchetClient: RatchetClient {
    private let api: Client
    private let state = DaemonClientState()

    public init(transport: any ClientTransport) {
        self.api = Client(
            serverURL: URL(string: "http://localhost") ?? URL(fileURLWithPath: "/"),
            transport: transport, middlewares: [APIErrorMiddleware()])
    }

    public convenience init(location: DaemonLocation) {
        self.init(transport: UnixSocketTransport(socketPath: location.socketPath, token: { location.readToken() }))
    }

    // MARK: Failures

    /// What the app sees of a failed call: the protocol's own errors, in plain words.
    private func translate(_ error: any Error) -> any Error {
        if let error = daemonError(in: error) { return error.clientError }
        if let socket = underlyingSocketError(in: error) {
            return RatchetClientError.unavailable(socket.errorDescription ?? "Cannot reach ratchetd.")
        }
        if let wrapped = error as? ClientError { return RatchetClientError.unavailable(wrapped.causeDescription) }
        return error
    }

    private func underlyingSocketError(in error: any Error) -> UnixSocketError? {
        if let error = error as? UnixSocketError { return error }
        if let wrapped = error as? ClientError { return underlyingSocketError(in: wrapped.underlyingError) }
        return nil
    }

    /// Runs `real` and translates its failure.
    private func calling<T: Sendable>(_ real: () async throws -> T) async throws -> T {
        do {
            return try await real()
        } catch {
            throw translate(error)
        }
    }

    // MARK: Fetching

    private func optimizationDetail(_ id: String) async throws -> VM.OptimizationDetail {
        let detail = try await api.getOptimization(path: .init(id: id)).ok.body.json
        await state.remember(agents: detail.agents.map(\.id), of: id)
        return detail
    }

    private func optimizationSnapshot() async throws -> [VM.OptimizationVm] {
        try await api.listOptimizations().ok.body.json.optimizations
    }

    /// One row of the table: the list entry, the app's model of it, and the detail it was enriched from.
    struct OptimizationRow: Sendable {
        var vm: VM.OptimizationVm
        var optimization: Optimization
        var detail: VM.OptimizationDetail?
    }

    /// Every optimization with its detail. A refresh asks for the table and the benchmarks within moments of each other;
    /// they share one load.
    private func optimizationRows() async throws -> [OptimizationRow] {
        let task = await state.rowsTask { Task { try await self.loadOptimizationRows() } }
        do {
            return try await task.value
        } catch {
            await state.forgetRows()
            throw error
        }
    }

    /// The list, then every detail side by side. The table needs the clock, the pull request and the noise, which only the
    /// detail has; one that fails to load shows with what the list knows.
    private func loadOptimizationRows() async throws -> [OptimizationRow] {
        let list = try await optimizationSnapshot()
        let details = await withTaskGroup(of: (Int, VM.OptimizationDetail?).self) { group in
            for (index, vm) in list.enumerated() {
                group.addTask { (index, try? await self.optimizationDetail(vm.id)) }
            }
            var found: [Int: VM.OptimizationDetail] = [:]
            for await (index, detail) in group {
                if let detail { found[index] = detail }
            }
            return found
        }
        return list.enumerated().map { index, vm in
            OptimizationRow(vm: vm, optimization: DaemonMapping.optimization(vm, detail: details[index]), detail: details[index])
        }
    }

    private func fetchOptimization(_ id: String) async throws -> Optimization {
        let detail = try await optimizationDetail(id)
        return DaemonMapping.optimization(detail.summary, detail: detail)
    }

    private func environments() async throws -> VM.EnvironmentsVm {
        try await api.listEnvironments().ok.body.json
    }

    // MARK: Overview and spend

    public func overview(range: TimeRange) async throws -> Overview {
        try await calling {
            DaemonMapping.overview(try await api.getOverview(query: .init(range: DaemonMapping.range(range))).ok.body.json)
        }
    }

    public func spend(range: TimeRange) async throws -> SpendSummary {
        try await calling {
            let list = try await optimizationSnapshot()
            let lines = list.filter { $0.spendUsd > 0 }.map { SpendLine(id: $0.id, label: $0.name, usd: $0.spendUsd) }
            return SpendSummary(
                range: range, totalUSD: list.reduce(0) { $0 + $1.spendUsd },
                budgetUSD: list.reduce(0) { $0 + $1.budgetUsd }, lines: lines)
        }
    }

    // MARK: Optimizations

    public func optimizations() async throws -> [Optimization] {
        try await calling { try await optimizationRows().map(\.optimization) }
    }

    public func optimization(id: String) async throws -> OptimizationDetail {
        try await calling {
            let detail = try await optimizationDetail(id)
            let optimization = DaemonMapping.optimization(detail.summary, detail: detail)
            async let candidates = candidates(optimizationID: id)
            async let moves = ratchetMoves(optimizationID: id, start: optimization.startedAt)
            async let change = winningChange(optimizationID: id)
            let winning = await change
            return OptimizationDetail(
                optimization: optimization, agents: detail.agents.map(DaemonMapping.agent),
                candidates: try await candidates, moves: (try? await moves) ?? [],
                receipt: detail.receipt.map { DaemonMapping.receipt($0, startedAt: optimization.startedAt) },
                filesChanged: winning.files, additions: winning.additions, deletions: winning.deletions)
        }
    }

    private func winningChange(optimizationID: String) async -> (files: Int, additions: Int, deletions: Int) {
        guard let change = try? await api.getWinningChange(path: .init(id: optimizationID)).ok.body.json else {
            return (0, 0, 0)
        }
        return (change.files.count, Int(change.additions), Int(change.deletions))
    }

    public func candidates(optimizationID: String) async throws -> [Candidate] {
        try await calling {
            let list = try await api.listCandidates(path: .init(id: optimizationID)).ok.body.json
            return list.map(DaemonMapping.candidate).sorted { $0.at > $1.at }
        }
    }

    public func ratchetHistory(optimizationID: String) async throws -> [RatchetMove] {
        try await calling {
            let start = try await fetchOptimization(optimizationID).startedAt
            return try await ratchetMoves(optimizationID: optimizationID, start: start)
        }
    }

    private func ratchetMoves(optimizationID: String, start: Date?) async throws -> [RatchetMove] {
        let series = try await api.getRatchetHistory(path: .init(id: optimizationID)).ok.body.json
        return DaemonMapping.moves(series, optimizationID: optimizationID, start: start ?? Date())
    }

    public func startOptimization(_ request: StartRequest) async throws -> Optimization {
        try await calling {
            let demo = request.templateID.flatMap { $0.hasPrefix("demo-") ? VM.DemoId(rawValue: $0) : nil }
            var kinds: [VM.AgentKind] = []
            if let list = try? await api.listConnectors().ok.body.json.connectors {
                for id in request.selection.agents.keys.sorted() {
                    if case .case3(let agent)? = list.first(where: { DaemonMapping.connectorID($0) == id }) {
                        let kind: VM.AgentKind = agent.agent == .codex ? .codex : .claudeCode
                        if !kinds.contains(kind) { kinds.append(kind) }
                    }
                }
            }
            let connections = await repoConnections(request.repoID, request.selection, demo: demo != nil)
            let goal = request.goal.trimmingCharacters(in: .whitespacesAndNewlines)
            let draftID = request.draftID
            let placements = Set(request.selection.agents.values)
            let input = VM.CreateOptimizationInput(
                agents: kinds.isEmpty ? [.claudeCode] : kinds,
                agentsWhere: placements.count == 1 ? placements.first.map(DaemonMapping.agentsWhere) : nil,
                baseRef: request.branch.isEmpty ? nil : request.branch,
                budgetUsd: request.budgetUSD,
                connections: connections,
                demo: demo,
                environmentDraftId: draftID,
                name: goal.isEmpty ? nil : String(goal.prefix(80)),
                plateauEvals: 30,
                repoUrl: demo == nil ? request.repoID : nil,
                timeLimitSeconds: Int64(request.timeLimitHours * 3600))
            let created = try await api.createOptimization(body: .json(input)).created.body.json
            _ = try await api.startOptimization(path: .init(id: created.id)).noContent
            await state.forgetRows()
            var started = try await fetchOptimization(created.id)
            started.goal = goal
            return started
        }
    }

    /// The connectors a run on `repo` uses, when the sheet chose some: the GitHub account is the one chosen or the repo's default.
    private func repoConnections(_ repo: String, _ selection: ConnectionSelection, demo: Bool) async -> VM.RepoConnections? {
        guard !demo, !selection.agents.isEmpty, let aws = selection.awsID else { return nil }
        var github = selection.githubID
        if github == nil {
            github = (try? await api.getConnectorSelection(query: .init(repo: repo)).ok.body.json)?.effective.github
        }
        guard let github else { return nil }
        return DaemonMapping.repoConnections(selection, github: github, aws: aws)
    }

    // MARK: Agents

    public func agents(optimizationID: String) async throws -> [Agent] {
        try await calling { try await optimizationDetail(optimizationID).agents.map(DaemonMapping.agent) }
    }

    public func pauseAgent(id: String) async throws -> Agent {
        try await calling {
            _ = try await api.pauseAgent(path: .init(id: id)).noContent
            return try await refreshedAgent(id)
        }
    }

    public func resumeAgent(id: String) async throws -> Agent {
        try await calling {
            _ = try await api.resumeAgent(path: .init(id: id)).noContent
            return try await refreshedAgent(id)
        }
    }

    public func messageAgent(id: String, text: String) async throws {
        try await calling {
            _ = try await api.messageAgent(path: .init(id: id), body: .json(.init(text: text))).noContent
        }
    }

    /// The agent as the daemon reports it now. The daemon has no "agent by id" route, so it is found through the
    /// optimization it belongs to (remembered from the details loaded so far, else looked for among the live ones).
    private func refreshedAgent(_ id: String) async throws -> Agent {
        var owner = await state.owner(ofAgent: id)
        if owner == nil {
            for vm in try await optimizationSnapshot() where vm.status == .running || vm.status == .baselining {
                _ = try await optimizationDetail(vm.id)
            }
            owner = await state.owner(ofAgent: id)
        }
        guard let owner else { throw RatchetClientError.notFound("agent \(id)") }
        let detail = try await optimizationDetail(owner)
        guard let agent = detail.agents.first(where: { $0.id == id }) else { throw RatchetClientError.notFound("agent \(id)") }
        return DaemonMapping.agent(agent)
    }

    // MARK: Benchmarks and templates

    public func benchmarks() async throws -> [BenchmarkRun] {
        try await calling {
            try await optimizationRows().compactMap { row in
                guard row.vm.baseline != nil else { return nil }
                let optimization = row.optimization
                let title: String
                switch row.vm.evaluatorKind {
                case .benchmark: title = "Benchmark"
                case .backtest: title = "Backtest"
                case .eval: title = "Evaluation"
                case .simulation: title = "Simulation"
                }
                return BenchmarkRun(
                    id: "bench-" + row.vm.id, title: title, optimizationTitle: optimization.title,
                    templateID: row.vm.pack ?? "custom", metric: optimization.metric, baseline: optimization.baseline,
                    best: optimization.best, noise: optimization.noise ?? 0, evals: optimization.evalsCount,
                    date: DaemonMapping.date(ms: row.vm.createdAt))
            }
        }
    }

    public func templates() async throws -> [Template] {
        try await calling { try await api.listTemplates().ok.body.json.map(DaemonMapping.template) }
    }

    // MARK: Repos

    public func repos() async throws -> [Repo] {
        do {
            return try await api.listRepos().ok.body.json.map(DaemonMapping.repo)
        } catch {
            // 409: not signed in to GitHub. The repo picker is simply empty until the user signs in.
            if let failure = daemonError(in: error), failure.status == 409 { return [] }
            throw translate(error)
        }
    }

    public func detectHarness(repoID: String) async throws -> HarnessDetection {
        do {
            let input = VM.InspectRepoInput(source: .case1(.init(kind: .github, repo: repoID)))
            let inspected = try await api.inspectRepo(body: .json(input)).ok.body.json
            return DaemonMapping.harness(repoID: repoID, inspected)
        } catch {
            if let failure = daemonError(in: error), !failure.isUnsupported {
                return HarnessDetection(repoID: repoID, found: false, summary: failure.message)
            }
            throw translate(error)
        }
    }

    // MARK: Connectors

    public func connectors() async throws -> [Connector] {
        try await calling {
            try await api.listConnectors().ok.body.json.connectors.map(DaemonMapping.connector)
        }
    }

    /// GitHub starts the device flow (the account appears in the list when it finishes); an AWS profile is connected by its
    /// name; an agent is a CLI login that was removed earlier. The UI for pasting keys and tokens comes later.
    public func addConnector(_ new: NewConnector) async throws -> Connector {
        try await calling {
            let list: VM.ConnectorsVm
            switch new.kind {
            case .github:
                // GitHub signs in with the device flow: the code is in `githubSignInFlow()`, the account appears when it ends.
                list = try await api.startGithubConnectorSignIn(body: .json(.init())).ok.body.json
                guard let existing = list.connectors.first(where: { DaemonMapping.connector($0).kind == .github }) else {
                    return Connector(id: "github-pending", kind: .github, name: new.name, detail: "Signing in with GitHub", state: .signedOut)
                }
                return DaemonMapping.connector(existing)
            case .aws:
                list = try await api.addAwsConnector(body: .json(.init(profile: new.name))).ok.body.json
            case .agent:
                let agent: VM.ConnectorAgent = new.name.lowercased().contains("codex") ? .codex : .claudeCode
                if let key = new.secret?.trimmingCharacters(in: .whitespacesAndNewlines), !key.isEmpty {
                    // The key goes to the daemon once; it files it in the Keychain and never returns it.
                    let label = new.name.trimmingCharacters(in: .whitespacesAndNewlines)
                    list = try await api.addAgentConnector(body: .json(.case2(.init(
                        agent: agent, apiKey: key, name: label.isEmpty ? nil : label, source: .apiKey)))).ok.body.json
                } else {
                    list = try await api.addAgentConnector(body: .json(.case1(.init(agent: agent, source: .login)))).ok.body.json
                }
            }
            let added = list.connectors.map(DaemonMapping.connector).first { $0.kind == new.kind && $0.name == new.name }
            guard let added = added ?? list.connectors.map(DaemonMapping.connector).last(where: { $0.kind == new.kind }) else {
                throw RatchetClientError.notFound(new.name)
            }
            return added
        }
    }

    public func githubSignInFlow() async throws -> GithubSignInFlow? {
        try await calling { DaemonMapping.githubFlow(try await api.listConnectors().ok.body.json.githubFlow) }
    }

    public func startGithubSignIn(connectorID: String?) async throws -> GithubSignInFlow? {
        try await calling {
            let list = try await api.startGithubConnectorSignIn(body: .json(.init(connectorId: connectorID))).ok.body.json
            return DaemonMapping.githubFlow(list.githubFlow)
        }
    }

    public func cancelGithubSignIn() async throws {
        try await calling { _ = try await api.cancelGithubConnectorSignIn().ok }
    }

    public func removeConnector(id: String) async throws {
        try await calling {
            _ = try await api.removeConnector(path: .init(id: id)).ok
        }
    }

    public func setDefaultConnector(id: String) async throws -> [Connector] {
        try await calling {
            try await api.setDefaultConnector(path: .init(id: id)).ok.body.json.connectors.map(DaemonMapping.connector)
        }
    }

    public func signInConnector(id: String) async throws -> Connector {
        try await calling {
            guard let connector = try await api.testConnector(path: .init(id: id)).ok.body.json.connector else {
                throw RatchetClientError.notFound(id)
            }
            return DaemonMapping.connector(connector)
        }
    }

    public func connectionSelection(repoID: String) async throws -> ConnectionSelection? {
        try await calling {
            DaemonMapping.selection(try await api.getConnectorSelection(query: .init(repo: repoID)).ok.body.json)
        }
    }

    public func saveConnectionSelection(_ selection: ConnectionSelection) async throws {
        try await calling {
            guard let aws = selection.awsID else { return }
            var github = selection.githubID
            if github == nil {
                github = try await api.getConnectorSelection(query: .init(repo: selection.repoID)).ok.body.json.effective.github
            }
            guard let github else { return }
            let input = VM.SaveRepoConnectionsInput(
                repo: selection.repoID, selection: DaemonMapping.repoConnections(selection, github: github, aws: aws))
            _ = try await api.saveConnectorSelection(body: .json(input)).ok
        }
    }

    // MARK: Environment drafts

    public func createDraft(_ request: DraftRequest) async throws -> EnvironmentDraft {
        try await calling {
            let input = VM.CreateEnvironmentDraftInput(agentConnector: .claudeCode, goal: request.goal, repo: request.repoID)
            let created = try await api.createEnvironmentDraft(body: .json(input)).created.body.json
            return try await settledDraft(created)
        }
    }

    public func reviseDraft(id: String, instruction: String) async throws -> EnvironmentDraft {
        try await calling {
            let revised = try await api.reviseEnvironmentDraft(path: .init(id: id), body: .json(.init(message: instruction))).ok.body.json
            return try await settledDraft(revised)
        }
    }

    public func approveDraft(id: String) async throws -> EnvironmentDraft {
        try await calling {
            DaemonMapping.draft(try await api.approveEnvironmentDraft(path: .init(id: id), body: .json(.init())).ok.body.json)
        }
    }

    /// An agent drafts the environment, so a new or revised draft is `drafting` for a while. Waits (up to 5 minutes) until
    /// it is ready, the way the mock answers at once.
    private func settledDraft(_ first: VM.EnvironmentDraftVm) async throws -> EnvironmentDraft {
        var draft = first
        var polls = 0
        while draft.status == .drafting {
            polls += 1
            guard polls <= 300 else { throw RatchetClientError.unavailable("Drafting the environment is taking too long.") }
            try await Task.sleep(for: .seconds(1))
            draft = try await api.getEnvironmentDraft(path: .init(id: draft.id)).ok.body.json
        }
        if draft.status == .failed {
            throw RatchetClientError.unavailable(draft.error ?? "The environment could not be drafted.")
        }
        return DaemonMapping.draft(draft)
    }

    // MARK: Sandbox

    public func sandboxStatus() async throws -> SandboxStatus {
        try await calling { DaemonMapping.sandbox(try await environments()) }
    }

    public func endSandbox() async throws -> TeardownReceipt {
        try await calling {
            guard let id = DaemonMapping.endableSandboxID(try await environments()) else {
                throw RatchetClientError.invalid("The sandbox is already off.")
            }
            let after = try await api.endEnvironment(path: .init(sandboxId: id)).ok.body.json
            guard let receipt = after.lastReceipt else { throw RatchetClientError.unavailable("The sandbox ended, but left no receipt.") }
            return await mappedReceipt(receipt)
        }
    }

    public func sandboxReceipt() async throws -> TeardownReceipt? {
        try await calling {
            guard let receipt = try await environments().lastReceipt else { return nil }
            return await mappedReceipt(receipt)
        }
    }

    /// The receipt has no start time; the earliest start of the runs it covers is the best answer.
    private func mappedReceipt(_ receipt: VM.TeardownReceiptVm) async -> TeardownReceipt {
        let covered = Set(receipt.runs.map(\.optimizationId))
        let starts = ((try? await optimizationSnapshot()) ?? []).filter { covered.contains($0.id) }.map(\.createdAt)
        return DaemonMapping.receipt(receipt, startedAt: starts.min().map(DaemonMapping.date(ms:)))
    }

    // MARK: Events

    public func events() async -> AsyncStream<RatchetEvent> {
        let (stream, continuation) = AsyncStream<RatchetEvent>.makeStream(bufferingPolicy: .unbounded)
        let task = Task { await self.follow(into: continuation) }
        continuation.onTermination = { _ in task.cancel() }
        return stream
    }

    /// Keeps one event connection open, reconnecting with `Last-Event-ID` and a growing pause until cancelled.
    private func follow(into continuation: AsyncStream<RatchetEvent>.Continuation) async {
        var lastID: Int64?
        var pause = 0.5
        var connectedBefore = false
        while !Task.isCancelled {
            do {
                let response = try await api.streamEvents(headers: .init(lastEventID: lastID))
                let body = try response.ok.body.textEventStream
                // After a dropped connection with nothing to resume from, whatever happened meanwhile is lost.
                if connectedBefore && lastID == nil { continuation.yield(.resync) }
                connectedBefore = true
                pause = 0.5
                var parser = SSEParser()
                for try await chunk in body {
                    var batch: [DaemonEvent] = []
                    for message in parser.feed(Array(chunk)) {
                        if let id = message.id, let number = Int64(id) { lastID = number }
                        guard let event = DaemonEvent.decode(message) else { continue }
                        switch event {
                        case .hello(let newest, _):
                            if lastID == nil { lastID = newest }
                        case .resync:
                            lastID = nil
                            continuation.yield(.resync)
                        default:
                            if !batch.contains(event) { batch.append(event) }
                        }
                    }
                    for event in batch { await deliver(event, into: continuation) }
                }
            } catch is CancellationError {
                break
            } catch {
                // The daemon is away or restarting: try again below.
            }
            if Task.isCancelled { break }
            do { try await Task.sleep(for: .seconds(pause)) } catch { break }
            pause = min(pause * 2, 10)
        }
        continuation.finish()
    }

    /// Turns what the daemon announced into the full models the app applies. The announcements carry ids only, so each
    /// one is a refetch of the thing that changed. A failed refetch is skipped: the next event repairs it.
    func deliver(_ event: DaemonEvent, into continuation: AsyncStream<RatchetEvent>.Continuation) async {
        switch event {
        case .optimization(let id, _), .spend(let id, _):
            await emitOptimization(id, into: continuation)
            if case .optimization = event { await emitNeedsYou(into: continuation) }
        case .ratchet(let id, _, _):
            if let moves = try? await ratchetMoves(optimizationID: id, start: (try? await fetchOptimization(id))?.startedAt) {
                for move in moves { continuation.yield(.moveAdded(optimizationID: id, move: move)) }
            }
            await emitOptimization(id, into: continuation)
        case .candidate(let id, let candidateID, _):
            if let list = try? await api.listCandidates(path: .init(id: id)).ok.body.json,
               let changed = list.first(where: { $0.id == candidateID }) {
                continuation.yield(.candidateUpdated(optimizationID: id, candidate: DaemonMapping.candidate(changed)))
            }
        case .agentEvent(let id, let agentID):
            if let detail = try? await optimizationDetail(id), let agent = detail.agents.first(where: { $0.id == agentID }) {
                continuation.yield(.agentUpdated(optimizationID: id, agent: DaemonMapping.agent(agent)))
            }
        case .sandbox:
            if let environments = try? await environments() {
                continuation.yield(.sandboxChanged(DaemonMapping.sandbox(environments)))
                if let receipt = environments.lastReceipt {
                    continuation.yield(.receiptIssued(await mappedReceipt(receipt)))
                }
            }
        case .connector, .github:
            if let list = try? await connectors() { continuation.yield(.connectorsChanged(list)) }
            if let flow = try? await githubSignInFlow() {
                continuation.yield(.githubFlowChanged(flow))
            } else {
                continuation.yield(.githubFlowChanged(nil))
            }
            await emitNeedsYou(into: continuation)
        case .environmentDraft(let draftID, let status):
            if status == "ready", let draft = try? await api.getEnvironmentDraft(path: .init(id: draftID)).ok.body.json {
                continuation.yield(.environmentDraftReady(draftID: draftID, repoID: draft.repo))
            }
            await emitNeedsYou(into: continuation)
        case .hello, .resync, .notice, .runActive, .unknown:
            break
        }
    }

    private func emitOptimization(_ id: String, into continuation: AsyncStream<RatchetEvent>.Continuation) async {
        guard let detail = try? await optimizationDetail(id) else { return }
        continuation.yield(.optimizationUpdated(DaemonMapping.optimization(detail.summary, detail: detail)))
        for agent in detail.agents {
            continuation.yield(.agentUpdated(optimizationID: id, agent: DaemonMapping.agent(agent)))
        }
    }

    /// "Needs you" comes from the overview.
    private func emitNeedsYou(into continuation: AsyncStream<RatchetEvent>.Continuation) async {
        guard let vm = try? await api.getOverview().ok.body.json else { return }
        continuation.yield(.needsYouChanged(vm.needsYou.map(DaemonMapping.needsYou)))
    }
}

/// What the client remembers between calls.
private actor DaemonClientState {
    private var agentOwners: [String: String] = [:]

    func remember(agents: [String], of optimizationID: String) {
        for agent in agents { agentOwners[agent] = optimizationID }
    }

    func owner(ofAgent id: String) -> String? { agentOwners[id] }

    private var rows: (task: Task<[DaemonRatchetClient.OptimizationRow], any Error>, at: Date)?

    /// The load of the optimization table started within the last two seconds, or a new one.
    func rowsTask(
        make: () -> Task<[DaemonRatchetClient.OptimizationRow], any Error>
    ) -> Task<[DaemonRatchetClient.OptimizationRow], any Error> {
        if let rows, Date().timeIntervalSince(rows.at) < 2 { return rows.task }
        let task = make()
        rows = (task, Date())
        return task
    }

    func forgetRows() { rows = nil }
}
