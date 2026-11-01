import Foundation

// What the Result, Benchmarks and live-agent screens show beyond the basic optimization detail: an agent's log, the spend
// breakdown, "where the time went", the before/after rows, the winning change's diff, the ratchet series of one metric and
// every candidate with its bench result. Plain values, with the small rules the Electron renderer had (filters, deltas,
// histogram bins, bar heights) as pure functions so the tests run them without a daemon.

// MARK: - Agent log

/// One line of an agent's log (`GET /v1/agents/{id}/events`).
public struct AgentLogLine: Identifiable, Codable, Hashable, Sendable {
    public var id: Int64
    public var at: Date
    /// "status", "message", "verdict", "tool"...: how the line is tinted.
    public var kind: String
    public var text: String

    public init(id: Int64, at: Date, kind: String, text: String) {
        self.id = id
        self.at = at
        self.kind = kind
        self.text = text
    }
}

/// A page of the log, oldest first. `cursor` goes back as the `cursor` of the next request to read only newer lines.
public struct AgentLogPage: Hashable, Sendable {
    public var lines: [AgentLogLine]
    public var cursor: Int64

    public init(lines: [AgentLogLine], cursor: Int64) {
        self.lines = lines
        self.cursor = cursor
    }
}

/// What the app keeps of one agent's log: the newest page first, then only what came after the cursor, the newest 300 lines.
public struct AgentLog: Hashable, Sendable {
    public static let capacity = 300

    public var lines: [AgentLogLine] = []
    public var cursor: Int64?
    /// True once the first page arrived (an empty log is "No events yet", not "loading").
    public var isLoaded = false
    public var error: String?

    public init() {}

    /// Appends a page. An empty page keeps the cursor; lines already shown are skipped, so a page read twice adds nothing.
    public mutating func apply(_ page: AgentLogPage) {
        isLoaded = true
        error = nil
        guard !page.lines.isEmpty else { return }
        let known = lines.last?.id ?? Int64.min
        lines.append(contentsOf: page.lines.filter { $0.id > known })
        if lines.count > Self.capacity { lines.removeFirst(lines.count - Self.capacity) }
        cursor = page.cursor
    }

    public mutating func fail(_ message: String) {
        isLoaded = true
        error = message
    }
}

// MARK: - Spend

/// One agent's share of the spend. `agentID` is nil for the shared infrastructure (the sandbox machines).
public struct AgentSpend: Identifiable, Hashable, Sendable {
    public var id: String
    public var agentID: String?
    public var name: String
    public var usd: Double
    public var wins: Int

    public init(id: String, agentID: String?, name: String, usd: Double, wins: Int) {
        self.id = id
        self.agentID = agentID
        self.name = name
        self.usd = usd
        self.wins = wins
    }

    public var isShared: Bool { agentID == nil }

    /// Spend for one verified win, or nil with no win.
    public var perWinUSD: Double? { wins > 0 ? usd / Double(wins) : nil }
}

/// Where one optimization's money went: by category (machines, experiments, tokens...) and by agent.
public struct SpendBreakdown: Hashable, Sendable {
    public var totalUSD: Double
    /// The part not yet reconciled with billing.
    public var estimatedUSD: Double
    public var budgetUSD: Double
    public var categories: [SpendLine]
    public var agents: [AgentSpend]
    /// Verified wins of the whole run.
    public var wins: Int

    public init(
        totalUSD: Double, estimatedUSD: Double, budgetUSD: Double, categories: [SpendLine], agents: [AgentSpend], wins: Int
    ) {
        self.totalUSD = totalUSD
        self.estimatedUSD = estimatedUSD
        self.budgetUSD = budgetUSD
        self.categories = categories
        self.agents = agents
        self.wins = wins
    }

    /// Spend for one verified win of the run, or nil with no win.
    public var perWinUSD: Double? { wins > 0 ? totalUSD / Double(wins) : nil }

