import Foundation
import Observation

/// What the Overview draws.
public enum OverviewContent: Equatable, Sendable {
    case loading
    /// "No wins yet": a title, one sentence and the one button (New Optimization).
    case empty(EmptyStateText)
    case wins
}

/// The value story Home and the Overview share. It loads both screens' numbers together and says whether they agree (they must:
/// the daemon computes `story` once). Home's header, the Overview's hero, spend line, forecast and chart read from here.
@MainActor @Observable
public final class ValueStoryModel {
    public private(set) var home: HomeSummary?
    public private(set) var overview: ValueOverview?
    public private(set) var isLoading = false
    public private(set) var error: String?
    public var range: TimeRange

    @ObservationIgnored private let client: any ValueStoryClient

    public init(client: any ValueStoryClient, range: TimeRange = .days90) {
        self.client = client
        self.range = range
    }

    public func load() async {
        isLoading = true
        defer { isLoading = false }
        let range = self.range
        do {
            async let homeSummary = client.home(range: range)
            async let value = client.valueOverview(range: range)
            let (fresh, full) = try await (homeSummary, value)
            home = fresh
            overview = full
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// The story both screens show: Home's, else the Overview's.
    public var story: ValueStory? { home?.story ?? overview?.story }

    /// Home and the Overview tell the same story. False only while one of them is stale.
    public var screensAgree: Bool {
        guard let home, let overview else { return true }
        return home.story == overview.story
    }

    public var content: OverviewContent {
        guard let overview else { return .loading }
        if overview.phase == .wins { return .wins }
        return .empty(overview.emptyState ?? EmptyStateText(title: "No wins yet", body: "Start your first optimization."))
    }

    /// What one run costs, for "Create and Run Once · ≈$38".
    public var typicalRunCostUSD: Double? { overview?.forecast.typicalRunUSD }

    /// The footnote under the Overview's numbers; "Edit value" opens the Value row of an optimization.
    public static let footnote = "Estimated from each optimization's Value row."
    public static let footnoteLink = "Edit value"

    /// The spend line: "AWS $121 · agents included in your plan (2.4M tokens)".
    public var spendDetail: String? { overview?.spent.detail }
}
