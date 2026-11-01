import Foundation

// Domain types, shaped like the REST resources of `ratchetd`. Plain Sendable values: the generated OpenAPI client
// replaces the mock without touching them (see RatchetClient.swift).

// MARK: - Ranges and metrics

public enum TimeRange: String, CaseIterable, Codable, Sendable, Identifiable {
    case days30, days90, all

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .days30: "30 days"
        case .days90: "90 days"
        case .all: "All time"
        }
    }

    /// nil means everything.
    public var days: Int? {
        switch self {
        case .days30: 30
        case .days90: 90
        case .all: nil
        }
    }
}

/// How one optimization's number reads: "412 → 288 ms", "nDCG 0.612 → 0.648", "WAPE 18.2 → 15.9%".
public struct MetricSpec: Codable, Hashable, Sendable {
    public var name: String
    /// Prefix of the result line when the unit alone is not enough ("nDCG").
    public var label: String?
    public var unit: String
    public var decimals: Int
    public var lowerIsBetter: Bool

    public init(name: String, label: String? = nil, unit: String = "", decimals: Int = 0, lowerIsBetter: Bool = true) {
        self.name = name
        self.label = label
        self.unit = unit
        self.decimals = decimals
        self.lowerIsBetter = lowerIsBetter
    }

    public func number(_ value: Double) -> String {
        String(format: "%.\(decimals)f", value)
    }

    /// One value with its unit: "288 ms", "0.648", "15.9%".
    public func text(_ value: Double) -> String {
        let n = number(value)
        if unit.isEmpty { return n }
        return unit == "%" ? n + unit : n + " " + unit
    }

    /// "412 → 288 ms", with the label in front when there is one.
    public func pair(from: Double, to: Double) -> String {
        var line = number(from) + " → " + text(to)
        if let label { line = label + " " + line }
        return line
    }
}

// MARK: - Optimizations

/// The markdown report of an optimization, as the daemon renders it (`GET /v1/optimizations/{id}/report`).
public struct OptimizationReport: Hashable, Sendable {
    public var fileName: String
    public var markdown: String

    public init(fileName: String, markdown: String) {
        self.fileName = fileName
        self.markdown = markdown
    }
}

public enum OptimizationStatus: String, CaseIterable, Codable, Sendable {
    case running, waiting, merged, done, noWin

    public var title: String {
        switch self {
        case .running: "Running"
        case .waiting: "Waiting on you"
        case .merged: "Merged"
        case .done: "Done"
        case .noWin: "No win"
        }
    }

    public var isFinished: Bool { self == .merged || self == .done || self == .noWin }
}

public struct Optimization: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var title: String
    public var repo: String
    public var branch: String
    public var goal: String
    public var status: OptimizationStatus
    public var metric: MetricSpec
    public var baseline: Double
    public var best: Double?
    /// Measurement noise as a fraction (0.021 is ±2.1%).
    public var noise: Double?
    public var spentUSD: Double
    public var budgetUSD: Double?
    public var valuePerYearUSD: Double?
    public var secondsLeft: Double?
    public var timeLimitHours: Double
    public var startedAt: Date?
    public var prNumber: Int?
    /// Where the pull request lives (any host: GitHub Enterprise too), when there is one.
    public var prURL: String?
    public var winsCount: Int
    public var evalsCount: Int
    public var agentsCount: Int

    public init(
        id: String, title: String, repo: String, branch: String = "main", goal: String = "",
        status: OptimizationStatus, metric: MetricSpec, baseline: Double, best: Double? = nil, noise: Double? = nil,
        spentUSD: Double = 0, budgetUSD: Double? = nil, valuePerYearUSD: Double? = nil,
        secondsLeft: Double? = nil, timeLimitHours: Double = 2, startedAt: Date? = nil, prNumber: Int? = nil,
        prURL: String? = nil, winsCount: Int = 0, evalsCount: Int = 0, agentsCount: Int = 1
    ) {
        self.id = id
        self.title = title
        self.repo = repo
        self.branch = branch
        self.goal = goal
        self.status = status
        self.metric = metric
        self.baseline = baseline
        self.best = best
        self.noise = noise
        self.spentUSD = spentUSD
        self.budgetUSD = budgetUSD
        self.valuePerYearUSD = valuePerYearUSD
        self.secondsLeft = secondsLeft
        self.timeLimitHours = timeLimitHours
        self.startedAt = startedAt
        self.prNumber = prNumber
        self.prURL = prURL
        self.winsCount = winsCount
        self.evalsCount = evalsCount
        self.agentsCount = agentsCount
    }

    /// (best - baseline) / baseline, or nil before the first win.
    public var change: Double? {
        guard let best, baseline != 0 else { return nil }
        return (best - baseline) / baseline
    }

    public var isImprovement: Bool {
        guard let change else { return false }
        return metric.lowerIsBetter ? change < 0 : change > 0
    }

    /// "412 → 288 ms", or nil before the first win.
    public var resultText: String? {
        guard let best else { return nil }
        return metric.pair(from: baseline, to: best)
    }

    /// Share of the budget spent, 0 to 1.
    public var budgetFraction: Double? {
        guard let budgetUSD, budgetUSD > 0 else { return nil }
        return min(max(spentUSD / budgetUSD, 0), 1)
    }

    public var isLive: Bool { status == .running || status == .waiting }
}

