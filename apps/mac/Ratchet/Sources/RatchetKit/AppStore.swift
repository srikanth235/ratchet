import Foundation
import Observation

public enum OptimizationFilter: String, CaseIterable, Identifiable, Sendable {
    case all, running, done

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .all: "All"
        case .running: "Running"
        case .done: "Done"
        }
    }

    public func includes(_ optimization: Optimization) -> Bool {
        switch self {
        case .all: true
        case .running: optimization.isLive
        case .done: optimization.status.isFinished
        }
    }
}

/// What the app shows right after launch, before the daemon has answered.
struct CachedState: Codable {
    var overview: Overview?
    var optimizations: [Optimization]
    var connectors: [Connector]
    var templates: [Template]
    var benchmarks: [BenchmarkRun]
    var repos: [Repo]
    var sandbox: SandboxStatus
    var receipt: TeardownReceipt?
}

/// The app's only state. It renders the last snapshot at once (`cacheURL`), refreshes from the client, then applies
/// the event stream. Views read it; actions go through its methods, which call the client.
@MainActor @Observable
public final class AppStore {
    public let client: any RatchetClient

    public private(set) var overview: Overview?
    public private(set) var optimizations: [Optimization] = []
    public private(set) var details: [String: OptimizationDetail] = [:]
    public private(set) var connectors: [Connector] = []
    public private(set) var templates: [Template] = []
    public private(set) var benchmarks: [BenchmarkRun] = []
    public private(set) var repos: [Repo] = []
    public private(set) var sandbox: SandboxStatus = .off
    public private(set) var receipt: TeardownReceipt?
    public private(set) var isLoaded = false
    public private(set) var lastError: String?
    /// The GitHub sign-in in progress: its code is shown until the account appears or the flow ends.
    public private(set) var githubFlow: GithubSignInFlow?
    /// Agent logs, result details and benchmark series of what was opened (see RatchetClient+Insights.swift).
    public internal(set) var insight = InsightsState()
    /// Whether the keychain can hold tokens and keys, and whether "Sign in with GitHub" has a real client id.
    public private(set) var connectorSetup = ConnectorSetup()
    public var range: TimeRange = .days90
    /// True when the data is the sample or the simulation (demo mode): the sidebar says so, quietly.
    public private(set) var isDemoData: Bool

    /// Called for each event worth a notification (a win to merge, a run that finished...). The app decides whether and
    /// how to show it; the store only decides that it happened.
    @ObservationIgnored public var onNotification: (@MainActor (NotificationRequest) -> Void)?
    /// Which categories the user has switched on.
    @ObservationIgnored public var enabledNotifications: @MainActor () -> Set<NotificationCategory> = { Set(NotificationCategory.allCases) }

    private var cacheURL: URL?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    @ObservationIgnored private var pendingWins: [String: RatchetMove] = [:]

    public init(client: any RatchetClient, cacheURL: URL? = nil, isDemoData: Bool = false) {
        self.client = client
        self.cacheURL = cacheURL
        self.isDemoData = isDemoData
        restoreCache()
    }

    /// Switches between the real daemon and the demo daemon (same socket): forgets what is shown, then reloads.
    public func useSource(isDemoData: Bool, cacheURL: URL?) async {
        self.isDemoData = isDemoData
        self.cacheURL = cacheURL
        overview = nil
        optimizations = []
        details = [:]
        connectors = []
        benchmarks = []
        repos = []
        sandbox = .off
        receipt = nil
        githubFlow = nil
        insight = InsightsState()
        isLoaded = false
        restoreCache()
        await refresh()
    }

    // MARK: Loading

    /// Loads everything, then follows the event stream.
    public func start() async {
        await refresh()
        startEvents()
    }