    /// Each category's share of the total, 0 to 1, in the order of `categories`.
    public var categoryFractions: [Double] {
        let sum = categories.reduce(0) { $0 + $1.usd }
        guard sum > 0 else { return categories.map { _ in 0 } }
        return categories.map { $0.usd / sum }
    }

    /// Each agent's share of the total, 0 to 1.
    public func fraction(of agent: AgentSpend) -> Double {
        totalUSD > 0 ? min(max(agent.usd / totalUSD, 0), 1) : 0
    }

    /// "Includes $1.20 estimated" or "Reconciled with billing".
    public var reconciliationNote: String {
        estimatedUSD > 0 ? "Includes " + Money.exact(estimatedUSD) + " estimated" : "Reconciled with billing"
    }
}

// MARK: - Where the time went, before and after

/// One span the evaluator timed, baseline against the best verified commit, in milliseconds.
public struct SpanTiming: Identifiable, Hashable, Sendable {
    public var name: String
    public var baselineMS: Double
    public var bestMS: Double

    public var id: String { name }

    public init(name: String, baselineMS: Double, bestMS: Double) {
        self.name = name
        self.baselineMS = baselineMS
        self.bestMS = bestMS
    }

    public var drop: Double { baselineMS - bestMS }

    /// The span that lost the most time, when any lost some.
    public static func biggestDrop(in spans: [SpanTiming]) -> SpanTiming? {
        guard let top = spans.max(by: { $0.drop < $1.drop }), top.drop > 0 else { return nil }
        return top
    }

    /// The longest bar of the list, which the other bars are drawn against.
    public static func longest(in spans: [SpanTiming]) -> Double {
        max(spans.map { max($0.baselineMS, $0.bestMS) }.max() ?? 1, 1)
    }
}

/// One row of "Before and after" / "Baseline vs best".
public struct CompareLine: Identifiable, Hashable, Sendable {
    public var key: String
    public var label: String
    public var unit: String
    public var baseline: Double?
    public var best: Double?
    /// Signed % change, best against baseline.
    public var deltaPercent: Double?
    public var lowerIsBetter: Bool

    public var id: String { key }

    public init(key: String, label: String, unit: String, baseline: Double?, best: Double?, deltaPercent: Double?, lowerIsBetter: Bool) {
        self.key = key
        self.label = label
        self.unit = unit
        self.baseline = baseline
        self.best = best
        self.deltaPercent = deltaPercent
        self.lowerIsBetter = lowerIsBetter
    }

    /// Worse than before by at least a whole percent, in the direction this metric counts as worse.
    public var isWorse: Bool {
        guard let delta = deltaPercent, delta.rounded() != 0 else { return false }
        return (delta > 0) == lowerIsBetter
    }

    /// "−28%", "+3.5%", "—". One decimal below 10%, none from there, unless `decimals` says otherwise.
    public func deltaText(decimals: Int? = nil) -> String {
        guard let delta = deltaPercent else { return "—" }
        let sign = delta < 0 ? "−" : "+"
        let magnitude = abs(delta)
        let places = decimals ?? (magnitude < 10 ? 1 : 0)
        return sign + String(format: "%.\(places)f", magnitude) + "%"
    }

    /// A value of this row with its unit: "182 ms", "0.31", "—".
    public func text(_ value: Double?) -> String {
        guard let value else { return "—" }
        return MetricNumber.text(value, unit: unit)
    }

    /// A guardrail is a metric outside the objective's family: the verified commit only had to pass it.
    public func isGuardrail(family: Set<String>) -> Bool { !family.contains(key) }
}

/// A measured value with about three significant digits and its unit: "131 ms", "0.648", "15.9%".
public enum MetricNumber {
    public static func decimals(_ value: Double) -> Int {
        let magnitude = abs(value)
        if magnitude >= 100 || magnitude == 0 { return 0 }
        if magnitude >= 10 { return 1 }
        return magnitude >= 1 ? 2 : 3
    }

    public static func text(_ value: Double) -> String {
        String(format: "%.\(decimals(value))f", value)
    }