public enum Verdict: String, CaseIterable, Codable, Sendable {
    case evaluating, verified, noise, blocked

    public var title: String {
        switch self {
        case .evaluating: "Evaluating"
        case .verified: "Verified win"
        case .noise: "Within noise"
        case .blocked: "Blocked"
        }
    }
}

public struct Candidate: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var title: String
    public var agent: String
    public var verdict: Verdict
    /// The measured value once the evaluator is done.
    public var value: Double?
    public var at: Date

    public init(id: String, title: String, agent: String, verdict: Verdict, value: Double? = nil, at: Date) {
        self.id = id
        self.title = title
        self.agent = agent
        self.verdict = verdict
        self.value = value
        self.at = at
    }
}

/// One step of the ratchet: a verified win that became the new best.
public struct RatchetMove: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var at: Date
    public var value: Double
    public var title: String

    public init(id: String, at: Date, value: Double, title: String) {
        self.id = id
        self.at = at
        self.value = value
        self.title = title
    }
}

public enum Placement: String, CaseIterable, Codable, Sendable {
    case local, cloud

    /// "this Mac" or "the cloud", for "Runs on ...".
    public var title: String { self == .local ? "this Mac" : "the cloud" }
}

public enum AgentState: String, Codable, Sendable {
    case working, paused, idle

    public var title: String {
        switch self {
        case .working: "Working"
        case .paused: "Paused"
        case .idle: "Idle"
        }
    }
}

public struct Agent: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var name: String
    public var placement: Placement
    public var state: AgentState
    public var activity: String
    public var evals: Int
    public var wins: Int
    public var spentUSD: Double

    public init(
        id: String, name: String, placement: Placement, state: AgentState, activity: String = "",
        evals: Int = 0, wins: Int = 0, spentUSD: Double = 0
    ) {
        self.id = id
        self.name = name
        self.placement = placement
        self.state = state
        self.activity = activity
        self.evals = evals
        self.wins = wins
        self.spentUSD = spentUSD
    }
}

public struct OptimizationDetail: Codable, Hashable, Sendable {
    public var optimization: Optimization
    public var agents: [Agent]
    public var candidates: [Candidate]
    public var moves: [RatchetMove]
    public var receipt: TeardownReceipt?
    public var filesChanged: Int
    public var additions: Int
    public var deletions: Int

    public init(
        optimization: Optimization, agents: [Agent] = [], candidates: [Candidate] = [], moves: [RatchetMove] = [],
        receipt: TeardownReceipt? = nil, filesChanged: Int = 0, additions: Int = 0, deletions: Int = 0
    ) {
        self.optimization = optimization
        self.agents = agents
        self.candidates = candidates
        self.moves = moves
        self.receipt = receipt
        self.filesChanged = filesChanged
        self.additions = additions
        self.deletions = deletions
    }
}

// MARK: - Overview and spend

public struct ValuePoint: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var date: Date
    public var valuePerYearUSD: Double
    /// The win behind this step; nil for the start and end of the window.
    public var win: String?
    public var optimizationTitle: String?

    public init(id: String, date: Date, valuePerYearUSD: Double, win: String? = nil, optimizationTitle: String? = nil) {
        self.id = id
        self.date = date
        self.valuePerYearUSD = valuePerYearUSD
        self.win = win
        self.optimizationTitle = optimizationTitle
    }
}

public enum NeedsYouKind: String, Codable, Sendable {
    case approveEnvironment, mergeWins
    /// A connector that needs signing in again; opens the Connectors page.
    case signIn
    /// A budget that ran out; opens the optimization.
    case budgetLimit
    /// A run that failed; opens the optimization.
    case runFailed
}

public struct NeedsYouItem: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var kind: NeedsYouKind
    public var title: String
    public var detail: String
    public var actionTitle: String
    public var optimizationID: String

    public init(id: String, kind: NeedsYouKind, title: String, detail: String, actionTitle: String, optimizationID: String) {
        self.id = id
        self.kind = kind
        self.title = title
        self.detail = detail
        self.actionTitle = actionTitle
        self.optimizationID = optimizationID
    }
}

