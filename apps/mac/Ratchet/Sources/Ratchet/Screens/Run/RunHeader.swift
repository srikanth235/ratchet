import RatchetKit
import SwiftUI

/// The header of an optimization (screens 2, 3, 19, 20): the name and where it runs, a status word with its shape, and the
/// actions: Run Now, Stop…, Pause Schedule (Pause Agent on a single agent), Open PR #n and a ⋯ menu with Archive… and Export Report….
/// Stop… asks first (the window's one confirmation, "Stop Checkout p95?").
struct RunHeader: View {
    let optimization: Optimization
    let detail: OptimizationDetail?
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ChatHub.self) private var hub
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    private var settings: OptimizationSettingsModel { hub.settings(for: optimization.id) }

    /// "Running", "Not started", "Merged"…; "Archived" once the schedule is stopped by Archive.
    private var statusWord: String { settings.isArchived ? "Archived" : optimization.status.title }

    private var subtitle: String {
        var parts = [optimization.repo + " · " + optimization.branch]
        if let started = optimization.startedAt { parts.append("started " + Elapsed.timeOfDay(started)) }
        if optimization.status == .running, let left = optimization.secondsLeft { parts.append(Elapsed.hm(left) + " left") }
        if optimization.spentUSD > 0 || optimization.budgetUSD != nil {
            let spent = Figure.spentOf(optimization.spentUSD, budget: optimization.budgetUSD)
            parts.append(spent.spent + (spent.budget.map { " of " + $0 } ?? ""))
        }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        // The actions sit beside the title when there is room, and under it when there is not.
        ViewThatFits(in: .horizontal) {
            PageHeader(optimization.title, subtitle: subtitle) { actions }
            VStack(alignment: .leading, spacing: Space.md) {
                PageHeader(optimization.title, subtitle: subtitle)
                actions
            }
        }
    }

    @ViewBuilder
    private var actions: some View {
        HStack(spacing: Space.sm) {
            StatusLabel.shaped(settings.isArchived ? .off : optimization.status.tone, statusWord)
                .padding(.trailing, Space.xs)
            if OptimizationAction.canRun(optimization) {
                Button("Run Now") { Task { await store.optimizationStore.runNow(optimization.id) } }
                    .buttonStyle(.kit(.quiet))
                    .disabled(!actionsEnabled)
                    .help("Start this optimization now (Shift-Command-R)")
            }
            if OptimizationAction.canStop(optimization) {
                Button("Stop…") { router.pendingStop = optimization.id }
                    .buttonStyle(.kit(.danger))
                    .disabled(!actionsEnabled)
                    .help("Stop this optimization. Wins found so far are kept. (Command-Period)")
            }
            pauseButton
            if OptimizationAction.canOpenPullRequest(optimization) {
                Button(optimization.prNumber.map { "Open PR #\($0)" } ?? "Create PR") {
                    Task { await OptimizationAction.openPullRequest(optimization, store: store) }
                }
                .buttonStyle(.kit(.quiet))
                .disabled(!actionsEnabled && optimization.prNumber == nil)
                .help("Open the pull request with the verified wins (Shift-Command-P)")
            }
            KitMoreMenu("More actions for \(optimization.title)") {
                Button("Archive…") { router.pendingArchive = optimization.id }
                    .disabled(!actionsEnabled || settings.isArchived)
                if optimization.status.isFinished {
                    Button("Export Report…") { Task { await OptimizationAction.exportReport(optimization, store: store) } }
                }
                Button("Copy Link") { OptimizationAction.copyLinks([optimization]) }
            }
        }
    }

    /// "Pause Agent" when one agent does the work, else "Pause Schedule"; each becomes its Resume.
    @ViewBuilder
    private var pauseButton: some View {
        let agents = detail?.agents ?? []
        if !optimization.status.isFinished, agents.count == 1, let agent = agents.first {
            Button(agent.state == .paused ? "Resume Agent" : "Pause Agent") {
                Task {
                    if agent.state == .paused { await store.optimizationStore.resumeAgent(agent.id) } else { await store.optimizationStore.pauseAgent(agent.id) }
                }
            }
            .buttonStyle(.kit(.quiet))
            .disabled(!actionsEnabled || agent.state == .idle)
            .help("Pause the one agent working on this, or let it carry on")
        } else if (settings.canPauseSchedule || settings.isSchedulePaused) && !settings.isArchived {
            Button(settings.pauseTitle) { Task { await settings.togglePause() } }
                .buttonStyle(.kit(.quiet))
                .disabled(!actionsEnabled)
                .help(settings.isSchedulePaused ? "Start running on its schedule again" : "Stop starting new runs on the schedule. Running work carries on.")
        }
    }
}
