import Foundation

/// A `RatchetClient` backed by sample data, with a fake live event stream so running optimizations visibly progress.
///
/// With `live: true` a timer calls `advance()` every `interval`; each call moves every running optimization one
/// simulated minute forward: a candidate appears, gets a verdict on the next step, a win becomes the new best, money
/// is spent, time runs down, and at zero the run ends, the sandbox is torn down and a receipt is issued. Tests and
/// snapshots use `live: false` and call `advance()` themselves.
public actor MockRatchetClient: RatchetClient {
    private var world: MockWorld
    private let live: Bool
    private let interval: Duration
    private var subscribers: [UUID: AsyncStream<RatchetEvent>.Continuation] = [:]
    private var ticker: Task<Void, Never>?
    private var scriptIndex: [String: Int] = [:]
    private var pending: [String: String] = [:]
    private var counter = 0

    public init(now: Date = Date(), live: Bool = false, interval: Duration = .seconds(2)) {
        self.world = MockWorld.standard(now: now)
        self.live = live
        self.interval = interval
    }

    // MARK: Overview and spend

    public func overview(range: TimeRange) async throws -> Overview {
        makeOverview(range)
    }

    public func spend(range: TimeRange) async throws -> SpendSummary {
        let overview = makeOverview(range)
        let aws = (overview.spentUSD * 0.66).rounded()
        return SpendSummary(
            range: range, totalUSD: overview.spentUSD, budgetUSD: overview.budgetUSD,
            lines: [
                SpendLine(id: "aws", label: "AWS sandboxes", usd: aws),
                SpendLine(id: "agents", label: "Agent keys", usd: overview.spentUSD - aws),
            ])
    }

    private func makeOverview(_ range: TimeRange) -> Overview {
        let windowDays = Double(range.days ?? 120)
        let start = world.now.addingTimeInterval(-windowDays * MockWorld.day)
        let wins = world.winSeries.sorted { $0.date < $1.date }
        let before = wins.last { $0.date < start }?.valueUSD ?? 0
        let inside = wins.filter { $0.date >= start }
        var series = [ValuePoint(id: "start", date: start, valuePerYearUSD: before)]
        for win in inside {
            series.append(ValuePoint(
                id: "win-\(Int(win.date.timeIntervalSince1970))", date: win.date, valuePerYearUSD: win.valueUSD,
                win: win.title, optimizationTitle: win.optimizationTitle))
        }
        series.append(ValuePoint(id: "end", date: world.now, valuePerYearUSD: wins.last?.valueUSD ?? 0))
        let spent = world.optimizations
            .filter { ($0.startedAt ?? world.now) >= start }
            .reduce(0) { $0 + $1.spentUSD }
        return Overview(
            range: range, spentUSD: spent, budgetUSD: world.budgetUSD, valueLockedPerYearUSD: totalValue(),
            winsCount: inside.count, series: series, needsYou: needsYou())
    }

    private func totalValue() -> Double {
        world.optimizations.reduce(0) { $0 + ($1.valuePerYearUSD ?? 0) }
    }

    private func needsYou() -> [NeedsYouItem] {
        var items: [NeedsYouItem] = []
        for optimization in world.optimizations {
            if optimization.status == .waiting {
                items.append(NeedsYouItem(
                    id: "approve-" + optimization.id, kind: .approveEnvironment, title: "Approve environment",
                    detail: optimization.title + " · full stack, $0.59/h", actionTitle: "Review", optimizationID: optimization.id))
            } else if optimization.status == .done, let pr = optimization.prNumber {
                var parts = [optimization.title]
                if let change = optimization.change { parts.append(Percent.signed(change)) }
                parts.append("PR #\(pr)")
                items.append(NeedsYouItem(
                    id: "merge-" + optimization.id, kind: .mergeWins,
                    title: "Merge " + Figure.wins(optimization.winsCount),
                    detail: parts.joined(separator: " · "), actionTitle: "Open PR", optimizationID: optimization.id))
            }
        }
        return items
    }

    // MARK: Optimizations

    public func optimizations() async throws -> [Optimization] {
        world.optimizations
    }

    public func optimization(id: String) async throws -> OptimizationDetail {
        guard let detail = world.details[id] else { throw RatchetClientError.notFound("optimization \(id)") }
        return detail
    }

    public func candidates(optimizationID: String) async throws -> [Candidate] {
        try await optimization(id: optimizationID).candidates
    }

    public func ratchetHistory(optimizationID: String) async throws -> [RatchetMove] {
        try await optimization(id: optimizationID).moves
    }

    public func startOptimization(_ request: StartRequest) async throws -> Optimization {
        let goal = request.goal.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { throw RatchetClientError.invalid("Say what should get better.") }
        guard request.budgetUSD > 0, request.timeLimitHours > 0 else {
            throw RatchetClientError.invalid("Budget and time must be more than zero.")
        }
        counter += 1
        let id = "opt-new-\(counter)"
        let optimization = Optimization(
            id: id, title: goal, repo: request.repoID, branch: request.branch, goal: goal, status: .running,
            metric: MetricSpec(name: "p95 latency", unit: "ms"), baseline: 400, noise: 0.02, budgetUSD: request.budgetUSD,
            secondsLeft: request.timeLimitHours * 3600, timeLimitHours: request.timeLimitHours, startedAt: world.now,
            agentsCount: max(request.selection.agents.count, 1))
        var agents: [Agent] = []
        for (agentID, placement) in request.selection.agents.sorted(by: { $0.key < $1.key }) {
            let name = world.connectors.first { $0.id == agentID }?.name ?? agentID
            agents.append(Agent(id: "\(id)-\(agentID)", name: name, placement: placement, state: .working, activity: "Reading the repo"))
        }
        world.optimizations.insert(optimization, at: 0)
        world.details[id] = OptimizationDetail(optimization: optimization, agents: agents)
        var events: [RatchetEvent] = [.optimizationUpdated(optimization)]
        if !world.sandbox.isOn, let account = request.selection.awsID.flatMap({ aws in world.connectors.first { $0.id == aws }?.name }) {
            world.sandbox = SandboxStatus(isOn: true, account: account, costSoFarUSD: 0, offAt: world.now.addingTimeInterval(request.timeLimitHours * 3600))
            events.append(.sandboxChanged(world.sandbox))
        }
        emit(events)
        return optimization
    }

    // MARK: Agents

    public func agents(optimizationID: String) async throws -> [Agent] {
        try await optimization(id: optimizationID).agents
    }

    public func pauseAgent(id: String) async throws -> Agent {
        try updateAgent(id: id) { agent in
            agent.state = .paused
            agent.activity = "Paused by you"
        }
    }

    public func resumeAgent(id: String) async throws -> Agent {
        try updateAgent(id: id) { agent in
            agent.state = .working
            agent.activity = "Picking up where it stopped"
        }
    }

    public func messageAgent(id: String, text: String) async throws {
        let note = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !note.isEmpty else { throw RatchetClientError.invalid("Write a message first.") }
        _ = try updateAgent(id: id) { agent in agent.activity = "Reading your note: " + note }
    }

    private func updateAgent(id: String, _ change: (inout Agent) -> Void) throws -> Agent {
        for (optimizationID, var detail) in world.details {
            guard let index = detail.agents.firstIndex(where: { $0.id == id }) else { continue }
            change(&detail.agents[index])
            world.details[optimizationID] = detail
            let agent = detail.agents[index]
            emit([.agentUpdated(optimizationID: optimizationID, agent: agent)])
            return agent
        }
        throw RatchetClientError.notFound("agent \(id)")
    }

    // MARK: Benchmarks, templates, repos

    public func benchmarks() async throws -> [BenchmarkRun] { world.benchmarks }

    public func templates() async throws -> [Template] { world.templates }

    public func repos() async throws -> [Repo] { world.repos }

    public func detectHarness(repoID: String) async throws -> HarnessDetection {
        guard world.repos.contains(where: { $0.id == repoID }) else { throw RatchetClientError.notFound("repo \(repoID)") }
        switch repoID {
        case "acme-org/shop-api":
            return HarnessDetection(repoID: repoID, found: true,
                                    summary: "Found a k6 script in bench/ and a docker-compose.yml. Looks like an HTTP latency test.",
                                    suggestedTemplateID: "http-latency")
        case "srikanth235/acme-cli":
            return HarnessDetection(repoID: repoID, found: true, summary: "Found a hyperfine script in bench/.",
                                    suggestedTemplateID: "cli-cold-start")
        default:
            return HarnessDetection(repoID: repoID, found: false,
                                    summary: "No benchmark found in this repo. Start from a template.", suggestedTemplateID: "build-size")
        }
    }

    // MARK: Connectors

    public func connectors() async throws -> [Connector] { world.connectors }

    public func addConnector(_ new: NewConnector) async throws -> Connector {
        let name = new.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw RatchetClientError.invalid("Give the connector a name.") }
        counter += 1
        let hasDefault = world.connectors.contains { $0.kind == new.kind && $0.isDefault }
        let connector = Connector(
            id: "\(new.kind.rawValue)-added-\(counter)", kind: new.kind, name: name,
            detail: new.detail.isEmpty ? "Added just now" : new.detail,
            state: new.kind == .agent ? .valid : .connected, isDefault: !hasDefault,
            monthlyCapUSD: new.kind == .aws ? 100 : nil, canRunInCloud: new.kind == .agent)
        world.connectors.append(connector)
        emit([.connectorsChanged(world.connectors)])
        return connector
    }

    public func removeConnector(id: String) async throws {
        guard let index = world.connectors.firstIndex(where: { $0.id == id }) else {
            throw RatchetClientError.notFound("connector \(id)")
        }
        let removed = world.connectors.remove(at: index)
        if removed.isDefault, let next = world.connectors.firstIndex(where: { $0.kind == removed.kind && $0.state.isUsable }) {
            world.connectors[next].isDefault = true
        }
        emit([.connectorsChanged(world.connectors)])
    }

    public func setDefaultConnector(id: String) async throws -> [Connector] {
        guard let kind = world.connectors.first(where: { $0.id == id })?.kind else {
            throw RatchetClientError.notFound("connector \(id)")
        }
        for index in world.connectors.indices where world.connectors[index].kind == kind {
            world.connectors[index].isDefault = world.connectors[index].id == id
        }
        emit([.connectorsChanged(world.connectors)])
        return world.connectors
    }

    public func connectionSelection(repoID: String) async throws -> ConnectionSelection? {
        world.selections[repoID]
    }

    public func saveConnectionSelection(_ selection: ConnectionSelection) async throws {
        world.selections[selection.repoID] = selection
    }

    // MARK: Environment drafts

    public func createDraft(_ request: DraftRequest) async throws -> EnvironmentDraft {
        guard world.repos.contains(where: { $0.id == request.repoID }) else { throw RatchetClientError.notFound("repo \(request.repoID)") }
        counter += 1
        let id = "draft-\(counter)"
        let singleTemplates: Set<String> = ["cli-cold-start", "build-size", "test-suite-time", "memory"]
        let small = singleTemplates.contains(request.templateID ?? "") || request.repoID.hasSuffix("cli") || request.repoID.hasSuffix("web-app")
        let draft = small ? Self.singleDraft(id: id, repoID: request.repoID) : MockWorld.fullStackDraft(id: id, repoID: request.repoID)
        world.drafts[id] = draft
        return draft
    }

    public func reviseDraft(id: String, instruction: String) async throws -> EnvironmentDraft {
        guard var draft = world.drafts[id] else { throw RatchetClientError.notFound("draft \(id)") }
        let text = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw RatchetClientError.invalid("Say what to change.") }
        let lower = text.lowercased()
        if lower.contains("smaller") || lower.contains("single") || lower.contains("one machine") {
            let small = Self.singleDraft(id: id, repoID: draft.repoID)
            draft.scope = small.scope
            draft.headline = small.headline
            draft.emphasis = small.emphasis
            draft.reason = small.reason
            draft.resources = small.resources
            draft.files = small.files
            draft.readyMinutes = small.readyMinutes
        } else if lower.contains("rds"), let index = draft.resources.firstIndex(where: { $0.id == "pg" }) {
            draft.resources[index] = DraftResource(
                id: "pg", name: "Amazon RDS Postgres 16", detail: "restored from a snapshot of anonymised orders",
                machine: "db.r6g.large", usdPerHour: 0.26)
            draft.reason = "Postgres now runs on RDS, as you asked."
        } else {
            draft.reason = "Noted: " + text
        }
        draft.revision += 1
        draft.state = .draft
        world.drafts[id] = draft
        return draft
    }

    public func approveDraft(id: String) async throws -> EnvironmentDraft {
        guard var draft = world.drafts[id] else { throw RatchetClientError.notFound("draft \(id)") }
        draft.state = .approved
        world.drafts[id] = draft
        return draft
    }

    private static func singleDraft(id: String, repoID: String) -> EnvironmentDraft {
        EnvironmentDraft(
            id: id, repoID: repoID, scope: .single,
            headline: "This task needs one machine: the project built from the repo, measured on identical hardware.",
            emphasis: "one machine",
            reason: "The measurement does not depend on other services.",
            draftedBy: "Claude Code", draftSeconds: 45, draftCostUSD: 0.12, readyMinutes: 3,
            resources: [DraftResource(id: "runner", name: "Runner", detail: "builds and measures the repo", machine: "c7i.large", usdPerHour: 0.09)],
            files: ["main.tf", "bench.sh"],
            checks: "isolated, results sealed, same hardware every time, full teardown")
    }

    // MARK: Sandbox

    public func sandboxStatus() async throws -> SandboxStatus { world.sandbox }

    public func endSandbox() async throws -> TeardownReceipt {
        guard world.sandbox.isOn else { throw RatchetClientError.invalid("The sandbox is already off.") }
        counter += 1
        let receipt = TeardownReceipt(
            id: "receipt-end-\(counter)", account: world.sandbox.account ?? "acme-prod",
            startedAt: world.now.addingTimeInterval(-6480), endedAt: world.now, costUSD: world.sandbox.costSoFarUSD,
            destroyed: ["EC2 c7i.2xlarge × 1", "EC2 r7g.large × 1", "EBS 120 GB", "Security group", "Key pair"])
        world.sandbox = .off
        world.receipt = receipt
        emit([.sandboxChanged(.off), .receiptIssued(receipt)])
        return receipt
    }

    public func sandboxReceipt() async throws -> TeardownReceipt? { world.receipt }

    // MARK: Events

    public func events() async -> AsyncStream<RatchetEvent> {
        let (stream, continuation) = AsyncStream<RatchetEvent>.makeStream()
        let id = UUID()
        subscribers[id] = continuation
        continuation.onTermination = { @Sendable [weak self] _ in
            Task { await self?.removeSubscriber(id) }
        }
        startTickerIfNeeded()
        return stream
    }

    private func removeSubscriber(_ id: UUID) {
        subscribers[id] = nil
    }

    private func emit(_ events: [RatchetEvent]) {
        for event in events {
            for continuation in subscribers.values { continuation.yield(event) }
        }
    }

    private func startTickerIfNeeded() {
        guard live, ticker == nil else { return }
        let interval = self.interval
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: interval)
                guard let self else { return }
                await self.advance()
            }
        }
    }

    /// Stops the fake live updates.
    public func stop() {
        ticker?.cancel()
        ticker = nil
    }

    // MARK: Simulation

    private struct ScriptStep {
        var title: String
        var agent: String
        var verdict: Verdict
        var gain: Double
    }

    private static let script: [ScriptStep] = [
        ScriptStep(title: "Prefetch the shipping rates", agent: "Claude Code · team key", verdict: .verified, gain: 0.04),
        ScriptStep(title: "Trim the session lookup", agent: "Claude Code · your login", verdict: .noise, gain: 0),
        ScriptStep(title: "Warm the pricing cache", agent: "Claude Code · your login", verdict: .verified, gain: 0.03),
        ScriptStep(title: "Rewrite the cart serializer", agent: "Claude Code · team key", verdict: .blocked, gain: 0),
        ScriptStep(title: "Make the audit write async", agent: "Claude Code · team key", verdict: .verified, gain: 0.02),
    ]

    /// Moves every running optimization one simulated minute forward and returns what changed (also sent to the
    /// event stream).
    @discardableResult
    public func advance() -> [RatchetEvent] {
        world.now = world.now.addingTimeInterval(60)
        var events: [RatchetEvent] = []
        for optimization in world.optimizations where optimization.status == .running {
            events += advance(optimizationID: optimization.id)
        }
        emit(events)
        return events
    }

    private func advance(optimizationID id: String) -> [RatchetEvent] {
        guard var detail = world.details[id] else { return [] }
        var optimization = detail.optimization
        guard detail.agents.contains(where: { $0.state == .working }) else { return [] }
        var events: [RatchetEvent] = []

        optimization.spentUSD += 0.4
        optimization.secondsLeft = max((optimization.secondsLeft ?? 0) - 60, 0)
        if world.sandbox.isOn { world.sandbox.costSoFarUSD += 0.01 }

        let index = scriptIndex[id, default: 0]
        if let candidateID = pending[id], let at = detail.candidates.firstIndex(where: { $0.id == candidateID }) {
            // Give the evaluating candidate its verdict.
            let step = Self.script[(index - 1 + Self.script.count) % Self.script.count]
            var candidate = detail.candidates[at]
            candidate.verdict = step.verdict
            let current = optimization.best ?? optimization.baseline
            switch step.verdict {
            case .verified:
                let better = optimization.metric.lowerIsBetter ? current * (1 - step.gain) : current * (1 + step.gain)
                candidate.value = better
                optimization.best = better
                optimization.winsCount += 1
                optimization.valuePerYearUSD = (optimization.valuePerYearUSD ?? 0) + 450
                let move = RatchetMove(id: "\(id)-move-\(detail.moves.count + 1)", at: world.now, value: better, title: candidate.title)
                detail.moves.append(move)
                world.winSeries.append(MockWorld.WinStep(
                    date: world.now, valueUSD: totalValue() + 450, title: candidate.title, optimizationTitle: optimization.title))
                events.append(.moveAdded(optimizationID: id, move: move))
            case .noise:
                candidate.value = current * 1.01
            default:
                candidate.value = nil
            }
            optimization.evalsCount += 1
            detail.candidates[at] = candidate
            events.append(.candidateUpdated(optimizationID: id, candidate: candidate))
            pending[id] = nil
            bump(&detail, agentName: candidate.agent, evalsBy: 1, winsBy: step.verdict == .verified ? 1 : 0, activity: nil, events: &events, id: id)
        } else {
            // A new candidate starts evaluating.
            let step = Self.script[index % Self.script.count]
            scriptIndex[id] = index + 1
            let candidate = Candidate(
                id: String(format: "%07lx", 0x5a31c00 + (index + 1) * 4099), title: step.title, agent: step.agent,
                verdict: .evaluating, value: nil, at: world.now)
            detail.candidates.insert(candidate, at: 0)
            pending[id] = candidate.id
            events.append(.candidateUpdated(optimizationID: id, candidate: candidate))
            bump(&detail, agentName: step.agent, evalsBy: 0, winsBy: 0, activity: step.title, events: &events, id: id)
        }

        if (optimization.secondsLeft ?? 0) <= 0, pending[id] == nil {
            optimization.status = .done
            optimization.secondsLeft = nil
            optimization.prNumber = 215
            for at in detail.agents.indices {
                detail.agents[at].state = .idle
                detail.agents[at].activity = "Finished"
                events.append(.agentUpdated(optimizationID: id, agent: detail.agents[at]))
            }
            if world.sandbox.isOn {
                let receipt = TeardownReceipt(
                    id: "receipt-\(id)", account: world.sandbox.account ?? "acme-prod",
                    startedAt: optimization.startedAt ?? world.now, endedAt: world.now, costUSD: world.sandbox.costSoFarUSD,
                    destroyed: ["EC2 c7i.2xlarge × 1", "EC2 r7g.large × 1", "EBS 120 GB", "Security group", "Key pair"])
                detail.receipt = receipt
                world.receipt = receipt
                world.sandbox = .off
                events.append(.sandboxChanged(.off))
                events.append(.receiptIssued(receipt))
            }
        }

        detail.optimization = optimization
        world.details[id] = detail
        if let at = world.optimizations.firstIndex(where: { $0.id == id }) { world.optimizations[at] = optimization }
        events.append(.optimizationUpdated(optimization))
        if world.sandbox.isOn { events.append(.sandboxChanged(world.sandbox)) }
        if optimization.status == .done { events.append(.needsYouChanged(needsYou())) }
        return events
    }

    private func bump(_ detail: inout OptimizationDetail, agentName: String, evalsBy: Int, winsBy: Int, activity: String?,
                      events: inout [RatchetEvent], id: String) {
        guard let at = detail.agents.firstIndex(where: { $0.name == agentName }) ?? detail.agents.indices.first else { return }
        detail.agents[at].evals += evalsBy
        detail.agents[at].wins += winsBy
        if let activity { detail.agents[at].activity = activity }
        events.append(.agentUpdated(optimizationID: id, agent: detail.agents[at]))
    }
}
