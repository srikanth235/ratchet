import AppKit
import RatchetKit
import SwiftUI

extension OptimizationDetailView {
    // MARK: Result

    @ViewBuilder
    func resultBody(_ optimization: Optimization) -> some View {
        HeroRow {
            HeroNumber(
                label: "Gain", value: optimization.change.map(Percent.signed) ?? "—",
                note: gainNote(optimization))
        } second: {
            HeroNumber(
                label: "Value locked in", value: optimization.valuePerYearUSD.map(Money.short) ?? "—", unit: "/ yr",
                note: optimization.valuePerYearUSD == nil ? "no verified win" : "estimated")
        } third: {
            HeroNumber(
                label: "Spent", value: Money.short(optimization.spentUSD),
                note: spentNote(optimization), progress: optimization.budgetFraction)
        }
        // What changed sits beside the chart when there is room, and under it when there is not.
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: Space.pageX) {
                resultMain(optimization)
                    .frame(minWidth: Space.section * 12, idealWidth: Space.section * 12, maxWidth: .infinity, alignment: .leading)
                KitSection("What changed") { changeBlock(optimization) }
                    .frame(width: Layout.sideColumnNarrow, alignment: .leading)
            }
            VStack(alignment: .leading, spacing: Space.block) {
                resultMain(optimization)
                    .frame(maxWidth: .infinity, alignment: .leading)
                KitSection("What changed") { changeBlock(optimization) }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        if let insights = store.insight.model(for: id).insights {
            resultInsights(insights, optimization)
        }
        KitSection("Candidates") { CandidateTable(optimization: optimization, candidates: detail?.candidates ?? []) }
        if let receipt = detail?.receipt {
            KitSection("Teardown receipt") {
                ReceiptView(receipt: receipt)
            }
        }
    }

    /// Where the time went, before and after, the winning change as a diff, and where the money went.
    @ViewBuilder
    func resultInsights(_ insights: RunInsights, _ optimization: Optimization) -> some View {
        HStack(alignment: .top, spacing: Space.pageX) {
            if let spans = insights.spans, !spans.isEmpty {
                KitSection("Where the time went") { SpansSection(spans: spans, metricLabel: optimization.metric.name) }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            KitSection("Before and after") {
                CompareSection(lines: insights.compare, family: insights.family, objective: optimization)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .id("time")
        if let change = insights.change, change.commits > 0 {
            KitSection("Winning change") { WinningChangeSection(change: change) }
                .id("change")
        }
        if let spend = insights.spend {
            HStack(alignment: .top, spacing: Space.pageX) {
                KitSection("Spend") { SpendSection(spend: spend) }
                    .frame(maxWidth: .infinity, alignment: .leading)
                KitSection("By agent") { AgentSpendSection(spend: spend) }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .id("spend")
        }
    }

    /// The chart of a finished run, or why there is none.
    @ViewBuilder
    func resultMain(_ optimization: Optimization) -> some View {
        if (detail?.moves ?? []).isEmpty {
            KitSection("Result") {
                Text("Nothing beat the noise, so there is nothing to merge. The sandbox is gone and nothing is billing.")
                    .kitType(.body).foregroundStyle(Ink.textMuted)
            }
        } else {
            KitSection("Best over time") { chart(optimization) }
        }
    }

    func gainNote(_ optimization: Optimization) -> String {
        guard let text = optimization.resultText else {
            return "nothing beat the noise" + (optimization.noise.map { " (" + Percent.noise($0) + ")" } ?? "")
        }
        return "Verified improvement · " + text
    }

    func spentNote(_ optimization: Optimization) -> String {
        let agents = Plural.count(optimization.agentsCount, "agent")
        return Figure.wins(optimization.winsCount) + " from " + agents
    }

    func changeBlock(_ optimization: Optimization) -> some View {
        VStack(alignment: .leading, spacing: Space.md) {
            if let detail, detail.filesChanged > 0 {
                HairlineList {
                    HairlineRow {
                        TitleStack(title: "Winning change", subtitle: Plural.count(detail.filesChanged, "file") + ", squashed")
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Text("+\(detail.additions)").kitType(.mono).foregroundStyle(Ink.ok)
                        Text("−\(detail.deletions)").kitType(.mono).foregroundStyle(Ink.bad)
                    }
                    if let number = optimization.prNumber {
                        HairlineRow {
                            TitleStack(title: "Pull request #\(number)", subtitle: optimization.status == .merged ? "Merged" : "Open · counts once it merges")
                                .frame(maxWidth: .infinity, alignment: .leading)
                            Button("Open") { openPullRequest(optimization, number) }
                                .buttonStyle(.kit(.quiet, small: true))
                        }
                    }
                }
            } else {
                Text("No change to merge.").kitType(.body).foregroundStyle(Ink.textMuted)
            }
        }
    }
}
