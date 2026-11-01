import RatchetKit
import SwiftUI

// What the Result page adds below the headline: where the time went, before and after, the winning change as a diff, and
// where the money went (by category, by agent, per win). Plain parts over the models in InsightModels.swift.

/// A thin two-bar pair: the baseline in a quiet tone, the final value in brass, both against the same scale.
struct PairBars: View {
    let baseline: Double
    let finalValue: Double
    let longest: Double

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            bar(baseline, color: Ink.off)
            bar(finalValue, color: Ink.brassFill)
        }
        .accessibilityHidden(true)
    }

    private func bar(_ value: Double, color: Color) -> some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule().fill(Ink.track)
                Capsule().fill(color).frame(width: proxy.size.width * min(max(value / max(longest, 1e-9), 0), 1))
            }
        }
        .frame(height: Dim.bar * 2)
    }
}

/// "Where the time went": the evaluator's spans, baseline against the best verified commit.
struct SpansSection: View {
    let spans: [SpanTiming]
    let metricLabel: String

    var body: some View {
        let longest = SpanTiming.longest(in: spans)
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.lg) {
                Text(metricLabel + " request, by span").kitType(.meta).foregroundStyle(Ink.textFaint)
                Spacer(minLength: Space.sm)
                legend(Ink.off, "Baseline")
                legend(Ink.brassFill, "Final")
            }
            HairlineList {
                ForEach(spans) { span in
                    HairlineRow(verticalPadding: Space.md) {
                        Text(span.name).kitType(.mono).foregroundStyle(Ink.text)
                            .frame(width: Space.section * 3, alignment: .leading)
                        PairBars(baseline: span.baselineMS, finalValue: span.bestMS, longest: longest)
                            .frame(maxWidth: .infinity)
                        HStack(spacing: Space.xs) {
                            Text(Self.ms(span.baselineMS)).foregroundStyle(Ink.textFaint)
                            Text("→").foregroundStyle(Ink.textFaint)
                            Text(Self.ms(span.bestMS)).foregroundStyle(Ink.text)
                        }
                        .kitType(.mono)
                        .frame(width: Space.section * 6, alignment: .trailing)
                    }
                }
            }
            if let top = SpanTiming.biggestDrop(in: spans) {
                Text("Biggest single drop: \(top.name) fell from \(Self.ms(top.baselineMS)) to \(Self.ms(top.bestMS)). Spans are timed by the service itself; the ratchet judges only the objective.")
                    .kitType(.meta).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .contain)
    }

    private func legend(_ color: Color, _ text: String) -> some View {
        HStack(spacing: Space.xs) {
            Capsule().fill(color).frame(width: Space.lg, height: Dim.bar * 2)
            Text(text).kitType(.caption).foregroundStyle(Ink.textFaint)
        }
    }

    static func ms(_ value: Double) -> String {
        String(format: "%.0f ms", value)
    }
}

/// "Before and after": the objective and the metrics next to it. A metric outside the objective's family is a guardrail:
/// it shows "ok" because the verified commit only had to pass it.
struct CompareSection: View {
    let lines: [CompareLine]
    let family: Set<String>
    let objective: Optimization

    var body: some View {
        if lines.isEmpty {
            VStack(alignment: .leading, spacing: Space.md) {
                HairlineList {
                    HairlineRow(verticalPadding: Space.md) {
                        Text(objective.metric.name).kitType(.body).foregroundStyle(Ink.text)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Text(objective.metric.text(objective.baseline)).kitType(.mono).foregroundStyle(Ink.textFaint)
                        Text("→").kitType(.mono).foregroundStyle(Ink.textFaint)
                        Text(objective.best.map(objective.metric.text) ?? "—").kitType(.mono).foregroundStyle(Ink.text)
                    }
                }
                Text("The other metrics compare once there is a verified win.").kitType(.meta).foregroundStyle(Ink.textFaint)
            }
        } else {
            HairlineList {
                ForEach(lines) { line in
                    HairlineRow(verticalPadding: Space.md) {
                        Text(line.label).kitType(.body).foregroundStyle(Ink.text).lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Text(line.text(line.baseline)).kitType(.mono).foregroundStyle(Ink.textFaint)
                        Text("→").kitType(.mono).foregroundStyle(Ink.textFaint)
                        Text(line.text(line.best)).kitType(.mono).foregroundStyle(Ink.text)
                        Text(delta(line)).kitType(.mono).foregroundStyle(tone(line))
                            .frame(width: Space.section * 3, alignment: .trailing)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
        }
    }

    private func delta(_ line: CompareLine) -> String {
        guard line.isGuardrail(family: family) else { return line.deltaText() }
        return line.deltaPercent == nil ? "ok" : line.deltaText() + " ok"
    }

    private func tone(_ line: CompareLine) -> Color {
        if line.isGuardrail(family: family) { return Ink.textFaint }
        return line.isWorse ? Ink.bad : Ink.ok
    }
}

/// "Winning change": the files of the squashed diff and the first hunk of the biggest one.
struct WinningChangeSection: View {
    let change: WinningChange
    var onOpenPullRequest: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.lg) {
                Text(change.headline).kitType(.meta).foregroundStyle(Ink.textFaint)
                Spacer(minLength: Space.sm)
                if change.unavailable == nil {
                    HStack(spacing: Space.sm) {
                        Text(Plural.count(change.files.count, "file")).foregroundStyle(Ink.textMuted)
                        Text("+\(change.additions)").foregroundStyle(Ink.ok)
                        Text("−\(change.deletions)").foregroundStyle(Ink.bad)
                    }
                    .kitType(.mono)
                }
            }
            if let why = change.unavailable {
                Text(why).kitType(.body).foregroundStyle(Ink.textMuted)
            } else {
                HairlineList {
                    ForEach(change.files) { file in
                        HairlineRow(verticalPadding: Space.sm) {
                            Text(file.path).kitType(.mono).foregroundStyle(Ink.text).lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            if file.binary {
                                Text("binary").kitType(.mono).foregroundStyle(Ink.textFaint)
                            } else {
                                HStack(spacing: Space.sm) {
                                    if file.additions > 0 { Text("+\(file.additions)").foregroundStyle(Ink.ok) }
                                    if file.deletions > 0 { Text("−\(file.deletions)").foregroundStyle(Ink.bad) }
                                }
                                .kitType(.mono)
                            }
                        }
                    }
                }
                if let path = change.excerptPath, !change.excerpt.isEmpty {
                    DiffExcerptView(path: path, lines: change.excerpt)
                }
            }
        }
        .accessibilityElement(children: .contain)
    }
}

