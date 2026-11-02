import Foundation
import Observation

/// The parts of a refresh, each loaded and failing on its own: one endpoint that is down leaves the rest of the app showing
/// what it has.
public enum LoadSection: String, CaseIterable, Sendable {
    case overview, optimizations, connectors, templates, benchmarks, repos, sandbox
}

/// The app's state, as a root over the stores that own it: `optimizationStore`, `connectorStore`, `sandboxStore`, `insight`.
/// It renders the last snapshot at once (`restoreCache`), refreshes from the client, then applies the event stream through the
/// pure `EventReducer`. What belongs to no store is here: the overview, templates, benchmarks and repos, the time range, the
/// error line, and whether the live stream is up. It also decides which events deserve a notification (`NotificationRouting`).
@MainActor @Observable
public final class AppStore {
    public let client: any RatchetClient
    /// How a test, snapshot or the self-test drives a sample client (time, setup answers); nil with the real daemon.
    public let scenario: (any ClientScenario)?

    /// The error line of the window (see `StoreErrors`).
    public let errors: StoreErrors
    public let optimizationStore: OptimizationsStore
    public let connectorStore: ConnectorsStore
    public let sandboxStore: SandboxStore
    /// Agent logs, result details and benchmark series of what was opened.
    public let insight: InsightsStore

    public private(set) var overview: Overview?
    public private(set) var templates: [Template] = []
    public private(set) var benchmarks: [BenchmarkRun] = []
    public private(set) var repos: [Repo] = []
    public private(set) var isLoaded = false
    /// Which parts of the last refresh failed, with the reason; empty when all worked.
    public private(set) var sectionErrors: [LoadSection: String] = [:]
    /// Whether the live connection to the daemon is up. Not part of the cache.
    public private(set) var streamState: StreamState = .connecting
    public var range: TimeRange = .days90
    /// True when the data is the sample or the simulation (demo mode): the sidebar says so, quietly.
    public private(set) var isDemoData: Bool

    /// The last thing that failed, shown in the window's banner until it is dismissed or the next action starts.
    public var lastError: String? { errors.message }

    /// Called for each event worth a notification (a win to merge, a run that finished...). The app decides whether and
    /// how to show it; the store only decides that it happened.
    @ObservationIgnored public var onNotification: (@MainActor (NotificationRequest) -> Void)?
    /// Which categories the user has switched on.
    @ObservationIgnored public var enabledNotifications: @MainActor () -> Set<NotificationCategory> = { Set(NotificationCategory.allCases) }

    private let cache: SnapshotCache
    @ObservationIgnored private var pendingWins: [String: RatchetMove] = [:]
    @ObservationIgnored private var resyncTask: Task<Void, Never>?
    @ObservationIgnored private var resyncAgain = false

    public init(client: any RatchetClient, scenario: (any ClientScenario)? = nil, cacheURL: URL? = nil, isDemoData: Bool = false) {
        let errors = StoreErrors()
        self.client = client
        self.scenario = scenario
        self.errors = errors
        self.optimizationStore = OptimizationsStore(client: client, errors: errors)
        self.connectorStore = ConnectorsStore(client: client, errors: errors)
        self.sandboxStore = SandboxStore(client: client, errors: errors)
        self.insight = InsightsStore(client: client, errors: errors)
        self.cache = SnapshotCache(url: cacheURL)
        self.isDemoData = isDemoData
        // What the person does in a store reaches the rest through the reducer.
        optimizationStore.onEvent = { [weak self] event in self?.apply(event) }
        optimizationStore.onChange = { [weak self] optimization in self?.show(optimization) }
    }

    /// Switches between the real daemon and the demo daemon (same socket): forgets what is shown, then reloads.
    public func useSource(isDemoData: Bool, cacheURL: URL?) async {
        self.isDemoData = isDemoData
        await cache.use(cacheURL)
        overview = nil
        templates = []
        benchmarks = []
        repos = []
        optimizationStore.reset()
        connectorStore.reset()
        sandboxStore.reset()
        insight.reset()
        pendingWins = [:]
        sectionErrors = [:]
        streamState = .connecting
        isLoaded = false
        await restoreCache()
        await refresh()
    }

