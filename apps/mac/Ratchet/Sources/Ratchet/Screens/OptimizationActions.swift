import AppKit
import RatchetKit
import SwiftUI

/// What the table's context menu, the Optimization menu and the detail toolbar do to an optimization. One place, so a menu
/// item and its button cannot disagree. Stop never acts at once: it asks `router.pendingStop`, and the window shows the one
/// confirmation ("Stop Checkout p95?").
enum OptimizationAction {
    /// "Run Now": an optimization that has not started.
    static func canRun(_ optimization: Optimization) -> Bool { optimization.status == .waiting }

    static func canStop(_ optimization: Optimization) -> Bool { !optimization.status.isFinished }

    /// A pull request exists, or the verified wins can become one.
    static func canOpenPullRequest(_ optimization: Optimization) -> Bool {
        optimization.prNumber != nil || (optimization.status.isFinished && optimization.winsCount > 0)
    }

    /// The pull request when there is one, else the `ratchet://` link that opens the optimization in the app.
    static func link(for optimization: Optimization) -> URL {
        if let address = optimization.prURL, let url = URL(string: address) { return url }
        if let number = optimization.prNumber, let url = URL(string: "https://github.com/\(optimization.repo)/pull/\(number)") {
            return url
        }
        return URL(string: "ratchet://optimization/\(optimization.id)") ?? URL(filePath: "/")
    }

    static func copyLinks(_ optimizations: [Optimization]) {
        let text = optimizations.map { link(for: $0).absoluteString }.joined(separator: "\n")
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    /// Opens the pull request in the browser; creates it first when only the wins exist.
    static func openPullRequest(_ optimization: Optimization, store: AppStore) async {
        if optimization.prNumber != nil {
            NSWorkspace.shared.open(link(for: optimization))
        } else if let url = await store.optimizationStore.openPullRequest(optimization.id) {
            NSWorkspace.shared.open(url)
        }
    }

    /// The markdown report, through the system save dialog.
    @MainActor
    static func exportReport(_ optimization: Optimization, store: AppStore) async {
        guard let report = await store.optimizationStore.report(optimization.id) else { return }
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
}

/// The context menu of a row (or a selection of rows). Items that do not apply are left out; every one is also in the
/// Optimization menu. No shortcuts are shown here: they belong to the menu bar.
struct OptimizationMenuItems: View {
    let ids: [String]
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    private var optimizations: [Optimization] { ids.compactMap { store.optimizationStore.optimization(id: $0) } }

    var body: some View {
        let chosen = optimizations
        if chosen.count == 1, let optimization = chosen.first {
            Button("Open") { router.open(optimization: optimization.id) }
            Divider()
            if OptimizationAction.canRun(optimization) {
                Button("Run Now") { Task { await store.optimizationStore.runNow(optimization.id) } }
                    .disabled(!actionsEnabled)
            }
            if OptimizationAction.canStop(optimization) {
                Button("Stop…", role: .destructive) { router.pendingStop = optimization.id }
                    .disabled(!actionsEnabled)
            }
            if OptimizationAction.canOpenPullRequest(optimization) {
                Button("Open Pull Request") { Task { await OptimizationAction.openPullRequest(optimization, store: store) } }
                    .disabled(!actionsEnabled && optimization.prNumber == nil)
            }
            Divider()
            Button("Copy Link") { OptimizationAction.copyLinks([optimization]) }
        } else if chosen.count > 1 {
            Button("Copy Links") { OptimizationAction.copyLinks(chosen) }
        }
    }
}
