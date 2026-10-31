import RatchetKit
import SwiftUI

/// The window: a sidebar with four pages and the sandbox status line, and the page (or the New optimization flow)
/// beside it.
struct ContentView: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(\.openSettings) private var openSettings
    /// The sidebar column's real width, so its surface and edge line reach the top of the window too.
    @State private var sidebarColumn = Dim.sidebarWidth

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(Dim.sidebarWidth)
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { sidebarColumn = $0 }
        } detail: {
            DetailRoot()
        }
        .background(alignment: .leading) {
            Ink.sidebar
                .frame(width: sidebarColumn)
                .overlay(alignment: .trailing) { Ink.sidebarEdge.frame(width: Dim.hairline) }
                .ignoresSafeArea()
        }
        .toolbar(removing: .title)
        // The page starts at the top edge, as in the design; the traffic lights sit over the sidebar.
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        .noInitialFocus()
        .task {
            await store.start()
            SelfTest.runIfRequested(store: store, router: router)
            Snapshot.runIfRequested(store: store, router: router, openSettings: openSettings)
        }
    }
}

private struct DetailRoot: View {
    @Environment(AppRouter.self) private var router

    var body: some View {
        Group {
            if let flow = router.wizard {
                NewOptimizationView(flow: flow)
            } else {
                switch router.page {
                case .overview: OverviewPage()
                case .optimizations: OptimizationsPage()
                case .benchmarks: BenchmarksPage()
                case .connectors: ConnectorsPage()
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Ink.canvas)
        .ignoresSafeArea(.container, edges: .top)
    }
}