public struct Overview: Codable, Hashable, Sendable {
    public var range: TimeRange
    public var spentUSD: Double
    public var budgetUSD: Double
    public var valueLockedPerYearUSD: Double
    public var winsCount: Int
    /// Chronological. The first and last points have no `win`.
    public var series: [ValuePoint]
    public var needsYou: [NeedsYouItem]

    public init(
        range: TimeRange, spentUSD: Double, budgetUSD: Double, valueLockedPerYearUSD: Double, winsCount: Int,
        series: [ValuePoint], needsYou: [NeedsYouItem]
    ) {
        self.range = range
        self.spentUSD = spentUSD
        self.budgetUSD = budgetUSD
        self.valueLockedPerYearUSD = valueLockedPerYearUSD
        self.winsCount = winsCount
        self.series = series
        self.needsYou = needsYou
    }

    /// Yearly value for every $1 spent (84 for $15.4k on $184).
    public var returnMultiple: Double? {
        spentUSD > 0 ? valueLockedPerYearUSD / spentUSD : nil
    }

    /// Days until the yearly value has paid for what was spent.
    public var paybackDays: Double? {
        valueLockedPerYearUSD > 0 ? spentUSD / (valueLockedPerYearUSD / 365) : nil
    }

    public var spentFraction: Double {
        budgetUSD > 0 ? min(max(spentUSD / budgetUSD, 0), 1) : 0
    }
}

public struct SpendLine: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var label: String
    public var usd: Double

    public init(id: String, label: String, usd: Double) {
        self.id = id
        self.label = label
        self.usd = usd
    }
}

public struct SpendSummary: Codable, Hashable, Sendable {
    public var range: TimeRange
    public var totalUSD: Double
    public var budgetUSD: Double
    public var lines: [SpendLine]

    public init(range: TimeRange, totalUSD: Double, budgetUSD: Double, lines: [SpendLine]) {
        self.range = range
        self.totalUSD = totalUSD
        self.budgetUSD = budgetUSD
        self.lines = lines
    }
}

// MARK: - Benchmarks, templates, repos

public struct BenchmarkRun: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var title: String
    public var optimizationTitle: String
    public var templateID: String
    public var metric: MetricSpec
    public var baseline: Double
    public var best: Double?
    public var noise: Double
    public var evals: Int
    public var date: Date

    public init(
        id: String, title: String, optimizationTitle: String, templateID: String, metric: MetricSpec,
        baseline: Double, best: Double? = nil, noise: Double, evals: Int, date: Date
    ) {
        self.id = id
        self.title = title
        self.optimizationTitle = optimizationTitle
        self.templateID = templateID
        self.metric = metric
        self.baseline = baseline
        self.best = best
        self.noise = noise
        self.evals = evals
        self.date = date
    }

    public var change: Double? {
        guard let best, baseline != 0 else { return nil }
        return (best - baseline) / baseline
    }

    public var isImprovement: Bool {
        guard let change else { return false }
        return metric.lowerIsBetter ? change < 0 : change > 0
    }
}

public enum TemplateKind: String, Codable, Sendable {
    case template, demo
}

public struct Template: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var kind: TemplateKind
    public var category: String
    public var title: String
    public var summary: String
    public var measures: String
    public var machine: String

    public init(id: String, kind: TemplateKind, category: String, title: String, summary: String, measures: String, machine: String) {
        self.id = id
        self.kind = kind
        self.category = category
        self.title = title
        self.summary = summary
        self.measures = measures
        self.machine = machine
    }
}

public struct Repo: Identifiable, Codable, Hashable, Sendable {
    /// "owner/name".
    public var id: String
    public var defaultBranch: String
    public var account: String
    /// Shown in the search results; nil when the source does not say.
    public var isPrivate: Bool?
    public var summary: String?

    public init(id: String, defaultBranch: String = "main", account: String, isPrivate: Bool? = nil, summary: String? = nil) {
        self.id = id
        self.defaultBranch = defaultBranch
        self.account = account
        self.isPrivate = isPrivate
        self.summary = summary
    }

    public var name: String { id.split(separator: "/").last.map(String.init) ?? id }
}

public struct HarnessDetection: Codable, Hashable, Sendable {
    public var repoID: String
    public var found: Bool
    public var summary: String
    public var suggestedTemplateID: String?

    public init(repoID: String, found: Bool, summary: String, suggestedTemplateID: String? = nil) {
        self.repoID = repoID
        self.found = found
        self.summary = summary
        self.suggestedTemplateID = suggestedTemplateID
    }
}
