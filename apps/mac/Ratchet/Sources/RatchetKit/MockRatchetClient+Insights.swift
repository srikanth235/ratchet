import Foundation

// Sample answers for the live log, the Result details and the Benchmarks page. Deterministic: a fixed `now` gives the same
// numbers every time, so the snapshots and the tests read the same.

extension MockWorld {
    /// Gives every agent of the sample world a short log that matches what it is doing.
    mutating func seedLogs() {
        for detail in details.values {
            let start = detail.optimization.startedAt ?? now
            for (offset, agent) in detail.agents.enumerated() {
                let wins = detail.candidates.filter { $0.agent == agent.name }
                var lines: [AgentLogLine] = []
                func add(_ minutes: Double, _ kind: String, _ text: String) {
                    lines.append(AgentLogLine(
                        id: Int64(lines.count + 1), at: start.addingTimeInterval(minutes * 60 + Double(offset)), kind: kind, text: text))
                }
                add(0, "status", "online · reading the repo")
                add(3, "message", "The goal is " + detail.optimization.goal.lowercased() + ". Looking for the hot path first.")
                for (index, candidate) in wins.reversed().enumerated() {
                    let base = 8 + Double(index) * 14
                    add(base, "message", "Trying: " + candidate.title)
                    add(base + 4, "tool", "git commit -m \"" + candidate.title + "\" (" + candidate.id + ")")
                    switch candidate.verdict {
                    case .verified: add(base + 9, "verdict", "verified: " + candidate.id + (candidate.value.map { " at " + detail.optimization.metric.text($0) } ?? ""))
                    case .noise: add(base + 9, "verdict", "within noise: " + candidate.id + ", not a win")
                    case .blocked: add(base + 9, "verdict", "blocked: protected path changed in " + candidate.id)
                    case .evaluating: add(base + 6, "status", "waiting for the trusted evaluator")
                    }
                }
                if agent.state == .working { add(8 + Double(wins.count) * 14, "message", agent.activity) }
                agentLogs[agent.id] = lines
            }
        }
    }
}

enum MockSamples {
    /// A right-skewed bell of `count` values around `mean`, from a fixed seed (the sum of three uniform draws, stretched on
    /// the slow side as latencies are).
    static func samples(mean: Double, spread: Double, count: Int, seed: UInt64) -> [Double] {
        var state = seed &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        func next() -> Double {
            state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            return Double(state >> 11) / Double(1 << 53)
        }
        return (0..<count).map { _ in
            let bell = (next() + next() + next()) / 3 - 0.5
            let skew = bell > 0 ? 1.4 : 0.8
            return mean + bell * 2 * spread * skew
        }
    }
}

extension MockRatchetClient {
    public func agentLog(agentID: String, after cursor: Int64?, limit: Int) async throws -> AgentLogPage {
        guard world.details.values.contains(where: { $0.agents.contains { $0.id == agentID } }) else {
            throw RatchetClientError.notFound("agent \(agentID)")
        }
        let all = world.agentLogs[agentID] ?? []
        guard let cursor else {
            let page = Array(all.suffix(max(limit, 1)))
            return AgentLogPage(lines: page, cursor: page.last?.id ?? 0)
        }
        let newer = Array(all.filter { $0.id > cursor }.prefix(max(limit, 1)))
        return AgentLogPage(lines: newer, cursor: newer.last?.id ?? cursor)
    }

    /// Called from the sample simulation when an agent starts or finishes a candidate.
    func appendLog(
        agent: Agent, evalsBy: Int, winsBy: Int, activity: String?, events: inout [RatchetEvent], id: String
    ) {
        let text: String
        let kind: String
        if let activity {
            kind = "message"
            text = "Trying: " + activity
        } else if winsBy > 0 {
            kind = "verdict"
            text = "verified: a new best"
        } else if evalsBy > 0 {
            kind = "verdict"
            text = "not a win"
        } else {
            return
        }
        var log = world.agentLogs[agent.id] ?? []
        log.append(AgentLogLine(id: Int64((log.last?.id ?? 0) + 1), at: world.now, kind: kind, text: text))
        world.agentLogs[agent.id] = log
        events.append(.agentLogAppended(optimizationID: id, agentID: agent.id))
    }

