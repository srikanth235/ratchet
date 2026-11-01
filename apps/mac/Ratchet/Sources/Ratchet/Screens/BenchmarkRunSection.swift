import Charts
import RatchetKit
import SwiftUI

/// Benchmarks of one optimization: the ratchet of a metric (with its noise band and a point per candidate), the distribution
/// of the bench samples, baseline against best, the recent evaluations of each agent, and every candidate with filters.
struct BenchmarkRunSection: View {
    @Environment(AppStore.self) private var store
    @State private var chosen: String?
    @State private var metric: String?
    @State private var filter: CandidateFilter = .all
    @State private var points: PointsMode = .all
    @State private var focus = SnapshotFocus.shared

    enum PointsMode: String, CaseIterable, Identifiable {
        case all, verifiedOnly
        var id: String { rawValue }
        var title: String { self == .all ? "All candidates" : "Verified only" }
    }

    /// The optimizations that have a baseline to show.
    private var runs: [Optimization] { store.optimizations.filter { $0.baseline != 0 } }

    private var runID: String? {
        if let chosen, runs.contains(where: { $0.id == chosen }) { return chosen }
        return runs.first?.id
    }

    var body: some View {
        KitSection("Run detail") {
            if let id = runID {
                VStack(alignment: .leading, spacing: Space.xl) {
                    picker(selected: id)
                    content(id)
                }
                .task(id: "\(id)|\(metric ?? "")") {
                    await store.loadBenchmark(id, metric: metric)
                }
            } else {
                Text("Start an optimization and its benchmark appears here.").kitType(.body).foregroundStyle(Ink.textMuted)
            }
        }
        .id("run")
        .onChange(of: focus.metric, initial: true) { _, forced in metric = forced }
    }

    private func picker(selected id: String) -> some View {
        HStack(spacing: Space.lg) {
            Picker("Optimization", selection: Binding(get: { id }, set: { chosen = $0; metric = nil; filter = .all })) {
                ForEach(runs) { run in Text(run.title).tag(run.id) }
            }
            .labelsHidden()
            .fixedSize()
            Spacer(minLength: Space.sm)
            if let series = store.insight.series[id], series.hasMetricSwitch {
                KitSwitch(items: series.options, selection: series.selected ?? series.options[0], title: { $0.label }) { choice in
                    metric = choice.isObjective ? nil : choice.key
                }
                .accessibilityLabel("Metric")
            }
        }
    }

