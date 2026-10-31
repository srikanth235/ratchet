import RatchetKit
import SwiftUI

/// The window: a sidebar with four pages and the sandbox status line, and the page (or the New optimization flow)
/// beside it.
struct ContentView: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 200, ideal: Dim.sidebarWidth, max: 280)
        } detail: {
            DetailRoot()
        }
        .toolbar(removing: .title)
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
    }
}
