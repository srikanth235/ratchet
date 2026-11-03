import Observation

/// Where snapshot mode wants a page scrolled to ("log", "time", "spend", "run"), so a scene can show a section far down a
/// long page. Views mark their sections with `.id(...)` and `KitPage` scrolls to the target; nil means the top.
@Observable
final class SnapshotFocus {
    static let shared = SnapshotFocus()

    var target: String?
    /// The metric the benchmark run should show ("p99"), or nil for the objective.
    var metric: String?
    /// Snapshot mode only: show the first-load state (grey blocks) even though the data is there (v0.3.1 screen 14).
    var showsLoading = false
    /// Snapshot mode only: the popover to open, by its title ("Change agents", "Edit Agents"), once its button is on screen.
    var popover: String?
    /// Snapshot mode only: the log view to show ("by-agent", "everything").
    var logTab: String?
    /// Snapshot mode only: the Connectors tab to show ("discover").
    var connectorsTab: String?
}

extension SnapshotFocus {
    /// Snapshot mode only: calls `open` when the scene asked for the popover with this title, after the button is on screen.
    @MainActor
    static func opens(_ title: String, focus: SnapshotFocus, _ open: () -> Void) async {
        guard Store.isOverridden, focus.popover == title else { return }
        try? await Task.sleep(for: .milliseconds(400))
        if focus.popover == title { open() }
    }
}