    @ViewBuilder
    private func content(_ id: String) -> some View {
        if let series = store.insight.series[id] {
            let results = store.insight.results[id] ?? []
            header(series)
            if series.steps.isEmpty {
                Text("The chart appears once the baseline run finishes.").kitType(.body).foregroundStyle(Ink.textMuted)
            } else {
                RatchetSeriesChart(series: series, showRejected: points == .all)
            }
            HStack(alignment: .top, spacing: Space.pageX) {
                VStack(alignment: .leading, spacing: Space.sm) {
                    Text("Run distribution").kitType(.section).foregroundStyle(Ink.textMuted)
                    histogram(id, series: series, results: results)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                VStack(alignment: .leading, spacing: Space.sm) {
                    Text("Baseline vs best").kitType(.section).foregroundStyle(Ink.textMuted)
                    baselineVsBest(id)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            recentEvaluations(series: series, results: results)
            candidates(series: series, results: results)
        } else {
            ProgressView().controlSize(.small)
        }
    }

    // MARK: Header

    private func header(_ series: BenchmarkSeries) -> some View {
        let label = series.selected.map { $0.isObjective ? "" : " · " + $0.label } ?? ""
        let saved = series.improvement.map { MetricNumber.text(abs($0), unit: series.unit) + " better · " } ?? ""
        return HStack(alignment: .bottom, spacing: Space.lg) {
            HeroNumber(
                label: "Best verified\(label) · \(series.lowerIsBetter ? "lower" : "higher") is better",
                value: series.best.map(MetricNumber.text) ?? "—", unit: series.unit.isEmpty ? nil : series.unit,
                note: saved + Plural.count(series.locks, "lock"))
            Spacer(minLength: Space.sm)
            KitSwitch(items: PointsMode.allCases, selection: points, title: { $0.title }) { points = $0 }
        }
    }

    // MARK: Distribution

    @ViewBuilder
    private func histogram(_ id: String, series: BenchmarkSeries, results: [CandidateResult]) -> some View {
        let baseline = store.insight.insights[id]?.baselineSamples ?? []
        let best = results.filter { $0.movedRatchet && !$0.samples.isEmpty }.max { $0.at < $1.at }
        if !baseline.isEmpty, let best {
            SamplesHistogram(
                baseline: baseline, best: best.samples, unit: series.unit,
                baselineMarker: series.baseline, bestMarker: best.benchValue)
        } else {
            Text("Needs a baseline and at least one verified win.").kitType(.body).foregroundStyle(Ink.textMuted)
        }
    }

    @ViewBuilder
    private func baselineVsBest(_ id: String) -> some View {
        let insights = store.insight.insights[id]
        if let lines = insights?.compare, !lines.isEmpty {
            HairlineList {
                ForEach(lines) { line in
                    VStack(alignment: .leading, spacing: Space.xs) {
                        HStack(spacing: Space.sm) {
                            Text(line.label).kitType(.body).foregroundStyle(Ink.text).lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            Text(line.text(line.baseline)).kitType(.mono).foregroundStyle(Ink.textFaint)
                            Text("→").kitType(.mono).foregroundStyle(Ink.textFaint)
                            Text(line.text(line.best)).kitType(.mono).foregroundStyle(Ink.text)
                            Text(line.deltaText(decimals: 0)).kitType(.mono)
                                .foregroundStyle(line.isWorse ? Ink.bad : Ink.ok)
                                .frame(width: Space.section * 2, alignment: .trailing)
                        }
                        PairBars(baseline: line.baseline ?? 0, finalValue: line.best ?? 0, longest: max(line.baseline ?? 0, line.best ?? 0))
                    }
                    .padding(.vertical, Space.sm)
                    .overlay(alignment: .bottom) { Hairline() }
                    .accessibilityElement(children: .combine)
                }
            }
        } else {
            Text("Needs a baseline and at least one verified win.").kitType(.body).foregroundStyle(Ink.textMuted)
        }
    }

    // MARK: Recent evaluations

    @ViewBuilder
    private func recentEvaluations(series: BenchmarkSeries, results: [CandidateResult]) -> some View {
        let agents = Array(Set(results.map(\.agent))).sorted()
        if !agents.isEmpty {
            VStack(alignment: .leading, spacing: Space.md) {
                Text("Recent evaluations").kitType(.section).foregroundStyle(Ink.textMuted)
                HStack(alignment: .top, spacing: Space.pageX) {
                    ForEach(agents, id: \.self) { agent in
                        let own = results.filter { $0.agent == agent }.sorted { $0.at < $1.at }.suffix(24)
                        VStack(alignment: .leading, spacing: Space.sm) {
                            Text(agent).kitType(.meta).foregroundStyle(Ink.textFaint).lineLimit(1)
                            EvalBarsView(results: Array(own), best: series.best)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
        }
    }

    // MARK: Candidates

    private func candidates(series: BenchmarkSeries, results: [CandidateResult]) -> some View {
        let rows = filter.apply(to: results)
        return VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.lg) {
                Text("Candidates").kitType(.section).foregroundStyle(Ink.textMuted)
                Text("\(rows.count) shown · re-evaluated by the trusted runner").kitType(.meta).foregroundStyle(Ink.textFaint)
                Spacer(minLength: Space.sm)
                KitSwitch(items: CandidateFilter.allCases, selection: filter, title: { $0.title }) { filter = $0 }
            }
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: Space.md) {
                    head("Commit").frame(width: Space.section * 3, alignment: .leading)
                    head("Change").frame(maxWidth: .infinity, alignment: .leading)
                    head("Guardrails").frame(width: Space.section * 4, alignment: .leading)
                    head("Measured").frame(width: Space.section * 3, alignment: .trailing)
                    head("Confirm").frame(width: Space.section * 2 + Space.sm, alignment: .trailing)
                    head("vs baseline").frame(width: Space.section * 3, alignment: .trailing)
                    head("Verdict").frame(width: TableColumns.status - Space.lg, alignment: .leading)
                }
                .padding(.bottom, Space.sm + 2)
                .overlay(alignment: .bottom) { Hairline() }
                ForEach(rows) { row in
                    CandidateResultRow(row: row, unit: series.unit)
                }
                if rows.isEmpty {
                    Text("No candidates match.").kitType(.body).foregroundStyle(Ink.textMuted).padding(.vertical, Space.md)
                }
            }
        }
    }

    private func head(_ title: String) -> some View {
        Text(title).kitType(.caption).foregroundStyle(Ink.textFaint)
    }
}

private struct CandidateResultRow: View {
    let row: CandidateResult
    let unit: String

    var body: some View {
        HStack(spacing: Space.md) {
            Text(row.sha).kitType(.mono).foregroundStyle(Ink.textFaint)
                .frame(width: Space.section * 3, alignment: .leading)
            TitleStack(title: row.message, subtitle: row.note ?? row.agent)
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: Space.xs) {
                StatusDot(tone: row.result == .blocked ? .bad : .ok)
                Text(row.guardrailText).kitType(.meta)
                    .foregroundStyle(row.result == .blocked ? Ink.bad : Ink.textMuted).lineLimit(1)
            }
            .frame(width: Space.section * 4, alignment: .leading)
            Text(row.benchValue.map(MetricNumber.text) ?? "—").kitType(.mono).foregroundStyle(row.benchValue == nil ? Ink.textFaint : Ink.text)
                .frame(width: Space.section * 3, alignment: .trailing)
            Text(row.confirmValue.map(MetricNumber.text) ?? "—").kitType(.mono).foregroundStyle(Ink.textFaint)
                .frame(width: Space.section * 2 + Space.sm, alignment: .trailing)
            Text(versus).kitType(.mono).foregroundStyle(versusTone)
                .frame(width: Space.section * 3, alignment: .trailing)
            StatusLabel(tone: tone, text: row.result.title)
                .frame(width: TableColumns.status - Space.lg, alignment: .leading)
        }
        .padding(.vertical, Dim.rowPadding - 2)
        .overlay(alignment: .bottom) { Hairline() }
        .help(row.reasons.joined(separator: "\n"))
    }

    private var versus: String {
        guard let value = row.vsBaselinePercent else { return "—" }
        return (value >= 0 ? "+" : "−") + String(format: "%.1f", abs(value)) + "%"
    }

    private var versusTone: Color {
        (row.vsBaselinePercent ?? 0) > 0 ? Ink.ok : Ink.textFaint
    }

    private var tone: StatusTone {
        switch row.result {
        case .verified: .ok
        case .pending: .run
        case .rejected, .failedConfirm: .off
        case .blocked: .bad
        }
    }
}

// MARK: - Charts

/// The ratchet: best verified value as a brass staircase, a faint band of noise around the baseline, a point per candidate
/// (filled when it moved the ratchet, hollow when not).
struct RatchetSeriesChart: View {
    let series: BenchmarkSeries
    let showRejected: Bool

    var body: some View {
        let end = max(series.tMax, series.steps.last?.minutes ?? 1, 1)
        let line = series.steps + (series.steps.last.map { [BenchmarkSeries.Step(minutes: end, value: $0.value)] } ?? [])
        let shown = series.visiblePoints(showRejected: showRejected)
        Chart {
            if let baseline = series.baseline, let floor = series.noiseFloor {
                RectangleMark(
                    xStart: .value("From", 0.0), xEnd: .value("To", end),
                    yStart: .value("Low", baseline - floor), yEnd: .value("High", baseline + floor))
                    .foregroundStyle(Ink.brassFill.opacity(0.14))
            }
            if let baseline = series.baseline {
                RuleMark(y: .value("Baseline", baseline))
                    .foregroundStyle(Ink.hairlineStrong)
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [4, 4]))
            }
            ForEach(Array(line.enumerated()), id: \.offset) { _, step in
                LineMark(x: .value("Minutes", step.minutes), y: .value("Best", step.value))
                    .interpolationMethod(.stepEnd)
                    .foregroundStyle(Ink.brass)
                    .lineStyle(StrokeStyle(lineWidth: 2, lineJoin: .round))
            }
            ForEach(shown) { point in
                PointMark(x: .value("Minutes", point.minutes), y: .value("Measured", point.value))
                    .symbol {
                        if point.verified {
                            Circle().fill(Ink.brass).frame(width: Space.sm, height: Space.sm)
                        } else {
                            Circle().strokeBorder(Ink.textFaint, lineWidth: Dim.hairline).frame(width: Space.sm, height: Space.sm)
                        }
                    }
            }
        }
        .chartXScale(domain: 0...end)
        .chartYScale(domain: yDomain)
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 4)) { value in
                AxisGridLine().foregroundStyle(Ink.hairline)
                AxisValueLabel {
                    if let number = value.as(Double.self) {
                        Text(MetricNumber.text(number)).kitType(.mono).foregroundStyle(Ink.textFaint)
                    }
                }
            }
        }
        .chartXAxis {
            AxisMarks(values: .automatic(desiredCount: 5)) { value in
                AxisValueLabel {
                    if let minutes = value.as(Double.self) {
                        Text(Elapsed.axis(minutes * 60)).kitType(.caption).foregroundStyle(Ink.textFaint)
                    }
                }
            }
        }
        .frame(height: Dim.chartHeightDetail)
        .accessibilityLabel("Ratchet chart")
        .accessibilityValue("\(series.locks) steps, best \(series.best.map(MetricNumber.text) ?? "none")")
    }

    private var yDomain: ClosedRange<Double> {
        var values = series.steps.map(\.value) + series.points.map(\.value)
        if let baseline = series.baseline {
            values.append(baseline)
            if let floor = series.noiseFloor { values += [baseline - floor, baseline + floor] }
        }
        let low = values.min() ?? 0
        let high = values.max() ?? 1
        let pad = max((high - low) * 0.1, 0.0001)
        return (low - pad)...(high + pad)
    }
}

