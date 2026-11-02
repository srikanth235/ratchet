import Foundation

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
