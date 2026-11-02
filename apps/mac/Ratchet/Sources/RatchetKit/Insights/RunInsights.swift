import Foundation

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
