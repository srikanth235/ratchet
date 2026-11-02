import Foundation

// The one value story Home and the Overview both tell, and what the Overview adds to it (`docs/harness-matrix.md`, P7 notes).
// Home reads `GET /v1/home`, the Overview reads `GET /v1/overview`: both carry the same `story`, so the numbers cannot differ.

/// `estimated` until the value has been measured in production.
public enum ValueBasisKind: String, Codable, Sendable {
    case estimated, measured
}

/// Wins counted in dollars per year.
public struct ValueAmount: Hashable, Sendable {
    public var usdPerYear: Double
    public var wins: Int
    /// Open pull requests the wins are in (only the pending amount has them).
    public var pullRequests: Int

    public init(usdPerYear: Double = 0, wins: Int = 0, pullRequests: Int = 0) {
        self.usdPerYear = usdPerYear
        self.wins = wins
        self.pullRequests = pullRequests
    }
}

/// What was spent, what is delivered (merged pull requests), what is pending (open ones) and what it returned. Pending value
/// never enters the return.
public struct ValueStory: Hashable, Sendable {
    public var basis: ValueBasisKind
    public var delivered: ValueAmount
    public var pending: ValueAmount
    /// Verified wins with no pull request yet ("Ready to ship").
    public var readyToShip: ValueAmount
    /// Real dollars in the range.
    public var spentUSD: Double
    /// Yearly value delivered per $1 spent; nil until something is delivered and something was spent.
    public var returnMultiple: Double?
    public var paybackDays: Double?
    /// The daemon's sentence: "$12.5k delivered + $2.9k pending · 68×". Empty without a value model.
    public var line: String

    public init(
        basis: ValueBasisKind = .estimated, delivered: ValueAmount = ValueAmount(), pending: ValueAmount = ValueAmount(),
        readyToShip: ValueAmount = ValueAmount(), spentUSD: Double = 0, returnMultiple: Double? = nil, paybackDays: Double? = nil,
        line: String = ""
    ) {
        self.basis = basis
        self.delivered = delivered
        self.pending = pending
        self.readyToShip = readyToShip
        self.spentUSD = spentUSD
        self.returnMultiple = returnMultiple
        self.paybackDays = paybackDays
        self.line = line
    }

    // The pieces the two screens draw, so Home and the Overview word them the same.

    public var spentText: String { Money.short(spentUSD) }
    /// "$12.5k": the big number, delivered value per year.
    public var deliveredText: String { Money.short(delivered.usdPerYear) }
    /// "$2.9k pending"; nil when nothing is pending.
    public var pendingText: String? { pending.usdPerYear > 0 ? Money.short(pending.usdPerYear) + " pending" : nil }
    /// "68×"; nil until it can be computed.
    public var returnText: String? { returnMultiple.map(Figure.multiple) }
    public var hasValue: Bool { delivered.usdPerYear > 0 || pending.usdPerYear > 0 || readyToShip.usdPerYear > 0 }

    /// "$12.5k delivered + $2.9k pending · 68×", from the numbers when the daemon sent no sentence.
    public var headline: String {
        if !line.isEmpty { return line }
        guard hasValue else { return "" }
        var text = deliveredText + " delivered"
        if let pendingText { text += " + " + pendingText }
        if let returnText { text += " · " + returnText }
        return text
    }
}

/// What was spent, split the way it was paid.
public struct SpendDetail: Hashable, Sendable {
    /// Real dollars: AWS plus agents billed to a key.
    public var usd: Double
    public var budgetUSD: Double
    public var awsUSD: Double
    public var agentsAPIUSD: Double
    /// Tokens used on the user's own plan: counted, never in `usd`.
    public var includedTokens: Int
    /// "AWS $121 · agents included in your plan (2.4M tokens)".
    public var detail: String

    public init(
        usd: Double = 0, budgetUSD: Double = 0, awsUSD: Double = 0, agentsAPIUSD: Double = 0, includedTokens: Int = 0, detail: String = ""
    ) {
        self.usd = usd
        self.budgetUSD = budgetUSD
        self.awsUSD = awsUSD
        self.agentsAPIUSD = agentsAPIUSD
        self.includedTokens = includedTokens
        self.detail = detail
    }
}

/// "Next 30 days": what the schedules will start and what that will cost.
public struct ValueForecast: Hashable, Sendable {
    public var days: Int
    public var scheduledRuns: Int
    /// Scheduled optimizations whose runs cannot be counted ahead ("on every push").
    public var unforecast: Int
    public var expectedUSD: Double
    public var expectedAWSUSD: Double
    public var expectedAgentsAPIUSD: Double
    public var expectedIncludedTokens: Int
    /// The ceiling: spend stops at it.
    public var hardCapUSD: Double?
    /// "62 runs · ~$140 · cap $500".
    public var line: String

