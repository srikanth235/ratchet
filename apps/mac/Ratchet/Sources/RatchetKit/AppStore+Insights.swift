import Foundation

/// What the store keeps for the Result, live log and Benchmarks screens, apart from the main state: one entry per agent or optimization that was opened.
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