/// The first hunk of a file: additions and removals tinted, the hunk header quiet.
struct DiffExcerptView: View {
    let path: String
    let lines: [DiffLine]

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
        ScrollView(.horizontal) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                    Text(line.rendered(path: path))
                        .kitType(.mono)
                        .foregroundStyle(color(line.kind))
                        .padding(.horizontal, Space.md)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(fill(line.kind))
                        .fixedSize(horizontal: true, vertical: false)
                }
            }
            .padding(.vertical, Space.sm)
        }
        .background(shape.fill(Ink.raised.opacity(0.4)))
        .clipShape(shape)
        .overlay(shape.strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
        .accessibilityLabel("Excerpt of \(path)")
    }

    private func color(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .add: Ink.ok
        case .del: Ink.bad
        case .hunk: Ink.textFaint
        case .ctx: Ink.textMuted
        }
    }

    private func fill(_ kind: DiffLineKind) -> Color {
        switch kind {
        case .add: Ink.ok.opacity(0.08)
        case .del: Ink.bad.opacity(0.08)
        default: .clear
        }
    }
}

/// Where the money went: a stacked bar by category, then the agents with their share and what each win cost.
struct SpendSection: View {
    let spend: SpendBreakdown

    private func tint(_ index: Int) -> Color {
        let tints: [Color] = [
            Ink.brassFill, Ink.brassFill.opacity(0.7), Ink.brassFill.opacity(0.45), Ink.off, Ink.hairlineStrong,
        ]
        return tints[index % tints.count]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
                Text(Money.exact(spend.totalUSD)).kitType(.monoStrong).foregroundStyle(Ink.text)
                Text(spend.budgetUSD > 0 ? "of " + Money.short(spend.budgetUSD) : "no budget set")
                    .kitType(.meta).foregroundStyle(Ink.textFaint)
                Spacer(minLength: Space.sm)
                Text(spend.perWinUSD.map { Money.exact($0) + " per verified win" } ?? "no verified win")
                    .kitType(.meta).foregroundStyle(Ink.textMuted)
            }
            stackedBar
            VStack(alignment: .leading, spacing: Space.sm) {
                ForEach(Array(spend.categories.enumerated()), id: \.element.id) { index, line in
                    HStack(spacing: Space.sm) {
                        Circle().fill(tint(index)).frame(width: Dim.dot, height: Dim.dot)
                        Text(line.label).kitType(.body).foregroundStyle(Ink.text)
                        Spacer(minLength: Space.sm)
                        Text(Money.exact(line.usd)).kitType(.mono).foregroundStyle(Ink.textMuted)
                    }
                }
            }
            .accessibilityLabel("Spend by category")
            Text(spend.reconciliationNote).kitType(.meta).foregroundStyle(Ink.textFaint)
        }
    }

    private var stackedBar: some View {
        let fractions = spend.categoryFractions
        return GeometryReader { proxy in
            HStack(spacing: 1) {
                ForEach(Array(fractions.enumerated()), id: \.offset) { index, fraction in
                    Rectangle().fill(tint(index)).frame(width: max(proxy.size.width * fraction - 1, 0))
                }
            }
            .clipShape(Capsule())
        }
        .frame(height: Space.sm)
        .background(Capsule().fill(Ink.track))
        .accessibilityHidden(true)
    }
}

/// "By agent": each agent's spend with its wins and what one win cost.
struct AgentSpendSection: View {
    let spend: SpendBreakdown

    var body: some View {
        if spend.agents.isEmpty {
            Text("No spend recorded.").kitType(.body).foregroundStyle(Ink.textMuted)
        } else {
            VStack(alignment: .leading, spacing: Space.lg) {
                ForEach(spend.agents) { agent in
                    VStack(alignment: .leading, spacing: Space.xs) {
                        HStack {
                            Text(title(agent)).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
                            Spacer(minLength: Space.sm)
                            Text(Money.exact(agent.usd)).kitType(.mono).foregroundStyle(Ink.text)
                        }
                        ThinBar(fraction: spend.fraction(of: agent))
                        Text(agent.perWinUSD.map { Money.exact($0) + " per win" } ?? "no wins")
                            .kitType(.caption).foregroundStyle(Ink.textFaint)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
        }
    }

    private func title(_ agent: AgentSpend) -> String {
        agent.isShared ? agent.name : agent.name + " · " + Plural.count(agent.wins, "win")
    }
}
