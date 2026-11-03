import Charts
import RatchetKit
import SwiftUI

/// "Value delivered vs spend": the delivered value per year as a brass staircase, the pending value as a dashed step on top of it
/// ("+$2.9k pending if it merges"), and the spend as a quiet labelled line ("Spent $184"). Pending never joins the delivered line.
/// The chart has a text equivalent (`ValueOverview.chartSummary`) for VoiceOver.
struct ValueChart: View {
    let overview: ValueOverview

    var body: some View {
        let points = overview.chart
        let story = overview.story
        let top = max(points.map { $0.deliveredUSDPerYear + $0.pendingUSDPerYear }.max() ?? 1, points.map(\.spentUSD).max() ?? 1, 1)
        Chart {
            ForEach(Array(points.enumerated()), id: \.offset) { _, point in
                AreaMark(x: .value("Date", point.at), yStart: .value("Floor", 0), yEnd: .value("Delivered", point.deliveredUSDPerYear), series: .value("Series", "area"))
                    .interpolationMethod(.stepEnd)
                    .foregroundStyle(LinearGradient(
                        colors: [Ink.brassFill.opacity(0.18), Ink.brassFill.opacity(0)], startPoint: .top, endPoint: .bottom))
            }
            ForEach(Array(points.enumerated()), id: \.offset) { _, point in
                LineMark(x: .value("Date", point.at), y: .value("Delivered", point.deliveredUSDPerYear), series: .value("Series", "delivered"))
                    .interpolationMethod(.stepEnd)
                    .foregroundStyle(Ink.brass)
                    .lineStyle(StrokeStyle(lineWidth: 2, lineJoin: .round))
            }
            // The pending step: dashed, from what is delivered up to what would be if the open pull requests merge.
            if story.pending.usdPerYear > 0, let last = points.last {
                RuleMark(
                    x: .value("Date", last.at), yStart: .value("Delivered", last.deliveredUSDPerYear),
                    yEnd: .value("Pending", last.deliveredUSDPerYear + story.pending.usdPerYear))
                    .foregroundStyle(Ink.brass)
                    .lineStyle(StrokeStyle(lineWidth: Dim.markStroke, dash: [4, 3]))
                    .annotation(position: .top, alignment: .trailing, spacing: Space.xs) {
                        Text("+" + Money.short(story.pending.usdPerYear) + " pending if "
                             + (story.pending.pullRequests == 1 ? "its PR merges" : "the PRs merge"))
                            .kitType(.caption).foregroundStyle(Ink.brassText)
                    }
            }
            // The spend: a thin line with its own label, so it is never mistaken for value.
            ForEach(Array(points.enumerated()), id: \.offset) { _, point in
                LineMark(x: .value("Date", point.at), y: .value("Spent", point.spentUSD), series: .value("Series", "spent"))
                    .interpolationMethod(.monotone)
                    .foregroundStyle(Ink.textMuted)
                    .lineStyle(StrokeStyle(lineWidth: Dim.markStroke))
            }
            if let last = points.last {
                PointMark(x: .value("Date", last.at), y: .value("Spent", last.spentUSD))
                    .foregroundStyle(Ink.textMuted)
                    .symbolSize(30)
                    .annotation(position: .top, alignment: .trailing, spacing: Space.xs) {
                        Text("Spent " + Money.short(last.spentUSD)).kitType(.caption).foregroundStyle(Ink.textMuted)
                    }
            }
        }
        .chartYScale(domain: 0...(top * 1.12))
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 4)) { value in
                AxisGridLine().foregroundStyle(Ink.hairline)
                AxisValueLabel {
                    if let number = value.as(Double.self) {
                        Text(Money.axis(number)).kitType(.axisMono).foregroundStyle(Ink.textFaint)
                    }
                }
            }
        }
        .chartXAxis {
            AxisMarks(values: .automatic(desiredCount: 4)) { value in
                AxisValueLabel {
                    if let date = value.as(Date.self) {
                        Text(Elapsed.shortDate(date)).kitType(.axis).foregroundStyle(Ink.textFaint).lineLimit(1)
                    }
                }
            }
        }
        .padding(.top, Space.xl)
        .frame(height: Dim.chartHeight + Space.xxl)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Value delivered versus spend")
        .accessibilityValue(overview.chartSummary)
    }
}