    public static func text(_ value: Double, unit: String) -> String {
        let number = text(value)
        if unit.isEmpty { return number }
        return unit == "%" ? number + unit : number + " " + unit
    }
}

// MARK: - Winning change

public enum DiffLineKind: String, Hashable, Sendable {
    case add, del, ctx, hunk
}

public struct DiffLine: Hashable, Sendable {
    public var kind: DiffLineKind
    public var text: String

    public init(kind: DiffLineKind, text: String) {
        self.kind = kind
        self.text = text
    }
}

public struct DiffFile: Identifiable, Hashable, Sendable {
    public var path: String
    public var additions: Int
    public var deletions: Int
    public var binary: Bool

    public var id: String { path }

    public init(path: String, additions: Int, deletions: Int, binary: Bool) {
        self.path = path
        self.additions = additions
        self.deletions = deletions
        self.binary = binary
    }
}

/// The squashed result as the pull request will show it: the best verified commit against the base branch.
public struct WinningChange: Hashable, Sendable {
    public var commits: Int
    public var sha: String?
    public var baseRef: String
    public var files: [DiffFile]
    public var additions: Int
    public var deletions: Int
    /// The first hunk of the most-changed file.
    public var excerptPath: String?
    public var excerpt: [DiffLine]
    /// Why there is no diff (no win yet, no git access, git failed); nil when `files` is real.
    public var unavailable: String?

    public init(
        commits: Int, sha: String?, baseRef: String, files: [DiffFile] = [], additions: Int = 0, deletions: Int = 0,
        excerptPath: String? = nil, excerpt: [DiffLine] = [], unavailable: String? = nil
    ) {
        self.commits = commits
        self.sha = sha
        self.baseRef = baseRef
        self.files = files
        self.additions = additions
        self.deletions = deletions
        self.excerptPath = excerptPath
        self.excerpt = excerpt
        self.unavailable = unavailable
    }

    /// "5 verified commits, squashed · a91f3c2 vs main", or "nothing to squash yet".
    public var headline: String {
        var text = commits > 0 ? Plural.count(commits, "verified commit") + ", squashed" : "nothing to squash yet"
        if let sha { text += " · \(sha) vs \(baseRef)" }
        return text
    }

    /// "2 files · +40 −2".
    public var statLine: String {
        Plural.count(files.count, "file") + " · +\(additions) −\(deletions)"
    }
}

extension DiffLine {
    /// How a line reads in the excerpt: a hunk header keeps its `@@ … @@` and names the file, the rest get a "+", "-" or " ".
    public func rendered(path: String) -> String {
        switch kind {
        case .hunk:
            // "@@ -1,3 +1,4 @@ function name" keeps only the ranges.
            if text.hasPrefix("@@"), let close = text.dropFirst(2).range(of: "@@") {
                return String(text[text.startIndex..<close.upperBound]) + " " + path
            }
            return text + " " + path
        case .add: return "+ " + text
        case .del: return "- " + text
        case .ctx: return "  " + text
        }
    }
}

// MARK: - Everything the Result screen adds

/// A metric the evaluator reports next to the objective (p50/p95/p99, or WAPE/MAPE/bias).
public struct MetricChoice: Identifiable, Hashable, Sendable {
    public var key: String
    public var label: String
    public var unit: String
    public var isObjective: Bool

    public var id: String { key }

    public init(key: String, label: String, unit: String, isObjective: Bool) {
        self.key = key
        self.label = label
        self.unit = unit
        self.isObjective = isObjective
    }
}

/// What the Result and Benchmarks screens read besides the plain optimization detail.
public struct RunInsights: Hashable, Sendable {
    public var compare: [CompareLine]
    /// nil when the evaluator reports no spans or there is no verified win yet.
    public var spans: [SpanTiming]?
    public var baselineSamples: [Double]
    public var metrics: [MetricChoice]
    public var calibration: String?
    public var spend: SpendBreakdown?
    public var change: WinningChange?

