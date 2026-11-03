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
        case .home: .home
        case .overview: .overview
        case .optimizations: .optimizations
        case .agents: .agents
        case .connectors: .connectors
        case .benchmarks: .benchmarks
        }
    }

    /// Whether the page has a destination. All of them do: Home and Agents show a placeholder until P5b fills `HomePage` and
    /// `AgentsPage`; the sidebar, the Go menu and the shortcuts follow this list.
    var isAvailable: Bool { true }

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
        case .home: HomePage()
        case .overview: OverviewPage()
        case .optimizations: OptimizationsPage()
        case .agents: AgentsPage()
        case .benchmarks: BenchmarksPage()
        case .connectors: ConnectorsPage()
        }
    }
}

/// One row of the sidebar: a page, or one of the recent runs under it. At most two levels: the section, then its rows.
enum SidebarItem: Hashable {
    case page(Page)
    case run(String)
}
