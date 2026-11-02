import AppKit
import RatchetKit
import SwiftUI

/// One optimization. While it runs: best so far, spend, time left, the staircase, the agents, the candidates and their
/// verdicts. Once it ended: the gain, the value, the pull request, what changed and the teardown receipt.
struct OptimizationDetailView: View {
    let id: String
    @Environment(AppStore.self) var store
    @Environment(AppRouter.self) var router
    @Environment(\.openURL) var openURL
    @State var confirmingStop = false

    var detail: OptimizationDetail? { store.optimizationStore.details[id] }
    var optimization: Optimization? { store.optimizationStore.optimization(id: id) ?? detail?.optimization }

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
        .task(id: id) {
            await store.optimizationStore.loadDetail(id: id)
            if store.optimizationStore.details[id]?.optimization.status.isFinished == true { await store.insight.loadInsights(id) }
        }
    }

    // MARK: Header

    func header(_ optimization: Optimization) -> some View {
        PageHeader(optimization.title, subtitle: optimization.repo + " · " + optimization.branch) {
            StatusLabel(tone: optimization.status.tone, text: optimization.status.title)
            if optimization.status == .waiting {
                Button("Review environment") { router.reviewEnvironment(of: optimization, client: store.client) }
                    .buttonStyle(.kit(.primary))
            }
            if let number = optimization.prNumber {
                Button("Open pull request #\(number)") { openPullRequest(optimization, number) }
                    .buttonStyle(.kit(.primary))
            } else if optimization.status.isFinished && optimization.winsCount > 0 {
                Button("Create pull request") { Task { if let url = await store.optimizationStore.openPullRequest(id) { openURL(url) } } }
                    .buttonStyle(.kit(.primary))
            }
            if optimization.status.isFinished {
                Button("Export report") { Task { await exportReport() } }
                    .buttonStyle(.kit(.quiet))
            } else {
                Button("Stop…") { confirmingStop = true }
                    .buttonStyle(.kit(.quiet))
                    .confirmationDialog(
                        "Stop this optimization?", isPresented: $confirmingStop, titleVisibility: .visible
                    ) {
                        Button("Stop and revoke keys", role: .destructive) { Task { await store.optimizationStore.stopOptimization(id) } }
                        Button("Keep running", role: .cancel) {}
                    } message: {
                        Text("The agents stop, their keys are revoked and the sandbox is removed once nothing else uses it. Wins found so far are kept.")
                    }
            }
        }
    }

    /// Where the pull request is: the address the daemon gave, else GitHub's.
    func openPullRequest(_ optimization: Optimization, _ number: Int) {
        let address = optimization.prURL ?? "https://github.com/\(optimization.repo)/pull/\(number)"
        if let url = URL(string: address) { openURL(url) }
    }

    /// The markdown report, through the system save dialog.
    @MainActor
    func exportReport() async {
        guard let report = await store.optimizationStore.report(id) else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = report.fileName
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            try report.markdown.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            store.reportProblem(error.localizedDescription)
        }
    }

    // MARK: Shared

    func chart(_ optimization: Optimization) -> some View {
        let points = stairPoints(optimization)
        return StaircaseChart(
            points: points, yFormat: optimization.metric.number, yDomain: yDomain(optimization, points),
            reference: optimization.baseline, xAxis: .elapsed(since: runStart(optimization)),
            previewLast: Store.isOverridden)
    }

    /// Where the elapsed-time axis starts.
    func runStart(_ optimization: Optimization) -> Date {
        optimization.startedAt ?? (detail?.moves ?? []).map(\.at).min() ?? Date()
    }

    func stairPoints(_ optimization: Optimization) -> [StairPoint] {
        let moves = (detail?.moves ?? []).sorted { $0.at < $1.at }
        let start = runStart(optimization)
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

    func yDomain(_ optimization: Optimization, _ points: [StairPoint]) -> ClosedRange<Double> {
        let values = points.map(\.value)
        let low = min(values.min() ?? 0, optimization.baseline)
        let high = max(values.max() ?? 1, optimization.baseline)
        let pad = max((high - low) * 0.12, 0.0001)
        return (low - pad)...(high + pad)
    }
}