    public init(
        compare: [CompareLine] = [], spans: [SpanTiming]? = nil, baselineSamples: [Double] = [], metrics: [MetricChoice] = [],
        calibration: String? = nil, spend: SpendBreakdown? = nil, change: WinningChange? = nil
    ) {
        self.compare = compare
        self.spans = spans
        self.baselineSamples = baselineSamples
        self.metrics = metrics
        self.calibration = calibration
        self.spend = spend
        self.change = change
    }

    /// The keys of the objective's family; every other compare row is a guardrail.
    public var family: Set<String> { Set(metrics.map(\.key)) }
}

// MARK: - Benchmarks

/// The ratchet of one metric: the staircase of the best value and a point per candidate.
public struct BenchmarkSeries: Hashable, Sendable {
    public struct Step: Hashable, Sendable {
        /// Minutes since the run started.
        public var minutes: Double
        public var value: Double

        public init(minutes: Double, value: Double) {
            self.minutes = minutes
            self.value = value
        }
    }

    public struct Point: Identifiable, Hashable, Sendable {
        public var id: Int
        public var minutes: Double
        public var value: Double
        public var agent: String
        public var verified: Bool
        public var label: String?

        public init(id: Int, minutes: Double, value: Double, agent: String, verified: Bool, label: String? = nil) {
            self.id = id
            self.minutes = minutes
            self.value = value
            self.agent = agent
            self.verified = verified
            self.label = label
        }
    }

    /// The metric plotted (the objective unless another option was asked for).
    public var metric: String
    public var options: [MetricChoice]
    public var unit: String
    public var lowerIsBetter: Bool
    public var baseline: Double?
    public var steps: [Step]
    public var points: [Point]
    /// Absolute noise floor of the baseline, in the metric's units; nil for a metric other than the objective.
    public var noiseFloor: Double?
    public var tMax: Double

    public init(
        metric: String, options: [MetricChoice], unit: String, lowerIsBetter: Bool, baseline: Double?, steps: [Step],
        points: [Point], noiseFloor: Double?, tMax: Double
    ) {
        self.metric = metric
        self.options = options
        self.unit = unit
        self.lowerIsBetter = lowerIsBetter
        self.baseline = baseline
        self.steps = steps
        self.points = points
        self.noiseFloor = noiseFloor
        self.tMax = tMax
    }

    /// The switch is shown only when the evaluator reports several metrics of the family.
    public var hasMetricSwitch: Bool { options.count > 1 }

    public var selected: MetricChoice? { options.first { $0.key == metric } }

    /// The best verified value so far (the last step).
    public var best: Double? { steps.last?.value }

    /// Verified wins: every step after the baseline.
    public var locks: Int { max(steps.count - 1, 0) }

    /// How much the best improved on the baseline, positive when it did, nil before the first win.
    public var improvement: Double? {
        guard let baseline, let best, steps.count > 1 else { return nil }
        return lowerIsBetter ? baseline - best : best - baseline
    }

    /// Points to draw: with `showRejected` off, only the ones that moved the ratchet.
    public func visiblePoints(showRejected: Bool) -> [Point] {
        showRejected ? points : points.filter(\.verified)
    }

    /// The agents that appear on the chart, in the order they first do.
    public var agents: [String] {
        var seen: [String] = []
        for point in points where !seen.contains(point.agent) { seen.append(point.agent) }
        return seen
    }
}

/// A candidate with everything the bench measured, for the candidates table and the histogram.
public struct CandidateResult: Identifiable, Hashable, Sendable {
    public enum Result: String, CaseIterable, Hashable, Sendable {
        case pending, verified, rejected, failedConfirm, blocked

        public var title: String {
            switch self {
            case .pending: "Evaluating"
            case .verified: "Verified win"
            case .rejected: "Rejected"
            case .failedConfirm: "Failed confirm"
            case .blocked: "Blocked"
            }
        }
    }

    public var id: String
    public var sha: String
    public var agent: String
    public var message: String
    public var result: Result
    public var reasons: [String]
    public var benchValue: Double?
    public var confirmValue: Double?
    /// Signed % improvement against the baseline (positive is better).
    public var vsBaselinePercent: Double?
    public var movedRatchet: Bool
    /// Per-run samples of the latest bench run.
    public var samples: [Double]
    public var at: Date

