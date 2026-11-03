import AppKit
import RatchetKit
import SwiftUI

/// The window: the system sidebar (the pages and the recent runs) and, beside it, the page or the New Optimization flow, under
/// a native toolbar. Banners (the daemon is down, an action failed) sit above the page. The title bar names the page; the
/// page and the open optimization are restored with the scene.
struct ContentView: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ChatHub.self) private var hub
    @Environment(ConnectorsPageState.self) private var connectors
    @Environment(DaemonController.self) private var daemon
    @Environment(AppNotifier.self) private var notifier
    @Environment(LoginItem.self) private var loginItem
    @Environment(\.openSettings) private var openSettings
    @Environment(\.openWindow) private var openWindow
    @State private var ran = false
    // The scene's own memory of where it was: the page and the open optimization (ids only, never data).
    @SceneStorage("shell.page") private var storedPage = ""
    @SceneStorage("shell.optimization") private var storedOptimization = ""
    @State private var restoredOptimization = false

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: Dim.sidebarWidth, ideal: Dim.sidebarWidth, max: Dim.sidebarWidth + Space.section * 2)
        } detail: {
            let showsStatus = daemon.showsStatusScreen(storeLoaded: store.isLoaded)
            VStack(spacing: 0) {
                // The service is down but there is data to show: say so on every page, above it.
                if !showsStatus && !daemon.phase.isRunning && daemon.phase != .checking {
                    DaemonBanner()
                        .padding(.horizontal, Space.pageX)
                        .padding(.top, Space.sm)
                        .transition(.opacity)
                }
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
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Ink.canvas)
            .animation(Motion.step, value: store.lastError)
            .animation(Motion.step, value: daemon.phase)
            .navigationTitle(navigationTitle)
            .navigationSubtitle(navigationSubtitle)
        }
        .environment(\.daemonActionsEnabled, daemon.phase.isRunning)
        .noInitialFocus()
        .background(MainWindowRegistrar())
        .confirmationDialog(
            stopTitle, isPresented: Binding(get: { router.pendingStop != nil }, set: { if !$0 { router.pendingStop = nil } }),
            titleVisibility: .visible, presenting: router.pendingStop
        ) { id in
            Button("Stop", role: .destructive) { Task { await store.optimizationStore.stopOptimization(id) } }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Wins found so far are kept.")
        }
        .confirmationDialog(
            archivePrompt.title, isPresented: Binding(get: { router.pendingArchive != nil }, set: { if !$0 { router.pendingArchive = nil } }),
            titleVisibility: .visible, presenting: router.pendingArchive
        ) { id in
            Button(archivePrompt.confirm, role: .destructive) { Task { await hub.settings(for: id).archive() } }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text(archivePrompt.message)
        }
        .onChange(of: router.page) { _, page in storedPage = page.rawValue }
        .onChange(of: router.optimizationID) { _, id in storedOptimization = id ?? "" }
        .task {
            restorePage()
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
            await hub.refresh(store: store)
            restoreOptimization()
            await notifier.refreshAuthorization()
            guard !ran else { return }
            ran = true
            SelfTest.runIfRequested(store: store, router: router, hub: hub, daemon: daemon, notifier: notifier)
            Snapshot.runIfRequested(
                store: store, router: router, hub: hub, connectors: connectors, openSettings: openSettings, daemon: daemon,
                notifier: notifier, loginItem: loginItem)
        }
        // The live events follow the same rule, as a task of their own: they stop when the daemon goes away or the data source
        // changes (demo mode), and start again with them. Nothing else holds on to the stream.
        .task(id: "\(daemon.phase.isRunning)|\(store.isDemoData)") {
            guard daemon.phase.isRunning else { return }
            await store.followEvents()
        }
        .onOpenURL { handle(url: $0) }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            // Back from System Settings or the shell: the permissions and the daemon may have changed.
            daemon.recheck()
            loginItem.refresh()
            Task { await notifier.refreshAuthorization() }
        }
    }
}

extension ContentView {
    /// The window title: the page, or the flow. Never the app's name.
    private var navigationTitle: String {
        router.creationChatID != nil ? "New Optimization" : router.page.title
    }

