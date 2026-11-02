import AppKit
import RatchetKit
import SwiftUI

/// The window: a sidebar with four pages and the sandbox status line, and the page (or the New optimization flow)
/// beside it.
struct ContentView: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ConnectorsPageState.self) private var connectors
    @Environment(DaemonController.self) private var daemon
    @Environment(AppNotifier.self) private var notifier
    @Environment(LoginItem.self) private var loginItem
    @Environment(\.openSettings) private var openSettings
    @Environment(\.openWindow) private var openWindow
    @State private var ran = false

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(Dim.sidebarWidth)
        } detail: {
            let showsStatus = daemon.showsStatusScreen(storeLoaded: store.isLoaded)
            VStack(spacing: 0) {
                // What went wrong stays on every page until it is dismissed or the next action starts.
                if let message = store.lastError, !showsStatus {
                    ErrorBanner(message: message) { store.dismissError() }
                        .padding(.horizontal, Space.pageX)
                        .padding(.top, Space.sm)
                        .padding(.bottom, Space.sm)
                        .transition(.opacity)
                }
                if showsStatus {
                    DaemonStatusView()
                } else {
                    DetailRoot()
                }
            }
            .animation(Motion.step, value: store.lastError)
        }
        .toolbar(removing: .title)
        // The page starts at the top edge, as in the design; the traffic lights sit over the sidebar.
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        .noInitialFocus()
        .task {
            wire()
            daemon.start()
            // Started by the login item: only the menu bar item shows. The window stays alive, hidden, so the data keeps
            // loading and notifications keep coming.
            if LaunchContext.startsInMenuBar { LaunchContext.hideWindow() }
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
                store: store, router: router, connectors: connectors, openSettings: openSettings, daemon: daemon,
                notifier: notifier, loginItem: loginItem)
        }
        // The live events follow the same rule, as a task of their own: they stop when the daemon goes away or the data source
        // changes (demo mode), and start again with them. Nothing else holds on to the stream.
        .task(id: "\(daemon.phase.isRunning)|\(store.isDemoData)") {
            guard daemon.phase.isRunning else { return }
            await store.followEvents()
        }
        .sheet(item: Binding(get: { router.templateProposal }, set: { router.templateProposal = $0 })) { flow in
            UseTemplateSheet(flow: flow)
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            daemon.recheck()
            loginItem.refresh()
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
            if !NSApp.windows.contains(where: { $0.canBecomeMain && $0.isVisible }) {
                LaunchContext.revealWindow { openWindow(id: "main") }
            }
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
    @Environment(ConnectorsPageState.self) private var connectors

    var body: some View {
        Group {
            if let flow = router.wizard {
                NewOptimizationView(flow: flow)
            } else {
                router.page.destination
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Ink.canvas)
        .ignoresSafeArea(.container, edges: .top)
    }
}
