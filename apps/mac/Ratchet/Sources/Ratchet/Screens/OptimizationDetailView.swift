import RatchetKit
import SwiftUI

/// One optimization. While it runs: best so far, spend, time left, the staircase, the agents, the candidates and their
/// verdicts. Once it ended: the gain, the value, the pull request, what changed and the teardown receipt.
struct OptimizationDetailView: View {
    let id: String
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(\.openURL) private var openURL

    private var detail: OptimizationDetail? { store.details[id] }
    private var optimization: Optimization? { store.optimization(id: id) ?? detail?.optimization }

    var body: some View {
        Group {
            if let optimization {
                KitPage {
                    Button { router.optimizationID = nil } label: {
                        HStack(spacing: Space.xs) {
                            KitIconView(icon: .back, size: Dim.icon - 4, weight: 1.8)
                            Text("Optimizations")
                        }
                    }
                    .buttonStyle(.kit(.link))
                    header(optimization)
                    if optimization.status.isFinished {
                        resultBody(optimization)
                    } else {
                        liveBody(optimization)
                    }
                }
            } else {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: id) { await store.loadDetail(id: id) }
    }

    // MARK: Header

    private func header(_ optimization: Optimization) -> some View {
        PageHeader(optimization.title, subtitle: optimization.repo + " · " + optimization.branch) {
            StatusLabel(tone: optimization.status.tone, text: optimization.status.title)
            if optimization.status == .waiting {
                Button("Review environment") { router.reviewEnvironment(of: optimization, client: store.client) }
                    .buttonStyle(.kit(.primary))
            }
            if let number = optimization.prNumber {
                Button("Open pull request #\(number)") { openPullRequest(optimization, number) }
                    .buttonStyle(.kit(.primary))
            }
        }
    }

    private func openPullRequest(_ optimization: Optimization, _ number: Int) {
        if let url = URL(string: "https://github.com/\(optimization.repo)/pull/\(number)") { openURL(url) }
    }

    // MARK: Live

    @ViewBuilder
    private func liveBody(_ optimization: Optimization) -> some View {
        HeroRow {
            HeroNumber(
                label: "Best so far", value: optimization.best.map(optimization.metric.text) ?? "—",
                note: bestNote(optimization))
        } second: {
            HeroNumber(
                label: "Spent", value: Money.short(optimization.spentUSD),
                note: optimization.budgetUSD.map { "of " + Money.short($0) + " budget" } ?? "no budget set",
                progress: optimization.budgetFraction)
        } third: {
            HeroNumber(
                label: "Time left", value: optimization.secondsLeft.map(Elapsed.hm) ?? "—",
                note: "of " + String(format: "%g", optimization.timeLimitHours) + " h limit")
        }
        if optimization.status == .waiting {
            HStack(spacing: Space.sm) {
                StatusDot(tone: .wait)
                Text("Waiting on you: approve the environment and the agents carry on.")
                    .kitType(.body).foregroundStyle(Ink.textMuted)
            }
        }
        HStack(alignment: .top, spacing: Space.pageX) {
            KitSection("Best so far") { chart(optimization) }
                .frame(maxWidth: .infinity, alignment: .leading)
            KitSection("Agents") { agentList }
                .frame(width: 360, alignment: .leading)
        }
        KitSection("Candidates") { candidateTable(optimization) }
    }

    private func bestNote(_ optimization: Optimization) -> String {
        var parts: [String] = []
        if let change = optimization.change { parts.append(Percent.signed(change)) }
        parts.append("baseline " + optimization.metric.text(optimization.baseline))
        if let noise = optimization.noise { parts.append("noise " + Percent.noise(noise)) }
        return parts.joined(separator: " · ")
    }

    private var agentList: some View {
        HairlineList {
            ForEach(detail?.agents ?? []) { agent in
                HairlineRow {
                    StatusDot(tone: agent.state.tone)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(agent.name).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
                        Text(agent.activity).kitType(.meta).foregroundStyle(Ink.textMuted).lineLimit(2)
                        Text("Runs on \(agent.placement.title) · \(agent.evals) evals · \(agent.wins) wins")
                            .kitType(.caption).foregroundStyle(Ink.textFaint)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    AgentActions(agent: agent)
                }
            }
        }
    }

    // MARK: Result

    @ViewBuilder
    private func resultBody(_ optimization: Optimization) -> some View {
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
        HStack(alignment: .top, spacing: Space.pageX) {
            if (detail?.moves ?? []).isEmpty {
                KitSection("Result") {
                    Text("Nothing beat the noise, so there is nothing to merge. The sandbox is gone and nothing is billing.")
                        .kitType(.body).foregroundStyle(Ink.textMuted)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                KitSection("Best over time") { chart(optimization) }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            KitSection("What changed") { changeBlock(optimization) }
                .frame(width: 320, alignment: .leading)
        }
        KitSection("Candidates") { candidateTable(optimization) }
        if let receipt = detail?.receipt {
            KitSection("Teardown receipt") {
                ReceiptView(receipt: receipt)
            }
        }
    }

    private func gainNote(_ optimization: Optimization) -> String {
        guard let text = optimization.resultText else {
            return "nothing beat the noise" + (optimization.noise.map { " (" + Percent.noise($0) + ")" } ?? "")
        }
        return "Verified improvement · " + text
    }

    private func spentNote(_ optimization: Optimization) -> String {
        let agents = optimization.agentsCount == 1 ? "1 agent" : "\(optimization.agentsCount) agents"
        return Figure.wins(optimization.winsCount) + " from " + agents
    }

    private func changeBlock(_ optimization: Optimization) -> some View {
        VStack(alignment: .leading, spacing: Space.md) {
            if let detail, detail.filesChanged > 0 {
                HairlineList {
                    HairlineRow {
                        TitleStack(title: "Winning change", subtitle: "\(detail.filesChanged) files, squashed")
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Text("+\(detail.additions)").kitType(.mono).foregroundStyle(Ink.ok)
                        Text("−\(detail.deletions)").kitType(.mono).foregroundStyle(Ink.bad)
                    }
                    if let number = optimization.prNumber {
                        HairlineRow {
                            TitleStack(title: "Pull request #\(number)", subtitle: optimization.status == .merged ? "Merged" : "Open, waiting for you")
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

    // MARK: Shared

    private func chart(_ optimization: Optimization) -> some View {
        let points = stairPoints(optimization)
        return StaircaseChart(
            points: points, yFormat: optimization.metric.number, yDomain: yDomain(optimization, points),
            reference: optimization.baseline, previewLast: Store.isOverridden)
    }

    private func stairPoints(_ optimization: Optimization) -> [StairPoint] {
        let moves = (detail?.moves ?? []).sorted { $0.at < $1.at }
        let start = optimization.startedAt ?? moves.first?.at ?? Date()
        var points = [StairPoint(id: "start", date: start, value: optimization.baseline)]
        for move in moves {
            points.append(StairPoint(id: move.id, date: move.at, value: move.value,
                                     label: optimization.metric.text(move.value) + " · " + move.title))
        }
        let last = points.last ?? points[0]
        var end = last.date.addingTimeInterval(max(last.date.timeIntervalSince(start) * 0.15, 600))
        if optimization.status == .running, let left = optimization.secondsLeft {
            end = max(end, start.addingTimeInterval(optimization.timeLimitHours * 3600 - left))
        }
        points.append(StairPoint(id: "end", date: end, value: last.value))
        return points
    }

    private func yDomain(_ optimization: Optimization, _ points: [StairPoint]) -> ClosedRange<Double> {
        let values = points.map(\.value)
        let low = min(values.min() ?? 0, optimization.baseline)
        let high = max(values.max() ?? 1, optimization.baseline)
        let pad = max((high - low) * 0.12, 0.0001)
        return (low - pad)...(high + pad)
    }

    private func candidateTable(_ optimization: Optimization) -> some View {
        let candidates = detail?.candidates ?? []
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Space.lg) {
                columnTitle("Commit").frame(width: 76, alignment: .leading)
                columnTitle("Candidate").frame(maxWidth: .infinity, alignment: .leading)
                columnTitle("Verdict").frame(width: TableColumns.status, alignment: .leading)
                columnTitle(optimization.metric.name).frame(width: TableColumns.spent + Space.xl, alignment: .trailing)
            }
            .padding(.bottom, Space.sm + 2)
            .overlay(alignment: .bottom) { Hairline() }
            ForEach(candidates) { candidate in
                HStack(spacing: Space.lg) {
                    Text(candidate.id).kitType(.mono).foregroundStyle(Ink.textFaint)
                        .frame(width: 76, alignment: .leading)
                    TitleStack(title: candidate.title, subtitle: candidate.agent)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    StatusLabel(tone: candidate.verdict.tone, text: candidate.verdict.title)
                        .frame(width: TableColumns.status, alignment: .leading)
                    Text(candidate.value.map(optimization.metric.text) ?? "—")
                        .kitType(.mono).foregroundStyle(candidate.value == nil ? Ink.textFaint : Ink.text)
                        .frame(width: TableColumns.spent + Space.xl, alignment: .trailing)
                }
                .padding(.vertical, Dim.rowPadding - 2)
                .overlay(alignment: .bottom) { Hairline() }
            }
        }
    }

    private func columnTitle(_ text: String) -> some View {
        Text(text).kitType(.caption).foregroundStyle(Ink.textFaint)
    }
}

/// Pause or resume one agent, and send it a message.
private struct AgentActions: View {
    let agent: Agent
    @Environment(AppStore.self) private var store
    @State private var composing = false
    @State private var text = ""

    var body: some View {
        HStack(spacing: Space.sm) {
            Button("Message") { composing = true }
                .buttonStyle(.kit(.link, small: true))
                .popover(isPresented: $composing) { composer }
            Button(agent.state == .paused ? "Resume" : "Pause") {
                Task {
                    if agent.state == .paused { await store.resumeAgent(agent.id) } else { await store.pauseAgent(agent.id) }
                }
            }
            .buttonStyle(.kit(.quiet, small: true))
            .disabled(agent.state == .idle)
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            Text("Message to " + agent.name).kitType(.section).foregroundStyle(Ink.textMuted)
            TextField("Try caching the tax table first", text: $text)
                .kitField()
                .frame(width: 280)
                .onSubmit(send)
            HStack {
                Spacer()
                Button("Send", action: send)
                    .buttonStyle(.kit(.primary, small: true))
                    .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(Space.lg)
        .background(Ink.canvas)
    }

    private func send() {
        let note = text
        guard !note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        text = ""
        composing = false
        Task { await store.messageAgent(agent.id, text: note) }
    }
}
