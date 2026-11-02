import Foundation

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
