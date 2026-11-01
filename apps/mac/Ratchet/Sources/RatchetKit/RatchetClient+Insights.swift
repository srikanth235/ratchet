import Foundation

/// The reads behind the live log, the Result details and the Benchmarks page. Separate from `RatchetClient` (which refines
/// it) so a client that has no answer, like a test double, compiles without them.
public protocol RatchetInsightsClient: Sendable {
    /// An agent's log. Without `cursor`: the newest `limit` lines (oldest first). With it: only lines newer than the cursor.
    func agentLog(agentID: String, after cursor: Int64?, limit: Int) async throws -> AgentLogPage
    /// Spend breakdown, "where the time went", before/after rows and the winning change of one optimization.
    func insights(optimizationID: String) async throws -> RunInsights
    /// The ratchet of one metric (`metric` is one of the series' options; nil is the objective).
    func benchmarkSeries(optimizationID: String, metric: String?) async throws -> BenchmarkSeries
    /// Every candidate with its verdict and bench results.
    func candidateResults(optimizationID: String) async throws -> [CandidateResult]
}

extension RatchetInsightsClient {
    public func agentLog(agentID: String, after cursor: Int64?, limit: Int) async throws -> AgentLogPage {
        throw RatchetClientError.unavailable("This client has no agent log.")
    }

    public func insights(optimizationID: String) async throws -> RunInsights {
        throw RatchetClientError.unavailable("This client has no result details.")
    }

    public func benchmarkSeries(optimizationID: String, metric: String?) async throws -> BenchmarkSeries {
        throw RatchetClientError.unavailable("This client has no benchmark series.")
    }

    public func candidateResults(optimizationID: String) async throws -> [CandidateResult] {
        throw RatchetClientError.unavailable("This client has no candidate results.")
    }
}

/// What the store keeps for the screens above, apart from the main state: one entry per agent or optimization that was opened.
public struct InsightsState: Sendable {
    public var logs: [String: AgentLog] = [:]
    public var insights: [String: RunInsights] = [:]
    /// Keyed by optimization id; the series of the metric last asked for.
    public var series: [String: BenchmarkSeries] = [:]
    public var results: [String: [CandidateResult]] = [:]

    public init() {}
}

extension AppStore {
    /// The newest page of an agent's log, or only what came after the cursor once there is one. Safe to call again at any time:
    /// the store skips lines it already shows.
    public func loadAgentLog(_ agentID: String) async {
        var log = insight.logs[agentID] ?? AgentLog()
        do {
            let page = try await client.agentLog(agentID: agentID, after: log.cursor, limit: 200)
            log.apply(page)
        } catch {
            log.fail(error.localizedDescription)
        }
        insight.logs[agentID] = log
    }

    /// An `agent-event` arrived: read the new lines, but only of a log somebody opened.
    func agentLogChanged(_ agentID: String) {
        guard insight.logs[agentID] != nil else { return }
        Task { await loadAgentLog(agentID) }
    }

    public func loadInsights(_ optimizationID: String) async {
        do {
            insight.insights[optimizationID] = try await client.insights(optimizationID: optimizationID)
        } catch {
            reportProblem(error.localizedDescription)
        }
    }

    /// The benchmark of one optimization: the series of `metric` (nil is the objective) and the candidates.
    public func loadBenchmark(_ optimizationID: String, metric: String? = nil) async {
        do {
            insight.series[optimizationID] = try await client.benchmarkSeries(optimizationID: optimizationID, metric: metric)
            insight.results[optimizationID] = try await client.candidateResults(optimizationID: optimizationID)
            if insight.insights[optimizationID] == nil { await loadInsights(optimizationID) }
        } catch {
            reportProblem(error.localizedDescription)
        }
    }
}