    public func insights(optimizationID: String) async throws -> RunInsights {
        guard let detail = world.details[optimizationID] else { throw RatchetClientError.notFound("optimization \(optimizationID)") }
        let optimization = detail.optimization
        let hasWin = optimization.best != nil
        let delta = optimization.change.map { $0 * 100 }
        let objective = MetricChoice(
            key: optimization.metric.name, label: optimization.metric.name, unit: optimization.metric.unit, isObjective: true)
        var metrics = [objective]
        var compare = [
            CompareLine(
                key: objective.key, label: objective.label, unit: objective.unit, baseline: optimization.baseline,
                best: optimization.best, deltaPercent: delta, lowerIsBetter: optimization.metric.lowerIsBetter),
        ]
        if optimizationID == "opt-checkout" {
            metrics = ["p50", "p95", "p99"].map { MetricChoice(key: $0, label: $0, unit: "ms", isObjective: $0 == "p95") }
            compare = [
                CompareLine(key: "p50", label: "p50", unit: "ms", baseline: 198, best: 141, deltaPercent: -28.8, lowerIsBetter: true),
                CompareLine(key: "p95", label: "p95", unit: "ms", baseline: 412, best: 288, deltaPercent: -30.1, lowerIsBetter: true),
                CompareLine(key: "p99", label: "p99", unit: "ms", baseline: 655, best: 455, deltaPercent: -30.5, lowerIsBetter: true),
                CompareLine(key: "error_rate", label: "error rate", unit: "%", baseline: 0.12, best: 0.11, deltaPercent: -8.3, lowerIsBetter: true),
                CompareLine(key: "peak_rss_mb", label: "peak rss", unit: "MB", baseline: 512, best: 538, deltaPercent: 5.1, lowerIsBetter: true),
            ]
        } else if hasWin {
            compare.append(CompareLine(
                key: "error_rate", label: "error rate", unit: "%", baseline: 0.1, best: 0.1, deltaPercent: 0, lowerIsBetter: true))
        } else {
            compare = []
        }
        let spans: [SpanTiming]? = hasWin && optimization.metric.unit == "ms"
            ? [
                SpanTiming(name: "parse", baselineMS: 38, bestMS: 31),
                SpanTiming(name: "db.query", baselineMS: 214, bestMS: 118),
                SpanTiming(name: "serialize", baselineMS: 96, bestMS: 79),
                SpanTiming(name: "wait", baselineMS: 64, bestMS: 60),
            ]
            : nil
        let baselineMean = optimization.baseline
        return RunInsights(
            compare: compare, spans: spans,
            baselineSamples: MockSamples.samples(mean: baselineMean, spread: baselineMean * 0.12, count: 60, seed: 7),
            metrics: metrics, calibration: "This metric is noisy (\(Percent.noise(optimization.noise ?? 0.02))): wins must beat it.",
            spend: mockSpend(detail), change: mockChange(detail))
    }

    private func mockSpend(_ detail: OptimizationDetail) -> SpendBreakdown {
        let total = detail.optimization.spentUSD
        let shares: [(String, String, Double)] = [
            ("tokens", "Model tokens", 0.46), ("bench_runner", "Bench runner", 0.24), ("agent_runner", "Agent machines", 0.18),
            ("experiment", "Experiments", 0.09), ("storage_egress", "Storage and transfer", 0.03),
        ]
        let categories = shares.map { SpendLine(id: $0.0, label: $0.1, usd: (total * $0.2 * 100).rounded() / 100) }
        // A finished run lists no agents; the candidates tell who worked on it.
        var sources = detail.agents.map { (id: $0.id, name: $0.name, evals: $0.evals, wins: $0.wins) }
        if sources.isEmpty {
            var names: [String] = []
            for candidate in detail.candidates where !names.contains(candidate.agent) { names.append(candidate.agent) }
            sources = names.map { name in
                let own = detail.candidates.filter { $0.agent == name }
                return (id: "agent-" + name, name: name, evals: own.count, wins: own.filter { $0.verdict == .verified }.count)
            }
        }
        var agents: [AgentSpend] = []
        let agentShare = total * 0.74
        let weights = sources.map { Double(max($0.evals, 1)) }
        let weightSum = max(weights.reduce(0, +), 1)
        for (index, source) in sources.enumerated() {
            agents.append(AgentSpend(
                id: source.id, agentID: source.id, name: source.name, usd: (agentShare * weights[index] / weightSum * 100).rounded() / 100,
                wins: source.wins))
        }
        if total > 0 {
            agents.append(AgentSpend(id: "shared", agentID: nil, name: "Shared infrastructure", usd: (total * 0.26 * 100).rounded() / 100, wins: 0))
        }
        return SpendBreakdown(
            totalUSD: total, estimatedUSD: detail.optimization.status.isFinished ? 0 : (total * 0.2 * 100).rounded() / 100,
            budgetUSD: detail.optimization.budgetUSD ?? 0, categories: categories, agents: agents, wins: detail.optimization.winsCount)
    }

