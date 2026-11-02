import RatchetKit
import SwiftUI

/// Spent, value locked in and return; the staircase of wins; what needs you; every optimization.
struct OverviewPage: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(AppNotifier.self) private var notifier
    @State private var query = ""
    @FocusState private var searching: Bool

    /// The first load is under way (or snapshot mode says so): grey blocks stand where the numbers will be.
    private var isLoading: Bool {
        SnapshotFocus.shared.showsLoading || (!store.isLoaded && store.overview == nil)
    }

    private var isEmpty: Bool { store.isLoaded && store.optimizationStore.optimizations.isEmpty && !isLoading }

    var body: some View {
        KitPage {
            PageHeader("Overview") {
                if isLoading {
                    HStack(spacing: Space.sm) {
                        ProgressView().controlSize(.small)
                        Text("Loading your optimizations…").kitType(.meta).foregroundStyle(Ink.textMuted)
                    }
                }
            }
            .gridProbe("header", area: "overview")

            if isLoading {
                OverviewSkeleton()
                    .gridProbe("skeleton", area: "overview")
            } else if isEmpty {
                EmptyOverview()
                    .gridProbe("empty", area: "overview")
            } else {
                loaded
            }
        }
        .gridArea("overview", inset: Space.pageX)
        .toolbar {
            ToolbarItem {
                Picker("Range", selection: Binding(get: { store.range }, set: { range in Task { await store.setRange(range) } })) {
                    ForEach(TimeRange.allCases) { range in Text(range.title).tag(range) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .help("Show wins from the last 30 days, 90 days or all time")
            }
        }
        .searchable(text: $query, placement: .toolbar, prompt: "Search optimizations")
        .searchFocused($searching)
        .focusedSceneValue(\.findAction, FindAction { searching = true })
    }

    @ViewBuilder
    private var loaded: some View {
        let shown = OptimizationSearch.filter(store.optimizationStore.optimizations, query: query)
        if notifier.shouldOfferPrompt && !store.optimizationStore.optimizations.isEmpty {
            NotificationPrompt()
                .gridProbe("notification-prompt", area: "overview")
        }

        hero
            .gridProbe("hero", area: "overview")

        // The needs-you column sits beside the chart when there is room, and under it when there is not.
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: Space.pageX) {
                chartBlock.frame(minWidth: Space.section * 12, idealWidth: Space.section * 12, maxWidth: .infinity, alignment: .leading)
                needsYouBlock.frame(width: Layout.sideColumnNarrow - Space.section * 2, alignment: .leading)
            }
            VStack(alignment: .leading, spacing: Space.block) {
                chartBlock.frame(maxWidth: .infinity, alignment: .leading)
                needsYouBlock.frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .gridProbe("chart-and-needs", area: "overview")

        KitSection("Optimizations") {
            if shown.isEmpty {
                ContentUnavailableView.search(text: query)
                    .frame(maxWidth: .infinity)
            } else {
                OptimizationTable(optimizations: shown, sizesToRows: true)
            }
        }
        .gridProbe("table", area: "overview")
    }

    @ViewBuilder
    private var hero: some View {
        let overview = store.overview
        HeroRow {
            HeroNumber(
                label: "Spent", value: Money.short(overview?.spentUSD ?? 0),
                note: "of " + Money.short(overview?.budgetUSD ?? 0) + " across AWS and agent keys",
                progress: overview?.spentFraction ?? 0)
        } second: {
            HeroNumber(
                label: "Value locked in", value: Money.short(overview?.valueLockedPerYearUSD ?? 0), unit: "/ yr",
                note: "from " + Figure.wins(overview?.winsCount ?? 0) + " (estimated)")
        } third: {
            HeroNumber(
                label: "Return", value: overview?.returnMultiple.map(Figure.multiple) ?? "—",
                note: returnNote(overview))
        }
    }

    private func returnNote(_ overview: Overview?) -> String {
        guard let overview, let days = overview.paybackDays else { return "yearly value for every $1 spent" }
        return "yearly value for every $1 spent · paid back in " + Elapsed.days(days)
    }

    private var chartBlock: some View {
        KitSection("Value locked in, per year") {
            if let series = store.overview?.series, series.count > 1 {
                StaircaseChart(points: stairPoints(series), yFormat: Money.axis, previewLast: Store.isOverridden, compact: true)
            } else {
                Color.clear.frame(height: Dim.chartHeight)
            }
        }
    }

    private func stairPoints(_ series: [ValuePoint]) -> [StairPoint] {
        series.map { point in
            StairPoint(
                id: point.id, date: point.date, value: point.valuePerYearUSD,
                label: point.win.map { _ in Money.short(point.valuePerYearUSD) + " / yr · " + (point.optimizationTitle ?? "") })
        }
    }

    private func act(on item: NeedsYouItem) {
        if item.kind == .signIn {
            router.show(.connectors)
        } else if item.kind == .approveEnvironment, let optimization = store.optimizationStore.optimization(id: item.optimizationID) {
            router.reviewEnvironment(of: optimization, client: store.client)
        } else if !item.optimizationID.isEmpty {
            router.open(optimization: item.optimizationID)
        }
    }

    private var needsYouBlock: some View {
        KitSection("Needs you") {
            VStack(alignment: .leading, spacing: Space.md) {
                HairlineList {
                    ForEach(store.needsYou) { item in
                        HairlineRow {
                            StatusDot(tone: .wait)
                            TitleStack(title: item.title, subtitle: item.detail, subtitleLines: 2)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            Button(item.actionTitle) { act(on: item) }
                                .buttonStyle(.kit(.quiet, small: true))
                        }
                    }
                }
                Text(store.needsYou.isEmpty ? "Nothing is waiting on you." : "Nothing else is waiting on you.")
                    .kitType(.meta).foregroundStyle(Ink.textFaint)
            }
        }
    }
}
