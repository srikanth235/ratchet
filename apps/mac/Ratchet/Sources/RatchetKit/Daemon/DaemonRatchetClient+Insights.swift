import Foundation
import RatchetAPI

extension DaemonRatchetClient {
    public func agentLog(agentID: String, after cursor: Int64?, limit: Int) async throws -> AgentLogPage {
        try await calling {
            var path = "/v1/agents/\(escaped(agentID))/events?limit=\(min(max(limit, 1), 1000))"
            if let cursor { path += "&cursor=\(cursor)" }
            return try DaemonMapping.agentLogPage(try await rawCall(.get, path, operationID: "listAgentEvents"))
        }
    }

    public func insights(optimizationID: String) async throws -> RunInsights {
        try await calling {
            let detail = try await optimizationDetail(optimizationID)
            var wins: [String: Int] = [:]
            for agent in detail.agents { wins[agent.id] = Int(agent.stats.verified) }
            let totalWins = Int(detail.summary.verifiedWins)
            let spendData = try? await rawCall(.get, "/v1/optimizations/\(escaped(optimizationID))/spend", operationID: "getSpend")
            let spend = spendData.flatMap { try? DaemonMapping.spendBreakdown($0, wins: wins, totalWins: totalWins) }
            let change = (try? await api.getWinningChange(path: .init(id: optimizationID)).ok.body.json).map(DaemonMapping.winningChange)
            return DaemonMapping.insights(detail, spend: spend, change: change)
        }
    }

    public func benchmarkSeries(optimizationID: String, metric: String?) async throws -> BenchmarkSeries {
        try await calling {
            let series = try await api.getRatchetHistory(path: .init(id: optimizationID), query: .init(metric: metric)).ok.body.json
            return DaemonMapping.benchmarkSeries(series)
        }
    }

    public func candidateResults(optimizationID: String) async throws -> [CandidateResult] {
        try await calling {
            try await api.listCandidates(path: .init(id: optimizationID)).ok.body.json.map(DaemonMapping.candidateResult)
        }
    }
}