    private func mockChange(_ detail: OptimizationDetail) -> WinningChange {
        let optimization = detail.optimization
        guard optimization.winsCount > 0 else {
            return WinningChange(commits: 0, sha: nil, baseRef: optimization.branch, unavailable: "No verified win yet.")
        }
        let files = [
            DiffFile(path: "src/db/pool.ts", additions: 22, deletions: 6, binary: false),
            DiffFile(path: "src/cart/prices.ts", additions: 31, deletions: 9, binary: false),
        ]
        let excerpt = [
            DiffLine(kind: .hunk, text: "@@ -14,9 +14,12 @@ export function createPool(config: Config) {"),
            DiffLine(kind: .ctx, text: "const pool = new Pool({"),
            DiffLine(kind: .del, text: "  max: 20,"),
            DiffLine(kind: .add, text: "  max: 48,"),
            DiffLine(kind: .add, text: "  idleTimeoutMillis: 30_000,"),
            DiffLine(kind: .ctx, text: "  connectionString: config.url,"),
            DiffLine(kind: .del, text: "  keepAlive: false,"),
            DiffLine(kind: .add, text: "  keepAlive: true,"),
            DiffLine(kind: .ctx, text: "});"),
        ]
        return WinningChange(
            commits: optimization.winsCount, sha: detail.candidates.first { $0.verdict == .verified }?.id ?? "a91f3c2",
            baseRef: optimization.branch, files: files, additions: 53, deletions: 15, excerptPath: "src/db/pool.ts", excerpt: excerpt)
    }

    public func benchmarkSeries(optimizationID: String, metric: String?) async throws -> BenchmarkSeries {
        guard let detail = world.details[optimizationID] else { throw RatchetClientError.notFound("optimization \(optimizationID)") }
        let optimization = detail.optimization
        let family = try await insights(optimizationID: optimizationID).metrics
        let objectiveKey = family.first { $0.isObjective }?.key ?? optimization.metric.name
        let key = metric ?? objectiveKey
        guard let choice = family.first(where: { $0.key == key }) else {
            throw RatchetClientError.invalid("This evaluator does not report \(key).")
        }
        // Another metric of the family is the objective scaled (p99 runs about 1.58 times p95).
        let scale = choice.isObjective ? 1 : (key == "p50" ? 0.48 : 1.58)
        let start = optimization.startedAt ?? world.now
        var steps = [BenchmarkSeries.Step(minutes: 0, value: optimization.baseline * scale)]
        for move in detail.moves.sorted(by: { $0.at < $1.at }) {
            steps.append(BenchmarkSeries.Step(minutes: move.at.timeIntervalSince(start) / 60, value: move.value * scale))
        }
        let measured = detail.candidates.compactMap { candidate in candidate.value.map { (candidate, $0) } }
        let points = measured.sorted { $0.0.at < $1.0.at }.enumerated().map { index, pair in
            BenchmarkSeries.Point(
                id: index, minutes: pair.0.at.timeIntervalSince(start) / 60, value: pair.1 * scale, agent: pair.0.agent,
                verified: pair.0.verdict == .verified, label: pair.0.id + " · " + pair.0.title)
        }
        let last = steps.last?.minutes ?? 0
        return BenchmarkSeries(
            metric: key, options: family, unit: choice.unit, lowerIsBetter: optimization.metric.lowerIsBetter,
            baseline: optimization.baseline * scale, steps: steps, points: points,
            noiseFloor: choice.isObjective ? optimization.baseline * (optimization.noise ?? 0.02) : nil,
            tMax: max(last, optimization.timeLimitHours * 60 * 0.5))
    }

    public func candidateResults(optimizationID: String) async throws -> [CandidateResult] {
        guard let detail = world.details[optimizationID] else { throw RatchetClientError.notFound("optimization \(optimizationID)") }
        let baseline = detail.optimization.baseline
        let lower = detail.optimization.metric.lowerIsBetter
        let moves = Set(detail.moves.map(\.title))
        return detail.candidates.enumerated().map { index, candidate in
            let result: CandidateResult.Result
            var reasons: [String] = []
            switch candidate.verdict {
            case .verified: result = .verified
            case .evaluating: result = .pending
            case .blocked:
                result = .blocked
                reasons = ["protected path changed: infra/audit.ts"]
            case .noise:
                result = index % 3 == 1 ? .failedConfirm : .rejected
                reasons = result == .failedConfirm ? ["Failed confirm on the hidden trace"] : ["Gain is within the noise floor"]
            }
            let versus = candidate.value.map { value in (lower ? baseline - value : value - baseline) / baseline * 100 }
            return CandidateResult(
                id: candidate.id, sha: candidate.id, agent: candidate.agent, message: candidate.title, result: result,
                reasons: reasons, benchValue: candidate.value, confirmValue: result == .verified ? candidate.value.map { $0 * 1.01 } : nil,
                vsBaselinePercent: versus, movedRatchet: moves.contains(candidate.title),
                samples: candidate.value.map { MockSamples.samples(mean: $0, spread: $0 * 0.1, count: 60, seed: UInt64(10 + index)) } ?? [],
                at: candidate.at)
        }
    }
}