/// Baseline samples as an outline and the best verified commit's as a filled step, over one shared x range.
struct SamplesHistogram: View {
    let baseline: [Double]
    let best: [Double]
    let unit: String
    var baselineMarker: Double?
    var bestMarker: Double?

    private struct Bin: Identifiable {
        let id: String
        let series: String
        let x: Double
        let count: Int
    }

    var body: some View {
        if let range = Histogram.range(of: [baseline, best]) {
            let count = Histogram.binCount(forSamples: max(baseline.count, best.count))
            let low = range.lowerBound
            let span = range.upperBound - range.lowerBound == 0 ? 1 : range.upperBound - range.lowerBound
            let bins = steps("baseline", baseline, low, span, count) + steps("best", best, low, span, count)
            Chart {
                ForEach(bins.filter { $0.series == "best" }) { bin in
                    AreaMark(x: .value("Value", bin.x), yStart: .value("Floor", 0), yEnd: .value("Samples", bin.count),
                             series: .value("Series", "best-fill"))
                        .interpolationMethod(.stepEnd)
                        .foregroundStyle(Ink.brassFill.opacity(0.4))
                }
                ForEach(bins) { bin in
                    LineMark(x: .value("Value", bin.x), y: .value("Samples", bin.count), series: .value("Series", bin.series))
                        .interpolationMethod(.stepEnd)
                        .foregroundStyle(bin.series == "best" ? Ink.brass : Ink.textFaint)
                        .lineStyle(StrokeStyle(lineWidth: 2, lineJoin: .round))
                }
                if let baselineMarker {
                    RuleMark(x: .value("Baseline", baselineMarker))
                        .foregroundStyle(Ink.textMuted)
                        .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 4]))
                }
                if let bestMarker {
                    RuleMark(x: .value("Best", bestMarker))
                        .foregroundStyle(Ink.brass)
                        .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 4]))
                }
            }
            .chartXScale(domain: low...(low + span))
            .chartYAxis(.hidden)
            .chartXAxis {
                AxisMarks(values: .automatic(desiredCount: 5)) { value in
                    AxisValueLabel {
                        if let number = value.as(Double.self) {
                            Text(MetricNumber.text(number)).kitType(.mono).foregroundStyle(Ink.textFaint)
                        }
                    }
                }
            }
            .frame(height: Dim.chartHeight)
            .accessibilityLabel("Histogram of baseline against best")
            HStack(spacing: Space.lg) {
                key(Ink.textFaint, "Baseline · \(baseline.count) samples")
                key(Ink.brassFill, "Best · \(best.count) samples")
                if !unit.isEmpty { Text(unit).kitType(.caption).foregroundStyle(Ink.textFaint) }
            }
        }
    }

    /// The stepped outline of one series: a point per bin edge, and the right edge to close the last bin.
    private func steps(_ name: String, _ values: [Double], _ low: Double, _ span: Double, _ count: Int) -> [Bin] {
        let counts = Histogram.bins(of: values, lo: low, hi: low + span, count: count)
        var out = counts.enumerated().map { index, n in
            Bin(id: "\(name)-\(index)", series: name, x: low + span * Double(index) / Double(count), count: n)
        }
        out.append(Bin(id: "\(name)-end", series: name, x: low + span, count: counts.last ?? 0))
        return out
    }

    private func key(_ color: Color, _ text: String) -> some View {
        HStack(spacing: Space.xs) {
            RoundedRectangle(cornerRadius: 2, style: .continuous).fill(color).frame(width: Space.md, height: Space.sm)
            Text(text).kitType(.caption).foregroundStyle(Ink.textFaint)
        }
    }
}

