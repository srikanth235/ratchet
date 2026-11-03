import RatchetKit
import SwiftUI

/// The value story: what was spent (and how, "included in your plan"), what was delivered and what is pending, the return, the
/// chart of value delivered against spend, the next 30 days, and every optimization with "pending" on the value of an open pull
/// request. It reads `ValueStoryModel`, the same numbers and words as Home (S-107, S-126).
struct OverviewPage: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(AppNotifier.self) private var notifier
    @Environment(ChatHub.self) private var hub
    @State private var query = ""
    @FocusState private var searching: Bool

    private var story: ValueStoryModel { hub.story }

    /// The first load is under way (or snapshot mode says so): grey blocks stand where the numbers will be.
    private var isLoading: Bool {
        SnapshotFocus.shared.showsLoading || !store.isLoaded || (story.overview == nil && story.error == nil)
    }

    var body: some View {
        KitPage {
            PageHeader("Overview") {
                HStack(spacing: Space.lg) {
                    if isLoading {
                        HStack(spacing: Space.sm) {
                            ProgressView().controlSize(.small)
                            Text("Loading your optimizations…").kitType(.meta).foregroundStyle(Ink.textMuted)
                        }
                    }
                    // The period belongs to the page's numbers, so it sits on the page, not in the window's toolbar.
                    RangeSwitch(selection: store.range) { range in Task { await store.setRange(range) } }
                        .help("Show wins from the last 30 days, 90 days or all time")
                }
            }
            .gridProbe("header", area: "overview")

            if isLoading {
                OverviewSkeleton()
                    .gridProbe("skeleton", area: "overview")
            } else if let overview = story.overview {
                switch story.content {
                case .loading:
                    OverviewSkeleton()
                case .empty(let text):
                    EmptyOverview(text: text)
                        .gridProbe("empty", area: "overview")
                case .wins:
                    loaded(overview)
                }
            } else if let error = story.error {
                failed(error)
            }
        }
        .gridArea("overview", inset: Space.pageX)
        .searchable(text: $query, placement: .toolbar, prompt: "Search optimizations")
        .searchFocused($searching)
        .focusedSceneValue(\.findAction, FindAction { searching = true })
        .task(id: store.range) {
            hub.story.range = store.range
            await hub.story.load()
        }
    }

    // MARK: Loaded

    @ViewBuilder
    private func loaded(_ overview: ValueOverview) -> some View {
        let shown = OptimizationSearch.filter(store.optimizationStore.optimizations, query: query)
        let story = overview.story
        if notifier.shouldOfferPrompt && !store.optimizationStore.optimizations.isEmpty {
            NotificationPrompt()
                .gridProbe("notification-prompt", area: "overview")
        }

        hero(overview)
            .gridProbe("hero", area: "overview")

        if !story.headline.isEmpty {
            Text("Value per year: \(Text(story.headline).font(KitType.bodyStrong.font))")
                .kitType(.body).foregroundStyle(Ink.text)
                .accessibilityElement(children: .combine)
                .gridProbe("story", area: "overview")
        }

        // The side column sits beside the chart when there is room, and under it when there is not.
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: Space.pageX) {
                chartBlock(overview).frame(minWidth: Space.section * 12, idealWidth: Space.section * 12, maxWidth: .infinity, alignment: .leading)
                sideColumn(overview).frame(width: Layout.sideColumnNarrow - Space.section * 2, alignment: .leading)
            }
            VStack(alignment: .leading, spacing: Space.block) {
                chartBlock(overview).frame(maxWidth: .infinity, alignment: .leading)
                sideColumn(overview).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .gridProbe("chart-and-side", area: "overview")

        KitSection("Return by optimization") {
            if shown.isEmpty {
                ContentUnavailableView.search(text: query)
                    .frame(maxWidth: .infinity)
            } else {
                OptimizationTable(optimizations: shown, sizesToRows: true)
            }
        }
        .gridProbe("table", area: "overview")

        footnote
            .gridProbe("footnote", area: "overview")
    }

    private func hero(_ overview: ValueOverview) -> some View {
        let story = overview.story
        return HeroRow {
            HeroNumber(
                label: "Spent", value: story.spentText,
                note: spentNote(overview),
                progress: overview.spent.budgetUSD > 0 ? overview.spent.usd / overview.spent.budgetUSD : 0)
        } second: {
            HeroNumber(label: "Delivered", value: story.deliveredText, unit: "/ yr", note: deliveredNote(story))
        } third: {
            HeroNumber(label: "Return", value: story.returnText ?? "—", note: returnNote(story))
        }
    }

    /// "of $500 cap · AWS $121 · agents included in your plan (2.4M tokens)".
    private func spentNote(_ overview: ValueOverview) -> String {
        var parts: [String] = []
        if overview.spent.budgetUSD > 0 { parts.append("of " + Money.short(overview.spent.budgetUSD) + " cap") }
        if !overview.spent.detail.isEmpty { parts.append(overview.spent.detail) }
        return parts.isEmpty ? "across AWS and agent keys" : parts.joined(separator: " · ")
    }

    /// "3 merged wins · +$2.9k pending in 1 PR".
    private func deliveredNote(_ story: ValueStory) -> String {
        var parts = [Plural.count(story.delivered.wins, "merged win")]
        if story.pending.usdPerYear > 0 {
            parts.append("+" + Money.short(story.pending.usdPerYear) + " pending in " + Plural.count(story.pending.pullRequests, "PR"))
        }
        return parts.joined(separator: " · ")
    }

    private func returnNote(_ story: ValueStory) -> String {
        guard let days = story.paybackDays else { return "yearly value for every $1 spent" }
        return "per $1 spent, yearly · paid back in " + Elapsed.days(days)
    }

    private func chartBlock(_ overview: ValueOverview) -> some View {
        KitSection("Value delivered vs spend") {
            if overview.hasChart {
                ValueChart(overview: overview)
            } else {
                Color.clear.frame(height: Dim.chartHeight)
            }
        }
    }

    // MARK: Side column

    private func sideColumn(_ overview: ValueOverview) -> some View {
        VStack(alignment: .leading, spacing: Space.block) {
            if overview.story.pending.usdPerYear > 0 { pendingCard(overview.story) }
            forecast(overview.forecast)
        }
    }

    /// "$2.9k / yr: 1 win in 1 open pull request. It counts once it merges." with the way to that pull request.
    private func pendingCard(_ story: ValueStory) -> some View {
        let open = store.optimizationStore.optimizations.first { $0.prNumber != nil && $0.status != .merged }
        return KitSection("Pending value") {
            VStack(alignment: .leading, spacing: Space.sm) {
                HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                    Text(Money.short(story.pending.usdPerYear)).kitType(.brand).foregroundStyle(Ink.text)
                    Text("/ yr").kitType(.meta).foregroundStyle(Ink.textMuted)
                }
                Text(Plural.count(story.pending.wins, "verified win") + " in " + Plural.count(story.pending.pullRequests, "open pull request")
                     + ". Counts once " + (story.pending.pullRequests == 1 ? "it merges." : "they merge."))
                    .kitType(.meta).foregroundStyle(Ink.textMuted)
                    .fixedSize(horizontal: false, vertical: true)
                if let open {
                    Button("Open PR #\(open.prNumber ?? 0)") { Task { await OptimizationAction.openPullRequest(open, store: store) } }
                        .buttonStyle(.kit(.quiet, small: true))
                }
            }
            .padding(Space.lg)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).fill(Ink.raised.opacity(0.5)))
            .overlay(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).strokeBorder(Ink.hairlineStrong, lineWidth: Dim.hairline))
            .accessibilityElement(children: .contain)
        }
    }

    private func forecast(_ forecast: ValueForecast) -> some View {
        KitSection("Next 30 days") {
            VStack(alignment: .leading, spacing: Space.sm) {
                HairlineList {
                    forecastRow("Scheduled runs", "\(forecast.scheduledRuns)")
                    forecastRow(
                        "Expected spend", "~" + Money.short(forecast.expectedUSD),
                        caption: forecast.expectedIncludedTokens > 0 ? "agents included in your plan" : nil)
                    if let cap = forecast.hardCapUSD { forecastRow("Hard cap", Money.short(cap)) }
                    if forecast.unforecast > 0 {
                        forecastRow("Not forecast", Plural.count(forecast.unforecast, "optimization"), caption: "they run on every push")
                    }
                }
            }
        }
    }

    private func forecastRow(_ label: String, _ value: String, caption: String? = nil) -> some View {
        HairlineRow(verticalPadding: Space.sm) {
            VStack(alignment: .leading, spacing: 0) {
                Text(label).kitType(.meta).foregroundStyle(Ink.textMuted)
                if let caption { Text(caption).kitType(.caption).foregroundStyle(Ink.textFaint) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Text(value).kitType(.mono).foregroundStyle(Ink.text)
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: Footnote and failure

    /// "Estimated from each optimization's Value row. Edit value": the link opens that row.
    private var footnote: some View {
        HStack(spacing: Space.xs) {
            Text(ValueStoryModel.footnote).kitType(.meta).foregroundStyle(Ink.textFaint)
            Button(ValueStoryModel.footnoteLink) { openValueRow() }
                .buttonStyle(.kit(.link))
                .help("Open the Value row of an optimization")
        }
    }

    /// The Value row of the optimization the numbers lean on most: the one with a pull request, else the first.
    private func openValueRow() {
        guard let target = ValueStoryModel.valueRowTarget(in: store.optimizationStore.optimizations) else { return }
        router.open(optimization: target.id)
        router.detailTab = .settings
        SnapshotFocus.shared.target = "value"
    }

    private func failed(_ message: String) -> some View {
        VStack(spacing: Space.md) {
            Text("Couldn't load the value story").kitType(.pageTitle).foregroundStyle(Ink.text)
            Text(message).kitType(.body).foregroundStyle(Ink.textMuted).multilineTextAlignment(.center)
            Button("Try Again") { Task { await hub.story.load() } }.buttonStyle(.kit(.quiet))
        }
        .frame(maxWidth: .infinity)
        .padding(.top, Space.block)
    }
}
