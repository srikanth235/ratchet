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
}