    /// Every page has a subtitle: the open optimization, a count, the range.
    private var navigationSubtitle: String {
        switch router.page {
        case .home:
            if let chatID = router.creationChatID { return hub.conversation(chatID, router: router).harnessLabel }
            return "Describe what to optimize"
        case .overview: return store.range.title
        case .optimizations:
            if let id = router.optimizationID, let optimization = store.optimizationStore.optimization(id: id) { return optimization.title }
            return Plural.count(store.optimizationStore.optimizations.count, "optimization")
        case .agents: return "Agent CLIs on this Mac"
        case .benchmarks: return Plural.count(store.benchmarks.count, "run")
        case .connectors: return Plural.count(store.connectorStore.connectors.count, "connector")
        }
    }

    private var archivePrompt: (title: String, message: String, confirm: String) {
        let title = router.pendingArchive.flatMap { store.optimizationStore.optimization(id: $0)?.title } ?? "this optimization"
        return OptimizationSettingsModel.archivePrompt(title: title)
    }

    private var stopTitle: String {
        guard let id = router.pendingStop, let optimization = store.optimizationStore.optimization(id: id) else { return "Stop this optimization?" }
        return "Stop \(optimization.title)?"
    }

    /// Comes back to the page the scene was on. Test and snapshot runs always start on Overview.
    private func restorePage() {
        guard !Store.isOverridden, let page = Page(rawValue: storedPage), page != router.page else { return }
        router.show(page)
    }

    /// The open optimization comes back once the list is there to say it still exists.
    private func restoreOptimization() {
        guard !Store.isOverridden, !restoredOptimization else { return }
        restoredOptimization = true
        guard !storedOptimization.isEmpty, router.optimizationID == nil, router.creationChatID == nil,
              store.optimizationStore.optimization(id: storedOptimization) != nil else { return }
        router.open(optimization: storedOptimization)
    }

    /// Connects the store's events to notifications, and a click on a notification to the right screen.
    private func wire() {
        store.enabledNotifications = { NotificationPreferences.enabled }
        store.onNotification = { [notifier] request in notifier.post(request) }
        notifier.onOpen = { [store, router, openWindow] destination in
            Self.go(to: destination, store: store, router: router, openWindow: openWindow)
        }
        DebugBridge.start(
            store: store, router: router, hub: hub, connectors: connectors, daemon: daemon, notifier: notifier,
            openSettings: openSettings)
    }

    /// Brings the window forward when it is closed or hidden (a click, a link or a command from the menu bar era).
    @MainActor
    static func revealWindowIfNeeded(openWindow: OpenWindowAction) {
        if !NSApp.windows.contains(where: { $0.canBecomeMain && $0.isVisible }) {
            LaunchContext.revealWindow { openWindow(id: "main") }
        }
    }

    /// A place a notification click or a `ratchet://` link opens.
    @MainActor
    static func go(to destination: NotificationDestination, store: AppStore, router: AppRouter, openWindow: OpenWindowAction) {
        revealWindowIfNeeded(openWindow: openWindow)
        switch destination {
        case .optimization(let id): router.open(optimization: id)
        case .connectors: router.show(.connectors)
        case .newOptimization: router.newOptimization()
        case .overview: router.show(.overview)
        }
    }

    /// `ratchet://...` (`.onOpenURL`): a page by name, or the same places a notification click opens.
    private func handle(url: URL) {
        guard let link = AppLink(url: url) else { return }
        switch link.resolve(pages: Page.sidebar.map(\.rawValue)) {
        case .page(let name):
            guard let page = Page(rawValue: name) else { return }
            Self.revealWindowIfNeeded(openWindow: openWindow)
            router.show(page)
        case .destination(let destination):
            // The same path as a click: it waits when the window is not ready yet.
            notifier.open(destination)
        }
    }
}

private struct DetailRoot: View {
    @Environment(AppRouter.self) private var router
    @Environment(AppStore.self) private var store

    private var showsConnectionBadge: Bool {
        if case .reconnecting = store.streamState { return true }
        return store.isDemoData
    }

    var body: some View {
        Group {
            router.page.destination
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .toolbar { ShellToolbar(showsConnectionBadge: showsConnectionBadge) }
    }
}
