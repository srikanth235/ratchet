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
    public var range: TimeRange = .days90
    /// True when the data is the built-in sample (no daemon is running): the sidebar says so, quietly.
    public let isDemoData: Bool

    private let cacheURL: URL?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    @ObservationIgnored private var pendingWins: [String: RatchetMove] = [:]

    public init(client: any RatchetClient, cacheURL: URL? = nil, isDemoData: Bool = false) {
        self.client = client
        self.cacheURL = cacheURL
        self.isDemoData = isDemoData
        restoreCache()
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

    /// Applies one server-sent event to what is on screen.
    public func apply(_ event: RatchetEvent) {
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
        case .needsYouChanged(let items):
            overview?.needsYou = items
        case .connectorsChanged(let list):
            connectors = list
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
        do {
            _ = try await client.signInConnector(id: id)
            connectors = try await client.connectors()
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
