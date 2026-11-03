import Foundation

// Overview, optimizations and their agents, benchmarks and templates of the sample world.

extension MockRatchetClient {
    // MARK: Overview and spend

    public func overview(range: TimeRange) async throws -> Overview {
        world.overview(range)
    }

    public func spend(range: TimeRange) async throws -> SpendSummary {
        let overview = world.overview(range)
        let aws = (overview.spentUSD * 0.66).rounded()
        return SpendSummary(
            range: range, totalUSD: overview.spentUSD, budgetUSD: overview.budgetUSD,
            lines: [
                SpendLine(id: "aws", label: "AWS sandboxes", usd: aws),
                SpendLine(id: "agents", label: "Agent keys", usd: overview.spentUSD - aws),
            ])
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

    public func runOptimization(id: String) async throws -> Optimization {
        guard var detail = world.details[id] else { throw RatchetClientError.notFound("optimization \(id)") }
        guard !detail.optimization.status.isFinished else { throw RatchetClientError.invalid("This optimization already ended.") }
        var started = detail.optimization
        started.status = .running
        if started.secondsLeft == nil { started.secondsLeft = started.timeLimitHours * 3600 }
        if started.startedAt == nil { started.startedAt = world.now }
        detail.optimization = started
        world.details[id] = detail
        if let at = world.optimizations.firstIndex(where: { $0.id == id }) { world.optimizations[at] = started }
        emit([.optimizationUpdated(started)])
        return started
    }

    public func stopOptimization(id: String) async throws {
        guard var detail = world.details[id] else { throw RatchetClientError.notFound("optimization \(id)") }
        guard !detail.optimization.status.isFinished else { throw RatchetClientError.invalid("This optimization already ended.") }
        var stopped = detail.optimization
        stopped.status = stopped.winsCount > 0 ? .done : .noWin
        stopped.secondsLeft = nil
        for index in detail.agents.indices {
            detail.agents[index].state = .idle
            detail.agents[index].activity = "Stopped by you"
        }
        detail.optimization = stopped
        world.details[id] = detail
        if let at = world.optimizations.firstIndex(where: { $0.id == id }) { world.optimizations[at] = stopped }
        emit([.optimizationUpdated(stopped)])
    }

    public func openPullRequest(optimizationID id: String) async throws -> URL {
        guard var detail = world.details[id] else { throw RatchetClientError.notFound("optimization \(id)") }
        var opt = detail.optimization
        guard opt.winsCount > 0 else { throw RatchetClientError.invalid("No verified win yet, so there is nothing to merge.") }
        let number = opt.prNumber ?? (300 + world.optimizations.count)
        let url = "https://github.com/\(opt.repo)/pull/\(number)"
        opt.prNumber = number
        opt.prURL = url
        detail.optimization = opt
        world.details[id] = detail
        if let at = world.optimizations.firstIndex(where: { $0.id == id }) { world.optimizations[at] = opt }
        emit([.optimizationUpdated(opt)])
        guard let parsed = URL(string: url) else { throw RatchetClientError.unavailable(url) }
        return parsed
    }

    public func report(optimizationID id: String) async throws -> OptimizationReport {
        guard let detail = world.details[id] else { throw RatchetClientError.notFound("optimization \(id)") }
        let opt = detail.optimization
        let slug = opt.title.lowercased().map { $0.isLetter || $0.isNumber ? String($0) : "-" }.joined()
        return OptimizationReport(
            fileName: "ratchet-\(slug).md",
            markdown: "# \(opt.title)\n\n\(opt.repo) · \(Plural.count(opt.winsCount, "verified win")).\n")
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

    func updateAgent(id: String, _ change: (inout Agent) -> Void) throws -> Agent {
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

    // MARK: Benchmarks and templates

    public func benchmarks() async throws -> [BenchmarkRun] { world.benchmarks }

    public func templates() async throws -> [Template] { world.templates }
}