    public func refresh() async {
        do {
            overview = try await client.overview(range: range)
            optimizations = try await client.optimizations()
            connectors = try await client.connectors()
            githubFlow = try await client.githubSignInFlow()
            connectorSetup = (try? await client.connectorSetup()) ?? connectorSetup
            templates = try await client.templates()
            benchmarks = try await client.benchmarks()
            repos = try await client.repos()
            sandbox = try await client.sandboxStatus()
            receipt = try await client.sandboxReceipt()
            isLoaded = true
            lastError = nil
            persist()
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func startEvents() {
        guard eventTask == nil else { return }
        let client = self.client
        eventTask = Task { [weak self] in
            let stream = await client.events()
            for await event in stream {
                guard let self else { return }
                self.apply(event)
            }
        }
    }

    public func stopEvents() {
        eventTask?.cancel()
        eventTask = nil
    }

    public func setRange(_ newRange: TimeRange) async {
        range = newRange
        do {
            overview = try await client.overview(range: newRange)
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func loadDetail(id: String) async {
        do {
            details[id] = try await client.optimization(id: id)
        } catch {
            lastError = error.localizedDescription
        }
    }

    // MARK: Derived

    public var runningOptimizations: [Optimization] { optimizations.filter { $0.status == .running } }

    public var needsYou: [NeedsYouItem] { overview?.needsYou ?? [] }

    public var needsYouCount: Int { needsYou.count }

    public func optimization(id: String) -> Optimization? {
        optimizations.first { $0.id == id }
    }

    public func visibleOptimizations(_ filter: OptimizationFilter) -> [Optimization] {
        optimizations.filter { filter.includes($0) }
    }

    public func connectors(of kind: ConnectorKind) -> [Connector] {
        connectors.filter { $0.kind == kind }
    }

    // MARK: Events

    private var observed: ObservedState {
        ObservedState(optimizations: optimizations, needsYou: needsYou, connectors: connectors)
    }

    /// Applies one server-sent event to what is on screen, and reports what is new that deserves a notification.
    public func apply(_ event: RatchetEvent) {
        let before = observed
        applyEvent(event)
        guard let onNotification, isLoaded else { return }
        let enabled = enabledNotifications()
        if case .environmentDraftReady(let draftID, let repoID) = event {
            if let request = NotificationRouting.environmentDrafted(draftID: draftID, repoID: repoID, enabled: enabled) {
                onNotification(request)
            }
            return
        }
        for request in NotificationRouting.changes(from: before, to: observed, enabled: enabled) { onNotification(request) }
    }

    private func applyEvent(_ event: RatchetEvent) {
        switch event {
        case .optimizationUpdated(let new):
            applyUpdate(new)
        case .candidateUpdated(let id, let candidate):
            guard var detail = details[id] else { return }
            if let at = detail.candidates.firstIndex(where: { $0.id == candidate.id }) {
                detail.candidates[at] = candidate
            } else {
                detail.candidates.insert(candidate, at: 0)
            }
            details[id] = detail
        case .moveAdded(let id, let move):
            pendingWins[id] = move
            guard var detail = details[id], !detail.moves.contains(where: { $0.id == move.id }) else { return }
            detail.moves.append(move)
            details[id] = detail
        case .agentUpdated(let id, let agent):
            guard var detail = details[id] else { return }
            if let at = detail.agents.firstIndex(where: { $0.id == agent.id }) {
                detail.agents[at] = agent
            } else {
                detail.agents.append(agent)
            }
            details[id] = detail
        case .agentLogAppended(_, let agentID):
            agentLogChanged(agentID)
        case .needsYouChanged(let items):
            overview?.needsYou = items
        case .connectorsChanged(let list):
            connectors = list
        case .githubFlowChanged(let flow):
            githubFlow = flow
        case .environmentDraftReady:
            break
        case .sandboxChanged(let status):
            sandbox = status
        case .receiptIssued(let issued):
            receipt = issued
        case .resync:
            Task { await resync() }
        }
    }

    /// The event stream lost its place: reload the lists and every open detail.
    private func resync() async {
        await refresh()
        for id in Array(details.keys) { await loadDetail(id: id) }
        for agentID in Array(insight.logs.keys) { await loadAgentLog(agentID) }
        for id in Array(insight.insights.keys) { await loadInsights(id) }
    }

    private func applyUpdate(_ new: Optimization) {
        let old = optimizations.first { $0.id == new.id }
        if let at = optimizations.firstIndex(where: { $0.id == new.id }) {
            optimizations[at] = new
        } else {
            optimizations.insert(new, at: 0)
        }
        if var detail = details[new.id] {
            detail.optimization = new
            details[new.id] = detail
        }
        guard var current = overview else { return }
        let spentDelta = new.spentUSD - (old?.spentUSD ?? 0)
        let valueDelta = (new.valuePerYearUSD ?? 0) - (old?.valuePerYearUSD ?? 0)
        let winsDelta = new.winsCount - (old?.winsCount ?? 0)
        current.spentUSD += spentDelta
        if valueDelta != 0 {
            current.valueLockedPerYearUSD += valueDelta
            if var last = current.series.last, last.win == nil {
                last.valuePerYearUSD = current.valueLockedPerYearUSD
                current.series.removeLast()
                if valueDelta > 0 {
                    let move = pendingWins.removeValue(forKey: new.id)
                    let date = move?.at ?? max(last.date, Date())
                    current.series.append(ValuePoint(
                        id: "win-\(new.id)-\(new.winsCount)", date: date, valuePerYearUSD: current.valueLockedPerYearUSD,
                        win: move?.title ?? "Verified win", optimizationTitle: new.title))
                    last.date = max(last.date, date)
                }
                current.series.append(last)
            }
        }
        if winsDelta > 0 { current.winsCount += winsDelta }
        overview = current
    }

    // MARK: Actions

    /// Puts a just-started optimization on screen without waiting for its event.
    public func register(_ optimization: Optimization) async {
        applyUpdate(optimization)
        await loadDetail(id: optimization.id)
        if let sandboxNow = try? await client.sandboxStatus() { sandbox = sandboxNow }
        persist()
    }

    /// The kill switch. Reloads the optimization so the screen shows how it ended.
    public func stopOptimization(_ id: String) async {
        do {
            try await client.stopOptimization(id: id)
            await refreshOptimization(id)
        } catch {
            lastError = error.localizedDescription
        }
    }

    /// Opens the pull request for the verified wins. Returns where it is, or nil (and `lastError`) when it could not.
    public func openPullRequest(_ id: String) async -> URL? {
        do {
            let url = try await client.openPullRequest(optimizationID: id)
            await refreshOptimization(id)
            return url
        } catch {
            lastError = error.localizedDescription
            return nil
        }
    }

    public func report(_ id: String) async -> OptimizationReport? {
        do {
            return try await client.report(optimizationID: id)
        } catch {
            lastError = error.localizedDescription
            return nil
        }
    }

    /// Something the views did (saving a file) failed: shown like any other error.
    public func reportProblem(_ text: String) { lastError = text }

    private func refreshOptimization(_ id: String) async {
        await loadDetail(id: id)
        if let fresh = details[id]?.optimization { applyUpdate(fresh) }
    }

    public func pauseAgent(_ id: String) async {
        do {
            let agent = try await client.pauseAgent(id: id)
            if let optimizationID = optimizationID(ofAgent: id) { apply(.agentUpdated(optimizationID: optimizationID, agent: agent)) }
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func resumeAgent(_ id: String) async {
        do {
            let agent = try await client.resumeAgent(id: id)
            if let optimizationID = optimizationID(ofAgent: id) { apply(.agentUpdated(optimizationID: optimizationID, agent: agent)) }
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func messageAgent(_ id: String, text: String) async {
        do {
            try await client.messageAgent(id: id, text: text)
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func addConnector(_ new: NewConnector) async {
        do {
            _ = try await client.addConnector(new)
            connectors = try await client.connectors()
        } catch {
            lastError = error.localizedDescription
        }
    }

    /// Starts the GitHub device flow (a new account, or `connectorID` again). The code to type appears in `githubFlow`.
    public func startGithubSignIn(connectorID: String? = nil) async {
        do {
            githubFlow = try await client.startGithubSignIn(connectorID: connectorID)
            lastError = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func cancelGithubSignIn() async {
        do {
            try await client.cancelGithubSignIn()
            githubFlow = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func removeConnector(_ id: String) async {
        do {
            try await client.removeConnector(id: id)
            connectors = try await client.connectors()
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func setDefaultConnector(_ id: String) async {
        do {
            connectors = try await client.setDefaultConnector(id: id)
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func signInConnector(_ id: String) async {
        // A GitHub sign-in is the device flow again; AWS and agents are checked (their login lives on the Mac).
        if let connector = connectors.first(where: { $0.id == id }), connector.kind == .github {
            await startGithubSignIn(connectorID: id)
            return
        }
        do {
            _ = try await client.signInConnector(id: id)
            connectors = try await client.connectors()
        } catch {
            lastError = error.localizedDescription
        }
    }

    // MARK: Connector forms
    // These throw, so a form can show the reason next to its field instead of in the window's error line.

    /// Adds a connector and reports why it could not be added.
    public func addConnectorChecked(_ new: NewConnector) async throws {
        _ = try await client.addConnector(new)
        connectors = try await client.connectors()
    }

    /// A pasted GitHub token: checked against GitHub by the daemon, filed in the keychain, never returned.
    public func addGithubToken(_ token: String, name: String? = nil) async throws {
        connectors = try await client.addGithubToken(token, name: name)
    }

    /// Rename; for AWS the region and the monthly cap too.
    public func updateConnector(_ id: String, _ update: ConnectorUpdate) async throws {
        connectors = try await client.updateConnector(id: id, update)
    }

    /// "Check connection": asks the provider and returns its one-line answer; the list shows the fresh status.
    public func checkConnector(_ id: String) async throws -> ConnectorCheck {
        let check = try await client.checkConnector(id: id)
        connectors = try await client.connectors()
        return check
    }

    /// "Refresh all": every connector is checked with its provider.
    public func refreshConnectors() async {
        do {
            connectors = try await client.refreshConnectors()
            lastError = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    public func endSandbox() async {
        do {
            receipt = try await client.endSandbox()
            sandbox = try await client.sandboxStatus()
        } catch {
            lastError = error.localizedDescription
        }
    }

    private func optimizationID(ofAgent agentID: String) -> String? {
        details.first { $0.value.agents.contains { $0.id == agentID } }?.key
    }

    // MARK: Cache

    private func restoreCache() {
        guard let url = cacheURL, let data = try? Data(contentsOf: url),
              let cached = try? JSONDecoder().decode(CachedState.self, from: data) else { return }
        overview = cached.overview
        optimizations = cached.optimizations
        connectors = cached.connectors
        templates = cached.templates
        benchmarks = cached.benchmarks
        repos = cached.repos
        sandbox = cached.sandbox
        receipt = cached.receipt
        range = cached.overview?.range ?? .days90
    }

    private func persist() {
        guard let url = cacheURL else { return }
        let state = CachedState(
            overview: overview, optimizations: optimizations, connectors: connectors, templates: templates,
            benchmarks: benchmarks, repos: repos, sandbox: sandbox, receipt: receipt)
        guard let data = try? JSONEncoder().encode(state) else { return }
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
    }
}
