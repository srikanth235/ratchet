import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

// Overview, optimizations and their agents, benchmarks and templates.

extension DaemonRatchetClient {
    // The generated client holds only the operations the app uses (tools/api/openapi-generator-config.yaml); these two
    // routes are called through the same transport and error middleware without generated code, so the committed client
    // stays what the generator produces. Adding them to the filter and regenerating (on a Mac) replaces this.
    private struct PullRequestPayload: Decodable { var url: String }
    private struct ReportPayload: Decodable {
        var fileName: String
        var markdown: String
    }

    public func stopOptimization(id: String) async throws {
        try await wire.calling {
            _ = try await api.stopOptimization(path: .init(id: id), body: .json(.init())).noContent
            await cache.forget()
        }
    }

    public func openPullRequest(optimizationID: String) async throws -> URL {
        try await wire.calling {
            let data = try await wire.rawCall(.post, "/v1/optimizations/\(wire.escaped(optimizationID))/pull-request", operationID: "openPullRequest")
            let pull = try JSONDecoder().decode(PullRequestPayload.self, from: data)
            await cache.forget()
            guard let url = URL(string: pull.url) else {
                throw RatchetClientError.unavailable("The pull request was opened, but its address is not readable: \(pull.url)")
            }
            return url
        }
    }

    public func report(optimizationID: String) async throws -> OptimizationReport {
        try await wire.calling {
            let data = try await wire.rawCall(.get, "/v1/optimizations/\(wire.escaped(optimizationID))/report", operationID: "getReport")
            let report = try JSONDecoder().decode(ReportPayload.self, from: data)
            return OptimizationReport(fileName: report.fileName, markdown: report.markdown)
        }
    }

    // MARK: Overview and spend

    public func overview(range: TimeRange) async throws -> Overview {
        try await wire.calling {
            DaemonMapping.overview(try await api.getOverview(query: .init(range: DaemonMapping.range(range))).ok.body.json)
        }
    }

    public func spend(range: TimeRange) async throws -> SpendSummary {
        try await wire.calling {
            let list = try await optimizationSnapshot()
            let lines = list.filter { $0.spendUsd > 0 }.map { SpendLine(id: $0.id, label: $0.name, usd: $0.spendUsd) }
            return SpendSummary(
                range: range, totalUSD: list.reduce(0) { $0 + $1.spendUsd },
                budgetUSD: list.reduce(0) { $0 + $1.budgetUsd }, lines: lines)
        }
    }

    // MARK: Optimizations

    public func optimizations() async throws -> [Optimization] {
        try await wire.calling { try await optimizationRows().map(\.optimization) }
    }

    public func optimization(id: String) async throws -> OptimizationDetail {
        try await wire.calling {
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
        try await wire.calling {
            let list = try await api.listCandidates(path: .init(id: optimizationID)).ok.body.json
            return list.map(DaemonMapping.candidate).sorted { $0.at > $1.at }
        }
    }

    public func ratchetHistory(optimizationID: String) async throws -> [RatchetMove] {
        try await wire.calling {
            let start = try await fetchOptimization(optimizationID).startedAt
            return try await ratchetMoves(optimizationID: optimizationID, start: start)
        }
    }

    func ratchetMoves(optimizationID: String, start: Date?) async throws -> [RatchetMove] {
        let series = try await api.getRatchetHistory(path: .init(id: optimizationID)).ok.body.json
        return DaemonMapping.moves(series, optimizationID: optimizationID, start: start ?? Date())
    }

    public func startOptimization(_ request: StartRequest) async throws -> Optimization {
        try await wire.calling {
            let demo = request.templateID.flatMap { $0.hasPrefix("demo-") ? VM.DemoId(rawValue: $0) : nil }
            var kinds: [VM.AgentKind] = []
            if let list = try? await api.listConnectors().ok.body.json.connectors {
                for id in request.selection.agents.keys.sorted() {
                    if case .case3(let agent)? = list.first(where: { DaemonMapping.connectorID($0) == id }) {
                        let kind = VM.AgentKind(rawValue: agent.agent.rawValue) ?? .claudeCode
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
            await cache.forget()
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
        try await wire.calling { try await optimizationDetail(optimizationID).agents.map(DaemonMapping.agent) }
    }

    public func pauseAgent(id: String) async throws -> Agent {
        try await wire.calling {
            _ = try await api.pauseAgent(path: .init(id: id)).noContent
            return try await refreshedAgent(id)
        }
    }

    public func resumeAgent(id: String) async throws -> Agent {
        try await wire.calling {
            _ = try await api.resumeAgent(path: .init(id: id)).noContent
            return try await refreshedAgent(id)
        }
    }

    public func messageAgent(id: String, text: String) async throws {
        try await wire.calling {
            _ = try await api.messageAgent(path: .init(id: id), body: .json(.init(text: text))).noContent
        }
    }

    /// The agent as the daemon reports it now. The daemon has no "agent by id" route, so it is found through the
    /// optimization it belongs to (remembered from the details loaded so far, else looked for among the live ones).
    private func refreshedAgent(_ id: String) async throws -> Agent {
        var owner = await cache.owner(ofAgent: id)
        if owner == nil {
            for vm in try await optimizationSnapshot() where vm.status == .running || vm.status == .baselining {
                _ = try await optimizationDetail(vm.id)
            }
            owner = await cache.owner(ofAgent: id)
        }
        guard let owner else { throw RatchetClientError.notFound("agent \(id)") }
        let detail = try await optimizationDetail(owner)
        guard let agent = detail.agents.first(where: { $0.id == id }) else { throw RatchetClientError.notFound("agent \(id)") }
        return DaemonMapping.agent(agent)
    }

    // MARK: Benchmarks and templates

    public func benchmarks() async throws -> [BenchmarkRun] {
        try await wire.calling {
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
        try await wire.calling { try await api.listTemplates().ok.body.json.map(DaemonMapping.template) }
    }
}