    public init(
        id: String, sha: String, agent: String, message: String, result: Result, reasons: [String] = [],
        benchValue: Double? = nil, confirmValue: Double? = nil, vsBaselinePercent: Double? = nil,
        movedRatchet: Bool = false, samples: [Double] = [], at: Date = Date(timeIntervalSince1970: 0)
    ) {
        self.id = id
        self.sha = sha
        self.agent = agent
        self.message = message
        self.result = result
        self.reasons = reasons
        self.benchValue = benchValue
        self.confirmValue = confirmValue
        self.vsBaselinePercent = vsBaselinePercent
        self.movedRatchet = movedRatchet
        self.samples = samples
        self.at = at
    }

    /// "passed", or the reason it was blocked, cut to 40 characters.
    public var guardrailText: String {
        guard result == .blocked else { return "passed" }
        return String((reasons.first ?? "blocked").prefix(40))
    }

    /// The first reason, shown under the change when the candidate was not blocked.
    public var note: String? { result == .blocked ? nil : reasons.first }
}

/// The verdict chips over the candidates table.
public enum CandidateFilter: String, CaseIterable, Identifiable, Sendable {
    case all, verified, rejected, failedConfirm, blocked, pending

    public var id: String { rawValue }

    /// "all", "verified", "failed confirm"...
    public var title: String {
        rawValue == "failedConfirm" ? "failed confirm" : rawValue
    }

    public func includes(_ candidate: CandidateResult) -> Bool {
        switch self {
        case .all: true
        case .verified: candidate.result == .verified
        case .rejected: candidate.result == .rejected
        case .failedConfirm: candidate.result == .failedConfirm
        case .blocked: candidate.result == .blocked
        case .pending: candidate.result == .pending
        }
    }

    public func apply(to candidates: [CandidateResult]) -> [CandidateResult] {
        candidates.filter(includes)
    }
}

// MARK: - Chart math

public enum Histogram {
    /// How many bins a histogram of `count` samples gets: about 4 per square root, between 8 and 28.
    public static func binCount(forSamples count: Int) -> Int {
        max(8, min(28, Int((Double(max(count, 1)).squareRoot() * 4).rounded())))
    }

    /// Counts every finite sample in `lo...hi`; the maximum goes into the last bin.
    public static func bins(of values: [Double], lo: Double, hi: Double, count: Int) -> [Int] {
        var out = [Int](repeating: 0, count: max(count, 1))
        let span = hi - lo == 0 ? 1 : hi - lo
        for value in values where value.isFinite {
            let index = Int(((value - lo) / span * Double(out.count)).rounded(.down))
            out[min(out.count - 1, max(0, index))] += 1
        }
        return out
    }

    /// The shared range of the series, so two histograms overlay on one x axis; nil without any finite sample.
    public static func range(of series: [[Double]]) -> ClosedRange<Double>? {
        let all = series.flatMap { $0 }.filter(\.isFinite)
        guard let low = all.min(), let high = all.max() else { return nil }
        return low...high
    }
}

public enum EvalBars {
    /// Heights from 6 to `height`, scaled over the value range (all equal values get 60%). Blocked bars are drawn as a flat
    /// stub by the view; this is only the scaling.
    public static func heights(of values: [Double], height: Double) -> [Double] {
        guard let low = values.min(), let high = values.max() else { return [] }
        let span = high - low
        return values.map { value in
            max(6, span == 0 ? (height * 0.6).rounded() : (6 + (value - low) / span * (height - 12)).rounded())
        }
    }

    /// Where a dashed guide for `best` sits inside bars scaled by `heights(of:height:)`, or nil when all values are equal.
    public static func guide(for best: Double, values: [Double], height: Double) -> Double? {
        guard let low = values.min(), let high = values.max(), high != low else { return nil }
        return 6 + (best - low) / (high - low) * (height - 12)
    }
}
