import AppKit
import RatchetKit
import SwiftUI

/// One optimization, in two tabs: the run (header with status, Stop…, Pause Schedule and Open PR; while it runs: best so far, spend,
/// the agents, the log in three views, the candidates, and the steering field; once it ended: the gain, the value, the pull request,
/// what changed and the teardown receipt) and its settings (editable rows, Archive).
struct OptimizationDetailView: View {
    let id: String
    @Environment(AppStore.self) var store
    @Environment(AppRouter.self) var router
    @Environment(ChatHub.self) var hub
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
                    RunHeader(optimization: optimization, detail: detail)
                        .gridProbe("header", area: "detail")
                    KitSegmented(
                        items: DetailTab.allCases, selection: Binding(get: { router.detailTab }, set: { router.detailTab = $0 }),
                        title: { $0.title },
                        help: { $0 == .run ? "The log, the agents and the result" : "Goal, agents, limits, schedule and value" })
                        .fixedSize()
                        .dimsWhenInactive()
                    switch router.detailTab {
                    case .run:
                        if optimization.status.isFinished {
                            resultBody(optimization)
                        } else {
                            liveBody(optimization)
                        }
                    case .settings:
                        OptimizationSettingsView(optimization: optimization, detail: detail)
                    }
                }
                // The steering field floats over the log while the optimization runs.
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    if router.detailTab == .run && !optimization.status.isFinished {
                        SteeringBar(optimizationID: id)
                    }
                }
            } else {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .gridArea("detail", inset: Space.pageX)
        .toolbar {
            ToolbarItemGroup {
                if let optimization, optimization.status.isFinished {
                    Button("Export Report…", systemImage: "square.and.arrow.up") {
                        Task { await OptimizationAction.exportReport(optimization, store: store) }
                    }
                    .help("Save the report as a markdown file (Shift-Command-E)")
                }
            }
        }
        .focusedSceneValue(\.optimizationSelection, [id])
        .task(id: id) {
            await store.optimizationStore.loadDetail(id: id)
            if store.optimizationStore.details[id]?.optimization.status.isFinished == true { await store.insight.loadInsights(id) }
            await hub.settings(for: id).load()
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
