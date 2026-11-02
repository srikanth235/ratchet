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
    @Environment(\.daemonActionsEnabled) var actionsEnabled

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
                        .gridProbe("header", area: "detail")
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
        .gridArea("detail", inset: Space.pageX)
        .toolbar {
            // Actions on the open optimization: the same as the Optimization menu and the table's context menu.
            ToolbarItemGroup {
                if let optimization {
                    if OptimizationAction.canRun(optimization) {
                        Button("Run Now", systemImage: "play") { Task { await store.optimizationStore.runNow(id) } }
                            .disabled(!actionsEnabled)
                            .help("Start this optimization now (Shift-Command-R)")
                    }
                    if OptimizationAction.canOpenPullRequest(optimization) {
                        Button(optimization.prNumber.map { "Open Pull Request #\($0)" } ?? "Create Pull Request", systemImage: "arrow.triangle.pull") {
                            Task { await OptimizationAction.openPullRequest(optimization, store: store) }
                        }
                        .disabled(!actionsEnabled && optimization.prNumber == nil)
                        .help("Open the pull request with the verified wins (Shift-Command-P)")
                    }
                    if optimization.status.isFinished {
                        Button("Export Report…", systemImage: "square.and.arrow.up") {
                            Task { await OptimizationAction.exportReport(optimization, store: store) }
                        }
                        .help("Save the report as a markdown file (Shift-Command-E)")
                    } else {
                        Button("Stop…", systemImage: "stop.circle") { router.pendingStop = id }
                            .disabled(!actionsEnabled)
                            .help("Stop this optimization (Command-Period)")
                    }
                }
            }
        }
        .focusedSceneValue(\.optimizationSelection, [id])
        .task(id: id) {
            await store.optimizationStore.loadDetail(id: id)
            if store.optimizationStore.details[id]?.optimization.status.isFinished == true { await store.insight.loadInsights(id) }
        }
    }

    // MARK: Header

    /// The title, where it runs and its status word. The actions are in the toolbar.
    func header(_ optimization: Optimization) -> some View {
        PageHeader(optimization.title, subtitle: optimization.repo + " · " + optimization.branch) {
            StatusLabel(tone: optimization.status.tone, text: optimization.status.title)
            if optimization.status == .waiting {
                Button("Review environment") { router.reviewEnvironment(of: optimization, client: store.client) }
                    .buttonStyle(.kit(.primary))
                    .disabled(!actionsEnabled)
            }
        }
    }

    /// Where the pull request is: the address the daemon gave, else GitHub's.
    func openPullRequest(_ optimization: Optimization, _ number: Int) {
        NSWorkspace.shared.open(OptimizationAction.link(for: optimization))
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
