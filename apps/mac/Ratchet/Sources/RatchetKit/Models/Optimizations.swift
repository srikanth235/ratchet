import Foundation

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
        // Created and not started yet. Nothing waits on a person: Run Now starts it, or its schedule does.
        case .waiting: "Not started"
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
