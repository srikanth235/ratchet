import Foundation

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
