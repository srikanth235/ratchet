import RatchetKit
import Charts
import SwiftUI

/// One step of a staircase. `label` is set on real wins (they get a dot and a tooltip); the first and last points,
/// which only hold the line to the edges of the window, have none.
struct StairPoint: Identifiable, Equatable {
    let id: String
    let date: Date
    let value: Double
    var label: String?
}

/// How the x axis is labelled.
enum StairAxis: Equatable {
    /// Calendar dates ("Oct 31"), for charts over days.
    case dates
    /// Time since the run started ("0 m", "30 m", "1 h"), for one run.
    case elapsed(since: Date)
}

/// A step chart in brass: the value holds until the next win, then jumps. Hover (or click and drag) over the chart to
/// see the win nearest the pointer.
struct StaircaseChart: View {
    let points: [StairPoint]
    let yFormat: (Double) -> String
    /// Fixed vertical range, or nil to fit the points.
    var yDomain: ClosedRange<Double>?
    /// A dashed reference line (the baseline of a run).
    var reference: Double?
    var xAxis: StairAxis = .dates
    /// Show the tooltip of the last win without hovering (snapshots).
    var previewLast = false
    /// The Overview's small chart: shorter, with the reference's tiny axis labels.
    var compact = false

    @State private var selectedDate: Date?

    var body: some View {
        let domain = yDomain ?? fittedDomain
        let floor = domain.lowerBound
        Chart {
            ForEach(points) { point in
                AreaMark(x: .value("Date", point.date), yStart: .value("Floor", floor), yEnd: .value("Value", point.value))
                    .interpolationMethod(.stepEnd)
                    .foregroundStyle(LinearGradient(
                        colors: [Ink.brassFill.opacity(0.18), Ink.brassFill.opacity(0)], startPoint: .top, endPoint: .bottom))
            }
            ForEach(points) { point in
                LineMark(x: .value("Date", point.date), y: .value("Value", point.value))
                    .interpolationMethod(.stepEnd)
                    .foregroundStyle(Ink.brass)
                    .lineStyle(StrokeStyle(lineWidth: 2, lineJoin: .round))
            }
            ForEach(wins) { point in
                PointMark(x: .value("Date", point.date), y: .value("Value", point.value))
                    .foregroundStyle(Ink.brass)
                    .symbolSize(40)
            }
            if let reference {
                RuleMark(y: .value("Baseline", reference))
                    .foregroundStyle(Ink.hairlineStrong)
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [4, 4]))
            }
            if let shown = highlighted {
                RuleMark(x: .value("Selected", shown.date))
                    .foregroundStyle(Ink.hairlineStrong)
                    .lineStyle(StrokeStyle(lineWidth: 1))
                    .annotation(position: .top, spacing: Space.xs) {
                        ChartTooltip(text: shown.label ?? "")
                    }
            }
        }
        .chartXSelection(value: $selectedDate)
        .chartXScale(domain: xDomain)
        .chartYScale(domain: domain)
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 4)) { value in
                AxisGridLine().foregroundStyle(Ink.hairline)
                AxisValueLabel {
                    if let number = value.as(Double.self) {
                        Text(yFormat(number)).kitType(compact ? .axisMono : .mono).foregroundStyle(Ink.textFaint)
                    }
                }
            }
        }
        .chartXAxis {
            // Ticks sit inside the plot, so the last label is never cut off at the right edge.
            AxisMarks(values: xTicks) { value in
                AxisValueLabel {
                    if let date = value.as(Date.self) {
                        Text(xLabel(date)).kitType(compact ? .axis : .caption).foregroundStyle(Ink.textFaint).lineLimit(1)
                    }
                }
            }
        }
        .padding(.top, compact ? Space.lg : Space.xxl)
        .frame(height: compact ? Dim.chartHeight : Dim.chartHeightDetail)
        .accessibilityLabel("Chart")
        .accessibilityValue(summary)
    }

    private var xTicks: [Date] {
        let domain = xDomain
        let span = domain.upperBound.timeIntervalSince(domain.lowerBound)
        switch xAxis {
        case .dates:
            return ChartTicks.fractions(count: 4).map { domain.lowerBound.addingTimeInterval(span * $0) }
        case .elapsed(let start):
            let offset = domain.lowerBound.timeIntervalSince(start)
            return ChartTicks.elapsed(span: span + offset).map { start.addingTimeInterval($0) }
                .filter { domain.contains($0) }
        }
    }

    private func xLabel(_ date: Date) -> String {
        switch xAxis {
        case .dates: Elapsed.shortDate(date)
        case .elapsed(let start): Elapsed.axis(date.timeIntervalSince(start))
        }
    }

    private var wins: [StairPoint] { points.filter { $0.label != nil } }

    private var xDomain: ClosedRange<Date> {
        guard let first = points.first?.date, let last = points.last?.date, first < last else {
            let now = Date()
            return now...now.addingTimeInterval(1)
        }
        return first...last
    }

    private var fittedDomain: ClosedRange<Double> {
        let values = points.map(\.value) + (reference.map { [$0] } ?? [])
        let low = values.min() ?? 0
        let high = values.max() ?? 1
        if low >= high { return low...(low + 1) }
        return 0...(high * 1.06)
    }

    /// The win nearest the pointer, or the last win when previewing.
    private var highlighted: StairPoint? {
        if let selectedDate {
            return wins.min { abs($0.date.timeIntervalSince(selectedDate)) < abs($1.date.timeIntervalSince(selectedDate)) }
        }
        return previewLast ? wins.last : nil
    }

    private var summary: String {
        guard let last = points.last else { return "No data" }
        return "\(wins.count) steps, now \(yFormat(last.value))"
    }
}

private struct ChartTooltip: View {
    let text: String

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
        Text(text)
            .kitType(.caption)
            .foregroundStyle(Ink.text)
            .padding(.horizontal, Space.md - 2)
            .padding(.vertical, Space.sm - 1)
            .background(shape.fill(Ink.raised))
            .overlay(shape.strokeBorder(Ink.hairlineStrong, lineWidth: Dim.hairline))
            .fixedSize()
    }
}
