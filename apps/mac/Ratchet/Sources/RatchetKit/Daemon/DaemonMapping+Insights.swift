import Foundation
import RatchetAPI

// The daemon's answers for the live log, the Result details and the Benchmarks page, as the app's models. Pure functions.
// The agent log and the spend breakdown are read without generated code (the generated client holds only the operations the
// app called before), so their JSON is decoded here with small payload types.

/// `GET /v1/agents/{id}/events`.
struct AgentEventsPayload: Decodable {
    struct Event: Decodable {
        var id: Int64
        var at: Int64
        var type: String
        var text: String
    }

    var events: [Event]
    var cursor: Int64
}

/// `GET /v1/optimizations/{id}/spend`.
struct SpendPayload: Decodable {
    struct Line: Decodable {
        var id: String
        var label: String
        var usd: Double
    }

    struct ByAgent: Decodable {
        var agentId: String?
        var kind: String?
        var branch: String?
        var usd: Double
    }

    var total: Double
    var estimated: Double
    var budgetUsd: Double
    var byCategory: [Line]
    var byAgent: [ByAgent]
}

extension DaemonMapping {
    // MARK: Agent log

    static func agentLogPage(_ data: Data) throws -> AgentLogPage {
        let payload = try JSONDecoder().decode(AgentEventsPayload.self, from: data)
        return AgentLogPage(
            lines: payload.events.map { AgentLogLine(id: $0.id, at: date(ms: $0.at), kind: $0.type, text: $0.text) },
            cursor: payload.cursor)
    }

    // MARK: Spend

    /// `wins` are the verified wins per agent id (from the optimization's agents); `totalWins` is the run's.
    static func spendBreakdown(_ data: Data, wins: [String: Int], totalWins: Int) throws -> SpendBreakdown {
        let payload = try JSONDecoder().decode(SpendPayload.self, from: data)
        var agents: [AgentSpend] = payload.byAgent.enumerated().map { index, row in
            let kind = row.kind.flatMap { VM.AgentKind(rawValue: $0) }
            let name = kind.map(title(of:)) ?? "Shared infrastructure"
            return AgentSpend(
                id: row.agentId ?? "shared-\(index)", agentID: row.agentId, name: name, usd: row.usd,
                wins: row.agentId.flatMap { wins[$0] } ?? 0)
        }
        // Two agents of one kind would read the same: tell them apart by their branch.
        let names = agents.map(\.name)
        for index in agents.indices where !agents[index].isShared && names.filter({ $0 == names[index] }).count > 1 {
            let branch = payload.byAgent[index].branch ?? String(agents[index].id.suffix(4))
            agents[index].name += " · " + branch
        }
        return SpendBreakdown(
            totalUSD: payload.total, estimatedUSD: payload.estimated, budgetUSD: payload.budgetUsd,
            categories: payload.byCategory.map { SpendLine(id: $0.id, label: $0.label, usd: $0.usd) },
            agents: agents, wins: totalWins)
    }

    // MARK: Detail

    static func metricChoice(_ vm: VM.MetricOption) -> MetricChoice {
        MetricChoice(key: vm.key, label: vm.label, unit: vm.unit, isObjective: vm.objective)
    }

    static func compareLine(_ vm: VM.CompareRow) -> CompareLine {
        CompareLine(
            key: vm.key, label: vm.label, unit: vm.unit, baseline: vm.baseline, best: vm.best, deltaPercent: vm.deltaPct,
            lowerIsBetter: vm.better == .lower)
    }

    static func spanTiming(_ vm: VM.SpanRow) -> SpanTiming {
        SpanTiming(name: vm.name, baselineMS: vm.baseline, bestMS: vm.best)
    }

    static func winningChange(_ vm: VM.WinningChangeVm) -> WinningChange {
        WinningChange(
            commits: Int(vm.commits), sha: vm.sha, baseRef: vm.baseRef,
            files: vm.files.map { DiffFile(path: $0.path, additions: Int($0.additions), deletions: Int($0.deletions), binary: $0.binary) },
            additions: Int(vm.additions), deletions: Int(vm.deletions), excerptPath: vm.excerpt?.path,
            excerpt: (vm.excerpt?.lines ?? []).map { DiffLine(kind: diffKind($0.kind), text: $0.text) },
            unavailable: vm.unavailable)
    }

    static func diffKind(_ kind: VM.DiffLineKind) -> DiffLineKind {
        switch kind {
        case .add: .add
        case .del: .del
        case .ctx: .ctx
        case .hunk: .hunk
        }
    }

    /// Everything the Result screen shows beyond the basic detail.
    static func insights(_ detail: VM.OptimizationDetail, spend: SpendBreakdown?, change: WinningChange?) -> RunInsights {
        RunInsights(
            compare: detail.compare.map(compareLine), spans: detail.spans?.map(spanTiming),
            baselineSamples: detail.baselineSamples, metrics: detail.metrics.map(metricChoice),
            calibration: detail.calibration, spend: spend, change: change)
    }

    // MARK: Benchmarks

    static func benchmarkSeries(_ vm: VM.RatchetSeries) -> BenchmarkSeries {
        BenchmarkSeries(
            metric: vm.metric, options: vm.options.map(metricChoice), unit: vm.unit, lowerIsBetter: vm.direction == .lower,
            baseline: vm.baseline, steps: vm.steps.map { BenchmarkSeries.Step(minutes: $0.t, value: $0.value) },
            points: vm.points.enumerated().map { index, point in
                BenchmarkSeries.Point(
                    id: index, minutes: point.t, value: point.value, agent: title(of: point.agent), verified: point.verified,
                    label: point.label)
            },
            noiseFloor: vm.noiseFloor, tMax: vm.tMax)
    }

    static func candidateResult(_ vm: VM.CandidateVm) -> CandidateResult {
        let result: CandidateResult.Result
        switch vm.verdict {
        case .pending: result = .pending
        case .verified: result = .verified
        case .rejected: result = .rejected
        case .failedConfirm: result = .failedConfirm
        case .blocked: result = .blocked
        }
        let firstLine = vm.message.split(whereSeparator: \.isNewline).first.map(String.init) ?? vm.message
        return CandidateResult(
            id: vm.id, sha: String(vm.sha.prefix(7)), agent: title(of: vm.agent), message: firstLine, result: result,
            reasons: vm.reasons, benchValue: vm.benchValue, confirmValue: vm.confirmValue, vsBaselinePercent: vm.vsBaselinePct,
            movedRatchet: vm.movedRatchet, samples: vm.samples, at: date(ms: vm.submittedAt))
    }
}
