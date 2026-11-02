import RatchetKit
import SwiftUI

/// The pages of the window. One case adds a page: its title, icon, availability and `destination` are all here, the sidebar and
/// the Go menu are built from `sidebar`, and the Command-number of a page is its place in it.
enum Page: String, CaseIterable, Identifiable {
    case home, overview, optimizations, agents, benchmarks, connectors

    var id: String { rawValue }

    var title: String {
        switch self {
        case .home: "Home"
        case .overview: "Overview"
        case .optimizations: "Optimizations"
        case .agents: "Agents"
        case .benchmarks: "Benchmarks"
        case .connectors: "Connectors"
        }
    }

    var icon: KitIcon {
        switch self {
        // Home and Agents get their own icons with their screens (P5, the design kit); until then they are not shown.
        case .home, .overview: .overview
        case .optimizations: .optimizations
        case .agents, .connectors: .connectors
        case .benchmarks: .benchmarks
        }
    }

    /// Whether the page has a screen yet. Home (the composer) and Agents come with the chat work (P5): turn them on there and the
    /// sidebar, the Go menu and the shortcuts follow.
    var isAvailable: Bool {
        switch self {
        case .home, .agents: false
        case .overview, .optimizations, .benchmarks, .connectors: true
        }
    }

    /// The pages the sidebar and the Go menu list, in order.
    static var sidebar: [Page] { allCases.filter(\.isAvailable) }

    /// Command-1 to Command-9: the page's place in the sidebar.
    var shortcut: KeyboardShortcut? {
        guard let index = Page.sidebar.firstIndex(of: self), index < 9 else { return nil }
        return KeyboardShortcut(KeyEquivalent(Character(String(index + 1))), modifiers: .command)
    }

    /// The number shown in help text: "2".
    var shortcutNumber: String? {
        shortcut.map { String($0.key.character) }
    }

    /// What the window shows for this page.
    @ViewBuilder var destination: some View {
        switch self {
        case .overview: OverviewPage()
        case .optimizations: OptimizationsPage()
        case .benchmarks: BenchmarksPage()
        case .connectors: ConnectorsPage()
        case .home, .agents: Text(title).kitType(.pageTitle).foregroundStyle(Ink.textMuted)
        }
    }
}