    // MARK: Loading

    /// Shows the last snapshot, then loads everything. The events are `followEvents()`'s, which the window runs in its own task.
    public func start() async {
        await restoreCache()
        await refresh()
    }

    /// Reloads every part of the app, two waves of five requests at a time. A part that fails keeps what it had and is
    /// recorded in `sectionErrors`; the rest still loads, and the app is loaded as soon as any part is.
    public func refresh() async {
        let client = self.client
        let range = self.range

        async let overviewResult = Self.attempt { try await client.overview(range: range) }
        async let optimizationsResult = Self.attempt { try await client.optimizations() }
        async let connectorsResult = Self.attempt { try await client.connectors() }
        async let flowResult = Self.attempt { try await client.githubSignInFlow() }
        async let setupResult = Self.attempt { try await client.connectorSetup() }
        let (overviewNow, optimizationsNow, connectorsNow, flowNow, setupNow) = await (
            overviewResult, optimizationsResult, connectorsResult, flowResult, setupResult)

        async let templatesResult = Self.attempt { try await client.templates() }
        async let benchmarksResult = Self.attempt { try await client.benchmarks() }
        async let reposResult = Self.attempt { try await client.repos() }
        async let sandboxResult = Self.attempt { try await client.sandboxStatus() }
        async let receiptResult = Self.attempt { try await client.sandboxReceipt() }
        let (templatesNow, benchmarksNow, reposNow, sandboxNow, receiptNow) = await (
            templatesResult, benchmarksResult, reposResult, sandboxResult, receiptResult)

        var failures: [LoadSection: String] = [:]
        if let value = Self.take(overviewNow, .overview, &failures) { overview = value }
        if let value = Self.take(optimizationsNow, .optimizations, &failures) { optimizationStore.optimizations = value }
        if let value = Self.take(connectorsNow, .connectors, &failures) { connectorStore.connectors = value }
        if let value = Self.take(flowNow, .connectors, &failures) { connectorStore.githubFlow = value }
        if let value = try? setupNow.get() { connectorStore.connectorSetup = value }
        if let value = Self.take(templatesNow, .templates, &failures) { templates = value }
        if let value = Self.take(benchmarksNow, .benchmarks, &failures) { benchmarks = value }
        if let value = Self.take(reposNow, .repos, &failures) { repos = value }
        if let value = Self.take(sandboxNow, .sandbox, &failures) { sandboxStore.sandbox = value }
        if let value = Self.take(receiptNow, .sandbox, &failures) { sandboxStore.receipt = value }

        sectionErrors = failures
        if failures.count < LoadSection.allCases.count { isLoaded = true }
        if let first = LoadSection.allCases.compactMap({ failures[$0] }).first {
            errors.failLoading(first)
        } else {
            errors.loadSucceeded()
        }
        await persist()
    }

    /// Runs `body` and keeps its failure, so a group of requests can run side by side and each report on its own.
    private nonisolated static func attempt<T: Sendable>(_ body: @Sendable () async throws -> T) async -> Result<T, any Error> {
        do {
            return .success(try await body())
        } catch {
            return .failure(error)
        }
    }

    private static func take<T>(_ result: Result<T, any Error>, _ section: LoadSection, _ failures: inout [LoadSection: String]) -> T? {
        switch result {
        case .success(let value): return value
        case .failure(let error):
            if failures[section] == nil { failures[section] = error.localizedDescription }
            return nil
        }
    }

    /// Follows the live event stream until the calling task is cancelled: give it to the window's `.task(id:)`, so it stops when
    /// the daemon goes away or the data source changes, and starts again with them.
    public func followEvents() async {
        streamState = .connecting
        let events = await client.events()
        for await event in events {
            if Task.isCancelled { break }
            apply(event)
        }
    }

    public func setRange(_ newRange: TimeRange) async {
        range = newRange
        do {
            overview = try await client.overview(range: newRange)
        } catch {
            errors.failLoading(error)
        }
    }

    // MARK: Derived

