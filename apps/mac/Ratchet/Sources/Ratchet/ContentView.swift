import AppKit
import RatchetKit
import SwiftUI

/// The window: a sidebar with four pages and the sandbox status line, and the page (or the New optimization flow)
/// beside it.
struct ContentView: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(DaemonController.self) private var daemon
    @Environment(AppNotifier.self) private var notifier
    @Environment(\.openSettings) private var openSettings
    @Environment(\.openWindow) private var openWindow
    @State private var ran = false

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(Dim.sidebarWidth)
        } detail: {
            if daemon.showsStatusScreen(storeLoaded: store.isLoaded) {
                DaemonStatusView()
            } else {
                DetailRoot()
            }
        }
        .toolbar(removing: .title)
        // The page starts at the top edge, as in the design; the traffic lights sit over the sidebar.
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        .noInitialFocus()
        .task {
            wire()
            daemon.start()
        }
        // Loads the data once the daemon answers (at once with sample data), and again when it comes back.
        .task(id: daemon.phase.isRunning) {
            guard daemon.phase.isRunning else { return }
            await store.start()
            await notifier.refreshAuthorization()
            guard !ran else { return }
            ran = true
            SelfTest.runIfRequested(store: store, router: router, daemon: daemon)
            Snapshot.runIfRequested(
                store: store, router: router, openSettings: openSettings, daemon: daemon, notifier: notifier)
        }
        .sheet(item: Binding(get: { router.templateProposal }, set: { router.templateProposal = $0 })) { flow in
            UseTemplateSheet(flow: flow)
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            daemon.recheck()
            Task { await notifier.refreshAuthorization() }
        }
    }
}

extension ContentView {
    /// Connects the store's events to notifications, and a click on a notification to the right screen.
    private func wire() {
        store.enabledNotifications = { NotificationPreferences.enabled }
        store.onNotification = { [notifier] request in notifier.post(request) }
        notifier.onOpen = { [store, router] destination in
            if !NSApp.windows.contains(where: { $0.canBecomeMain && $0.isVisible }) { openWindow(id: "main") }
            switch destination {
            case .optimization(let id): router.open(optimization: id)
            case .connectors: router.show(.connectors)
            case .newOptimization: router.startWizard(client: store.client)
            case .overview: router.show(.overview)
            }
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
