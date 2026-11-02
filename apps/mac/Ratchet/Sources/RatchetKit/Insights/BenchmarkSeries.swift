import Foundation

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