/// An agent's recent evaluations as flat bars: green is verified, a short red stub is blocked, a dashed outline failed its
/// confirm run, grey was rejected. A dashed brass line marks the best verified value.
struct EvalBarsView: View {
    let results: [CandidateResult]
    let best: Double?

    private let total: CGFloat = Space.section * 2
    private let stub: CGFloat = 6

    var body: some View {
        let known = results.compactMap(\.benchValue)
        let floorValue = known.min() ?? 0
        let values = results.map { $0.benchValue ?? floorValue }
        let scale = Double(total - Space.lg)
        let heights = EvalBars.heights(of: values, height: scale)
        ZStack(alignment: .bottomLeading) {
            HStack(alignment: .bottom, spacing: Space.xs) {
                ForEach(Array(results.enumerated()), id: \.element.id) { index, result in
                    bar(result, height: CGFloat(heights[index]))
                }
            }
            if let best, let guide = EvalBars.guide(for: best, values: values, height: scale) {
                Rectangle().fill(Ink.brass.opacity(0.7)).frame(height: Dim.hairline)
                    .padding(.bottom, CGFloat(guide))
                    .accessibilityHidden(true)
            }
        }
        .frame(maxWidth: .infinity, minHeight: total, maxHeight: total, alignment: .bottomLeading)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Recent evaluations")
    }

    @ViewBuilder
    private func bar(_ result: CandidateResult, height: CGFloat) -> some View {
        let shape = RoundedRectangle(cornerRadius: 2, style: .continuous)
        switch result.result {
        case .verified:
            shape.fill(Ink.ok).frame(width: Space.sm, height: height)
        case .blocked:
            shape.fill(Ink.bad).frame(width: Space.sm, height: stub)
        case .failedConfirm:
            shape.strokeBorder(Ink.textFaint, style: StrokeStyle(lineWidth: 1, dash: [2, 2])).frame(width: Space.sm, height: height)
        case .rejected:
            shape.fill(Ink.hairlineStrong).frame(width: Space.sm, height: height)
        case .pending:
            shape.fill(Ink.brass.opacity(0.5)).frame(width: Space.sm, height: height)
        }
    }
}
