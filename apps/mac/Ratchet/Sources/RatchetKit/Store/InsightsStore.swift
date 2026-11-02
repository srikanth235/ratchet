import Foundation
import Observation

/// One agent's log. Its own object, so a new line re-renders the views that show this log and nothing else (the detail page
/// around it reads other objects).
@MainActor @Observable
public final class AgentLogModel {
    public internal(set) var log = AgentLog()
    /// One read in flight at a time: a second request while one runs asks for one more read after it, not a parallel one.
    @ObservationIgnored var loading = false
    @ObservationIgnored var readAgain = false

    public init() {}
}

/// What the Result details and the Benchmarks page show for one optimization.
@MainActor @Observable
public final class OptimizationInsights {
    public internal(set) var insights: RunInsights?
    /// The series of the metric last asked for.
    public internal(set) var series: BenchmarkSeries?
    public internal(set) var results: [CandidateResult]?

    public init() {}
}

/// The reads behind the live log, the Result details and the Benchmarks page. Hands out one observable per agent log and per
/// optimization; the models of what was opened longest ago are dropped past a limit.
@MainActor
public final class InsightsStore {
    /// How many logs and optimizations are kept.
    public static let logLimit = 24
    public static let optimizationLimit = 12

    private let client: any OptimizationsClient
    private let errors: StoreErrors
    private var logs: [String: AgentLogModel] = [:]
    private var logOrder: [String] = []
    private var optimizations: [String: OptimizationInsights] = [:]
    private var optimizationOrder: [String] = []

    public init(client: any OptimizationsClient, errors: StoreErrors) {
        self.client = client
        self.errors = errors
    }

    // MARK: Models

    /// The model of an agent's log, made when first asked for.
    public func logModel(_ agentID: String) -> AgentLogModel {
        if let model = logs[agentID] {
            touch(agentID, in: &logOrder)
            return model
        }
        let model = AgentLogModel()
        logs[agentID] = model
        logOrder.append(agentID)
        for victim in Eviction.victims(order: logOrder, keeping: [agentID], limit: Self.logLimit) {
            logs[victim] = nil
            logOrder.removeAll { $0 == victim }
        }
        return model
    }

    /// The log as it stands, or nil when nobody opened it.
    public func log(_ agentID: String) -> AgentLog? { logs[agentID]?.log }

    public func hasLog(_ agentID: String) -> Bool { logs[agentID] != nil }

    public func model(for optimizationID: String) -> OptimizationInsights {
        if let model = optimizations[optimizationID] {
            touch(optimizationID, in: &optimizationOrder)
            return model
        }
        let model = OptimizationInsights()
        optimizations[optimizationID] = model
        optimizationOrder.append(optimizationID)
        for victim in Eviction.victims(order: optimizationOrder, keeping: [optimizationID], limit: Self.optimizationLimit) {
            optimizations[victim] = nil
            optimizationOrder.removeAll { $0 == victim }
        }
        return model
    }

    public func insights(of optimizationID: String) -> RunInsights? { optimizations[optimizationID]?.insights }
    public func series(of optimizationID: String) -> BenchmarkSeries? { optimizations[optimizationID]?.series }
    public func results(of optimizationID: String) -> [CandidateResult]? { optimizations[optimizationID]?.results }

    private func touch(_ id: String, in order: inout [String]) {
        guard order.last != id else { return }
        order.removeAll { $0 == id }
        order.append(id)
    }

    /// Forgets everything (another data source).
    func reset() {
        logs = [:]
        logOrder = []
        optimizations = [:]
        optimizationOrder = []
    }

    // MARK: Loading

    /// The newest page of an agent's log, or only what came after the cursor once there is one. Safe to call again at any time:
    /// the log skips lines it already shows, and a call while a read is in flight asks for one more read after it.
    public func loadAgentLog(_ agentID: String) async {
        let model = logModel(agentID)
        if model.loading {
            model.readAgain = true
            return
        }
        model.loading = true
        defer { model.loading = false }
        repeat {
            model.readAgain = false
            do {
                // The cursor is read now, and the page is applied to the log as it is when the answer arrives.
                let page = try await client.agentLog(agentID: agentID, after: model.log.cursor, limit: 200)
                model.log.apply(page)
            } catch {
                model.log.fail(error.localizedDescription)
            }
        } while model.readAgain
    }

    /// An `agent-event` arrived: read the new lines, but only of a log somebody opened.
    func agentLogChanged(_ agentID: String) {
        guard logs[agentID] != nil else { return }
        Task { await loadAgentLog(agentID) }
    }

    public func loadInsights(_ optimizationID: String) async {
        do {
            let loaded = try await client.insights(optimizationID: optimizationID)
            model(for: optimizationID).insights = loaded
        } catch {
            errors.report(error.localizedDescription)
        }
    }

    /// The benchmark of one optimization: the series of `metric` (nil is the objective) and the candidates.
    public func loadBenchmark(_ optimizationID: String, metric: String? = nil) async {
        do {
            let series = try await client.benchmarkSeries(optimizationID: optimizationID, metric: metric)
            let results = try await client.candidateResults(optimizationID: optimizationID)
            let model = model(for: optimizationID)
            model.series = series
            model.results = results
            if model.insights == nil { await loadInsights(optimizationID) }
        } catch {
            errors.report(error.localizedDescription)
        }
    }

    /// The stream lost its place: read again what somebody has open.
    func reloadOpen() async {
        let agents = Array(logs.keys)
        let optimizationIDs = Array(optimizations.compactMap { $0.value.insights == nil ? nil : $0.key })
        for agentID in agents { await loadAgentLog(agentID) }
        for id in optimizationIDs { await loadInsights(id) }
    }
}

/// Which entries to drop when a cache is past its limit: the ones used longest ago, never the one just asked for.
enum Eviction {
    /// `order` runs from least to most recently used.
    static func victims(order: [String], keeping: Set<String>, limit: Int) -> [String] {
        guard order.count > limit else { return [] }
        var surplus = order.count - limit
        var found: [String] = []
        for id in order where surplus > 0 && !keeping.contains(id) {
            found.append(id)
            surplus -= 1
        }
        return found
    }
}