    public var needsYou: [NeedsYouItem] { overview?.needsYou ?? [] }

    public var needsYouCount: Int { needsYou.count }

    // MARK: Events

    private var liveState: LiveState {
        LiveState(
            overview: overview, optimizations: optimizationStore.optimizations, details: optimizationStore.details,
            connectors: connectorStore.connectors, githubFlow: connectorStore.githubFlow, sandbox: sandboxStore.sandbox,
            receipt: sandboxStore.receipt, pendingWins: pendingWins)
    }

    /// Applies one server-sent event to what is on screen, and reports what is new that deserves a notification.
    public func apply(_ event: RatchetEvent) {
        let announcing = onNotification != nil && isLoaded
        let result = EventReducer.reduce(
            liveState, event, isLoaded: announcing, enabled: announcing ? enabledNotifications() : [])
        write(result)
        if let agentID = result.logAppended { insight.agentLogChanged(agentID) }
        if let state = result.streamState { streamState = state }
        if result.resync { requestResync() }
        if let onNotification {
            for request in result.notifications { onNotification(request) }
        }
    }

    /// Stores only the parts the reduction changed, so views of the others do not re-render.
    private func write(_ result: EventReduction) {
        let state = result.state
        pendingWins = state.pendingWins
        for part in result.touched {
            switch part {
            case .overview: overview = state.overview
            case .optimizations: optimizationStore.optimizations = state.optimizations
            case .details: optimizationStore.details = state.details
            case .connectors: connectorStore.connectors = state.connectors
            case .githubFlow: connectorStore.githubFlow = state.githubFlow
            case .sandbox: sandboxStore.sandbox = state.sandbox
            case .receipt: sandboxStore.receipt = state.receipt
            }
        }
    }

    /// An optimization the person just changed (stopped, opened a pull request): on screen at once, announced to nobody.
    private func show(_ optimization: Optimization) {
        write(EventReducer.register(optimization, in: liveState))
    }

    // MARK: Resync

    /// The event stream lost its place: reload the lists and everything open. One reload runs at a time; a request that comes
    /// while it runs makes it go once more when it ends, instead of stacking.
    private func requestResync() {
        if resyncTask != nil {
            resyncAgain = true
            return
        }
        resyncTask = Task { [weak self] in
            guard let self else { return }
            repeat {
                self.resyncAgain = false
                await self.refresh()
                await self.optimizationStore.reloadDetails()
                await self.insight.reloadOpen()
            } while self.resyncAgain && !Task.isCancelled
            self.resyncTask = nil
        }
    }

    // MARK: Actions

    /// Puts a just-started optimization on screen without waiting for its event.
    public func register(_ optimization: Optimization) async {
        show(optimization)
        await optimizationStore.loadDetail(id: optimization.id)
        await sandboxStore.refreshStatus()
        await persist()
    }

    /// Something the views did (saving a file) failed: shown like any other error.
    public func reportProblem(_ text: String) { errors.report(text) }

    /// The banner's close button.
    public func dismissError() { errors.clear() }

    // MARK: Cache

    /// Shows the last snapshot (read and decoded off the main actor), so the window has something before the daemon answers.
    public func restoreCache() async {
        guard let cached = await cache.load() else { return }
        overview = cached.overview
        optimizationStore.optimizations = cached.optimizations
        connectorStore.connectors = cached.connectors
        templates = cached.templates
        benchmarks = cached.benchmarks
        repos = cached.repos
        sandboxStore.sandbox = cached.sandbox
        sandboxStore.receipt = cached.receipt
        range = cached.overview?.range ?? .days90
    }

    /// Hands the current state to the cache, which encodes and writes it later, off the main actor.
    private func persist() async {
        let state = CachedState(
            overview: overview, optimizations: optimizationStore.optimizations, connectors: connectorStore.connectors,
            templates: templates, benchmarks: benchmarks, repos: repos, sandbox: sandboxStore.sandbox,
            receipt: sandboxStore.receipt)
        await cache.save(state)
    }

    /// Writes what the cache holds now (quitting, tests).
    public func flushCache() async { await cache.flush() }
}