    public init(
        days: Int = 30, scheduledRuns: Int = 0, unforecast: Int = 0, expectedUSD: Double = 0, expectedAWSUSD: Double = 0,
        expectedAgentsAPIUSD: Double = 0, expectedIncludedTokens: Int = 0, hardCapUSD: Double? = nil, line: String = ""
    ) {
        self.days = days
        self.scheduledRuns = scheduledRuns
        self.unforecast = unforecast
        self.expectedUSD = expectedUSD
        self.expectedAWSUSD = expectedAWSUSD
        self.expectedAgentsAPIUSD = expectedAgentsAPIUSD
        self.expectedIncludedTokens = expectedIncludedTokens
        self.hardCapUSD = hardCapUSD
        self.line = line
    }

    /// What one scheduled run costs on average: the "≈$38" of "Create and Run Once"; nil with no scheduled runs.
    public var typicalRunUSD: Double? {
        scheduledRuns > 0 ? expectedUSD / Double(scheduledRuns) : nil
    }
}

/// One point of "Value delivered vs spend": cumulative real dollars and the yearly value counted by then.
public struct ValueChartPoint: Hashable, Sendable {
    public var at: Date
    public var spentUSD: Double
    public var deliveredUSDPerYear: Double
    /// Drawn as the dashed step.
    public var pendingUSDPerYear: Double
    public var readyUSDPerYear: Double

    public init(at: Date, spentUSD: Double, deliveredUSDPerYear: Double, pendingUSDPerYear: Double = 0, readyUSDPerYear: Double = 0) {
        self.at = at
        self.spentUSD = spentUSD
        self.deliveredUSDPerYear = deliveredUSDPerYear
        self.pendingUSDPerYear = pendingUSDPerYear
        self.readyUSDPerYear = readyUSDPerYear
    }
}

/// What the Overview shows when it has no numbers.
public enum OverviewPhase: String, Codable, Sendable {
    /// No optimization at all.
    case empty
    /// Optimizations exist but none has a verified win yet.
    case noWins = "no-wins"
    case wins
}

public struct EmptyStateText: Hashable, Sendable {
    public var title: String
    public var body: String

    public init(title: String, body: String) {
        self.title = title
        self.body = body
    }
}

/// The Overview's value part: the story, the spend split, the forecast, the chart and the empty state.
public struct ValueOverview: Hashable, Sendable {
    public var range: TimeRange
    public var phase: OverviewPhase
    public var emptyState: EmptyStateText?
    public var story: ValueStory
    public var spent: SpendDetail
    public var forecast: ValueForecast
    /// Oldest first.
    public var chart: [ValueChartPoint]

    public init(
        range: TimeRange, phase: OverviewPhase = .wins, emptyState: EmptyStateText? = nil, story: ValueStory, spent: SpendDetail,
        forecast: ValueForecast, chart: [ValueChartPoint]
    ) {
        self.range = range
        self.phase = phase
        self.emptyState = emptyState
        self.story = story
        self.spent = spent
        self.forecast = forecast
        self.chart = chart
    }

    /// The chart has something to draw: a win, delivered or not.
    public var hasChart: Bool { phase == .wins && chart.count > 1 }
}

/// The run that failed most recently and still needs a look ("1 run failed: CLI cold start").
public struct HomeFailure: Hashable, Sendable {
    public var optimizationID: String
    public var title: String
    public var detail: String

    public init(optimizationID: String, title: String, detail: String) {
        self.optimizationID = optimizationID
        self.title = title
        self.detail = detail
    }
}

/// The line above the composer on Home: the story and a few counts.
public struct HomeSummary: Hashable, Sendable {
    public var range: TimeRange
    /// No optimization yet: show the composer and no numbers.
    public var isEmpty: Bool
    public var story: ValueStory
    public var optimizations: Int
    public var openPullRequests: Int
    public var mergedPullRequests: Int
    public var failed: HomeFailure?

    public init(
        range: TimeRange, isEmpty: Bool = false, story: ValueStory, optimizations: Int = 0, openPullRequests: Int = 0,
        mergedPullRequests: Int = 0, failed: HomeFailure? = nil
    ) {
        self.range = range
        self.isEmpty = isEmpty
        self.story = story
        self.optimizations = optimizations
        self.openPullRequests = openPullRequests
        self.mergedPullRequests = mergedPullRequests
        self.failed = failed
    }

    /// "Spent · 90 days".
    public var spentCaption: String { "Spent · " + range.title }

    /// "3 optimizations · 2 PRs this week".
    public var countsLine: String {
        var parts = [Plural.count(optimizations, "optimization")]
        let prs = openPullRequests + mergedPullRequests
        if prs > 0 { parts.append(Plural.count(prs, "PR")) }
        return parts.joined(separator: " · ")
    }
}
