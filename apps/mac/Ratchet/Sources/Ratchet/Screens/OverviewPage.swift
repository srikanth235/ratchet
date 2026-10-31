import RatchetKit
import SwiftUI

/// Spent, value locked in and return; the staircase of wins; what needs you; every optimization.
struct OverviewPage: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(AppNotifier.self) private var notifier

    var body: some View {
        KitPage {
            PageHeader("Overview") {
                RangeSwitch(selection: store.range) { range in
                    Task { await store.setRange(range) }
                }
                Button("New optimization") { router.startWizard(client: store.client) }
                    .buttonStyle(.kit(.primary))
            }
            .gridProbe("header", area: "overview")

            if notifier.shouldOfferPrompt && !store.optimizations.isEmpty {
                NotificationPrompt()
                    .gridProbe("notification-prompt", area: "overview")
            }

            hero
                .gridProbe("hero", area: "overview")

            HStack(alignment: .top, spacing: Space.pageX) {
                chartBlock.frame(maxWidth: .infinity, alignment: .leading)
                needsYouBlock.frame(width: 256, alignment: .leading)
            }
            .gridProbe("chart-and-needs", area: "overview")

            KitSection("Optimizations") {
                OptimizationTable(optimizations: store.optimizations) { router.open(optimization: $0.id) }
            }
            .gridProbe("table", area: "overview")
        }
        .gridArea("overview")
        .overlay {
            if !store.isLoaded && store.overview == nil {
                ProgressView().controlSize(.small)
            }
        }
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
        } else if item.kind == .approveEnvironment, let optimization = store.optimization(id: item.optimizationID) {
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
