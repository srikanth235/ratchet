import AppKit
import IOKit.pwr_mgt
import RatchetKit
import SwiftUI

/// What snapshot mode shoots, one PNG per scene and mode.
enum SnapScene: String, CaseIterable {
    case overview, optimizations, benchmarks, connectors
    case home, agents
    // v0.3.1 screens 12 and 14, and 13 (a connector that needs signing in again): the service is down with data on screen,
    // the first load is under way, a failed connector with its fix beside it.
    case daemonDown = "daemon-down"
    case loadingOverview = "loading-overview"
    case failedConnector = "failed-connector"
    case detailLive = "detail-live"
    case detailResult = "detail-result"
    case detailNoWin = "detail-nowin"
    // The live log, the Result details and the benchmark run (a section far down its page, scrolled into view).
    case detailLiveLog = "detail-live-log"
    case detailResultTime = "detail-result-time"
    case detailResultSpend = "detail-result-spend"
    case benchmarksRun = "benchmarks-run"
    case benchmarksRunP99 = "benchmarks-run-p99"
    // The states the bundled daemon and the notifications add.
    case daemonStarting = "daemon-starting"
    case daemonApproval = "daemon-approval"
    case notificationPrompt = "notification-prompt"
    case connectorSignIn = "connector-signin"
    // A failed action: the dismissible banner above the page (Stop on an optimization that already ended).
    case errorBanner = "error-banner"
    case connectorsAddGithub = "connectors-add-github"
    case connectorsAddAWS = "connectors-add-aws"
    case connectorsAddAgent = "connectors-add-agent"
    case connectorsEdit = "connectors-edit"
    case connectorsRename = "connectors-rename"
    case connectorsCheck = "connectors-check"
    case connectorsKeychain = "connectors-keychain"
    case connectorsDiscover = "connectors-discover"
    // P5b: Home, the creation conversation, the run, the optimization's settings, the Agents page, the menu bar's keep-awake line.
    case homeAgentsPopover = "home-agents-popover"
    case homeEmpty = "home-empty"
    case conversationDrafting = "conversation-drafting"
    case conversationDraft = "conversation-draft"
    case conversationDraftPopover = "conversation-draft-popover"
    case conversationEnvError = "conversation-env-error"
    case conversationLimit = "conversation-limit"
    case runOverview = "run-overview"
    case runByAgent = "run-by-agent"
    case runEverything = "run-everything"
    case runStop = "run-stop"
    case optimizationSettings = "optimization-settings"
    case optimizationSettingsPopover = "optimization-settings-popover"
    case agentsSignIn = "agents-signin"
    case overviewNoWins = "overview-nowins"
    case menuBarAwake = "menubar-awake"
    // The parts gallery (DesignKit/KitGallery.swift): every part in every state, one section per scene.
    case galleryActions = "gallery-actions"
    case galleryChoices = "gallery-choices"
    case galleryRows = "gallery-rows"
    case galleryInputs = "gallery-inputs"
    case galleryStatus = "gallery-status"
    case galleryIdentity = "gallery-identity"
    case galleryType = "gallery-type"
    case galleryTokens = "gallery-tokens"

    /// The gallery section this scene shows, if it is a gallery scene.
    var gallerySection: GallerySection? {
        switch self {
        case .galleryActions: .actions
        case .galleryChoices: .choices
        case .galleryRows: .rows
        case .galleryInputs: .inputs
        case .galleryStatus: .status
        case .galleryIdentity: .identity
        case .galleryType: .type
        case .galleryTokens: .tokens
        default: nil
        }
    }

    // The daemon not answering: failed, stopped, and a build with no ratchetd inside.
    case daemonFailed = "daemon-failed"
    case daemonStopped = "daemon-stopped"
    case daemonUnbundled = "daemon-unbundled"
    // Nothing yet: the pages with no optimizations, no wins, no benchmark runs.
    case emptyOverview = "empty-overview"
    case emptyOptimizations = "empty-optimizations"
    case emptyBenchmarks = "empty-benchmarks"
    // The menu bar panel, hosted in a window so it can be captured: with running optimizations, and with none.
    case menuBar = "menubar"
    case menuBarEmpty = "menubar-empty"
    // Settings with notifications denied in System Settings. Last: it opens the Settings window.
    case notificationsDenied = "notifications-denied"

    /// What a scene is a picture of.
    enum Subject { case window, menuBar, settings }

    var subject: Subject {
        switch self {
        case .menuBar, .menuBarEmpty, .menuBarAwake: .menuBar
        case .notificationsDenied: .settings
        default: .window
        }
    }

    /// Scenes that show the app with nothing in it.
    var isEmpty: Bool {
        switch self {
        case .emptyOverview, .emptyOptimizations, .emptyBenchmarks, .menuBarEmpty, .homeEmpty: true
        default: false
        }
    }

    /// Puts the window in this scene's state.
    func apply(
        store: AppStore, router: AppRouter, hub: ChatHub, connectors: ConnectorsPageState, daemon: DaemonController,
        notifier: AppNotifier
    ) async {
        // Every scene starts from the calm state.
        SnapshotFocus.shared.target = nil
        SnapshotFocus.shared.metric = nil
        SnapshotFocus.shared.showsLoading = false
        daemon.preview(nil)
        notifier.preview(.allowed, prompt: false)
        store.dismissError()
        await store.connectorStore.cancelGithubSignIn()
        KitGalleryState.shared.isShown = gallerySection != nil
        KitGalleryState.shared.section = gallerySection
        SnapshotFocus.shared.popover = nil
        SnapshotFocus.shared.logTab = nil
        SnapshotFocus.shared.connectorsTab = nil
        router.closeConversation()
        router.pendingStop = nil
        router.pendingArchive = nil
        router.detailTab = .run
        router.show(.overview)
        connectors.reset()
        router.advancedExpanded = false
        hub.signInHelp = nil
        hub.homeNoticeClosed = false
        hub.composer.text = ""
        hub.composer.repo = nil
        hub.composer.cloudAccount = nil
        hub.composer.limits = nil
        hub.composer.schedule = nil
        store.apply(.powerChanged(PowerState()))
        await hub.sampleStory?.use(self == .overviewNoWins ? .noWins : isEmpty ? .empty : .wins)
        let mock = store.scenario
        await mock?.simulate(keychainAvailable: true)
        await mock?.simulate(preflight: nil)
        // Empty scenes empty the sample world; every other scene brings the sample data back.
        await mock?.simulate(empty: isEmpty)
        await store.refresh()
        await hub.refresh(store: store)
        switch self {
        case .overview, .emptyOverview, .overviewNoWins, .menuBar, .menuBarEmpty, .galleryActions, .galleryChoices, .galleryRows, .galleryInputs,
            .galleryStatus, .galleryIdentity, .galleryType, .galleryTokens:
            break
        case .home:
            router.show(.home)
            Self.fillComposer(hub)
        case .homeEmpty: router.show(.home)
        case .homeAgentsPopover:
            router.show(.home)
            Self.fillComposer(hub)
            SnapshotFocus.shared.popover = ComposerChip.agents.popoverTitle
        case .agents: router.show(.agents)
        case .agentsSignIn:
            router.show(.agents)
            hub.signInHelp = SignInHelp(harness: .codex, command: AgentsPageModel.signInCommand(.codex))
        case .menuBarAwake:
            store.apply(.powerChanged(PowerState(keepingAwake: true, localOptimizationIDs: ["opt-checkout"])))
        case .connectorsDiscover:
            router.show(.connectors)
            SnapshotFocus.shared.connectorsTab = "discover"
        case .conversationDrafting, .conversationDraft, .conversationDraftPopover, .conversationEnvError, .conversationLimit:
            await Self.startConversation(self, hub: hub, router: router, store: store)
        case .runOverview, .runByAgent, .runEverything:
            await Self.openRun(self, store: store, router: router, hub: hub)
        case .runStop:
            router.open(optimization: "opt-checkout")
            router.pendingStop = "opt-checkout"
        case .optimizationSettings, .optimizationSettingsPopover:
            router.open(optimization: "opt-checkout")
            router.detailTab = .settings
            let settings = hub.settings(for: "opt-checkout")
            await settings.load()
            // A saved row shows "Saved" beside it.
            await settings.saveSchedule(.nightly)
            if self == .optimizationSettingsPopover { SnapshotFocus.shared.popover = "Edit Agents" }
        case .daemonDown: daemon.preview(.stopped, statusScreen: false)
        case .loadingOverview: SnapshotFocus.shared.showsLoading = true
        case .failedConnector:
            router.show(.connectors)
            connectors.rowNotes["aws-acme-staging"] = ConnectorNote(
                ok: false, text: "The AWS session expired Oct 31. “CLI cold start” failed on it. Sign in again.")
        case .emptyOptimizations: router.show(.optimizations)
        case .emptyBenchmarks: router.show(.benchmarks)
        case .daemonFailed: daemon.preview(.failed("Ratchet's background service did not start."))
        case .daemonStopped: daemon.preview(.stopped)
        case .daemonUnbundled: daemon.previewUnbundled()
        case .notificationsDenied: notifier.preview(.denied, prompt: false)
        case .daemonStarting: daemon.preview(.starting)
        case .daemonApproval: daemon.preview(.needsApproval)
        case .notificationPrompt: notifier.preview(.notDetermined, prompt: true)
        case .errorBanner:
            router.open(optimization: "opt-checkout")
            await store.optimizationStore.stopOptimization("opt-bundle")
        case .connectorSignIn:
            router.show(.connectors)
            await store.connectorStore.startGithubSignIn()
        case .optimizations: router.show(.optimizations)
        case .benchmarks: router.show(.benchmarks)
        case .connectors: router.show(.connectors)
        case .detailLive: router.open(optimization: "opt-checkout")
        case .detailResult: router.open(optimization: "opt-forecast")
        case .detailNoWin: router.open(optimization: "opt-bundle")
        case .detailLiveLog:
            router.open(optimization: "opt-checkout")
            await store.optimizationStore.loadDetail(id: "opt-checkout")
            await store.insight.loadAgentLog("agent-claude-login")
            SnapshotFocus.shared.target = "log"
        case .detailResultTime:
            router.open(optimization: "opt-cli")
            await store.optimizationStore.loadDetail(id: "opt-cli")
            await store.insight.loadInsights("opt-cli")
            SnapshotFocus.shared.target = "time"
        case .detailResultSpend:
            router.open(optimization: "opt-forecast")
            await store.optimizationStore.loadDetail(id: "opt-forecast")
            await store.insight.loadInsights("opt-forecast")
            SnapshotFocus.shared.target = "spend"
        case .benchmarksRun, .benchmarksRunP99:
            router.show(.benchmarks)
            await store.insight.loadBenchmark("opt-checkout", metric: nil)
            if self == .benchmarksRunP99 { SnapshotFocus.shared.metric = "p99" }
            SnapshotFocus.shared.target = "run"
        case .connectorsAddGithub, .connectorsAddAWS, .connectorsAddAgent, .connectorsEdit, .connectorsRename, .connectorsCheck,
            .connectorsKeychain:
            router.show(.connectors)
            // The page has to be on screen to take what is set below.
            try? await Task.sleep(for: .milliseconds(400))
            switch self {
            case .connectorsAddGithub: connectors.sheet = ConnectorSheetRequest(kind: .github, showsToken: true)
            case .connectorsAddAWS: connectors.sheet = ConnectorSheetRequest(kind: .aws, pick: "sandbox-eu")
            case .connectorsAddAgent: connectors.sheet = ConnectorSheetRequest(kind: .agent)
            case .connectorsEdit: connectors.rowModes["aws-acme-prod"] = .edit
            case .connectorsRename: connectors.rowModes["gh-acme-org"] = .rename
            case .connectorsCheck:
                connectors.rowModes["aws-acme-staging"] = .ssoHint
                connectors.rowNotes["gh-srikanth235"] = ConnectorNote(ok: true, text: "GitHub says you are signed in as srikanth235.")
                connectors.rowNotes["agent-codex-login"] = ConnectorNote(ok: false, text: "Codex is not signed in. Run `codex login` in a terminal.")
            default:
                await mock?.simulate(keychainAvailable: false)
                await store.refresh()
            }
        }
    }

    /// A composer as the mockup shows it: the repo, the cloud, the limits and the schedule set.
    private static func fillComposer(_ hub: ChatHub) {
        hub.composer.text = "Make checkout p95 faster in shop-api. Run it every night."
        hub.composer.repo = "acme-org/shop-api"
        hub.composer.cloudAccount = hub.composer.options.cloudAccounts.first
        hub.composer.limits = LimitsEditor(budgetUSD: 50, hours: 2)
        hub.composer.schedule = .nightly
    }

    /// Sends from Home and waits for what the scene shows: the draft card, the drafting state, or the error.
    private static func startConversation(_ scene: SnapScene, hub: ChatHub, router: AppRouter, store: AppStore) async {
        router.show(.home)
        hub.composer.repo = scene == .conversationLimit ? nil : "acme-org/shop-api"
        let words = "Make checkout p95 faster in shop-api. Run it every night, $50 a run."
        switch scene {
        case .conversationDrafting: hub.composer.text = "[simulate:slow] " + words
        case .conversationEnvError: hub.composer.text = "[simulate:env-fail] " + words
        case .conversationLimit: hub.composer.text = "[simulate:rate-limit] " + words
        default: hub.composer.text = words
        }
        guard let id = await hub.sendFromHome(store: store, router: router) else { return }
        let card = hub.draftCard(id)
        let model = hub.conversation(id, router: router)
        for _ in 0..<300 {
            let ready: Bool
            switch scene {
            case .conversationDrafting: ready = model.isReplying
            case .conversationLimit: ready = model.error != nil
            default: ready = card.card != nil && !card.isDrafting
            }
            if ready { break }
            try? await Task.sleep(for: .milliseconds(10))
        }
        if scene == .conversationDraftPopover { SnapshotFocus.shared.popover = "Change agents" }
    }

    /// The run of the live optimization with two notes: the first read by Claude Code, the second still queued.
    private static func openRun(_ scene: SnapScene, store: AppStore, router: AppRouter, hub: ChatHub) async {
        router.open(optimization: "opt-checkout")
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        for agent in store.optimizationStore.details["opt-checkout"]?.agents ?? [] { await store.insight.loadAgentLog(agent.id) }
        switch scene {
        case .runByAgent: SnapshotFocus.shared.logTab = "by-agent"
        case .runEverything: SnapshotFocus.shared.logTab = "everything"
        default: break
        }
        SnapshotFocus.shared.target = "log"
        // The steering field is on screen now, and following its chat.
        try? await Task.sleep(for: .milliseconds(500))
        let steering = hub.steering(for: "opt-checkout")
        steering.draft = "skip the cache layer, it is owned by another team"
        await steering.send()
        for _ in 0..<300 where steering.entries.count < 1 { try? await Task.sleep(for: .milliseconds(10)) }
        if let chatID = steering.chatID { await hub.chatScenario?.readNotes(chatID: chatID, by: .claudeCode, at: Store.fixedNow) }
        steering.draft = "focus on database queries"
        await steering.send()
        for _ in 0..<300 where steering.entries.count < 2 { try? await Task.sleep(for: .milliseconds(10)) }
    }
}

/// Screenshot mode for agents.
///   APP_STORE=/tmp/test.store APP_SNAPSHOT=/tmp/shots ~/Applications/Ratchet.app/Contents/MacOS/Ratchet
/// writes <scene>-light.png and <scene>-dark.png for every SnapScene (the four pages, the optimization detail live,
/// finished and without a win, the four New optimization steps, the eight gallery-<section> scenes of the parts gallery),
/// and settings-<tab>-light.png and -dark.png for
/// every Settings tab, then quits. Each PNG gets a <scene>-<mode>-layout.json with the grid probes
/// (tools/check-layout.swift).
/// APP_SNAPSHOT_PAGES=overview,new-budget limits the scenes. "all" means every scene; add "settings" for Settings.
enum Snapshot {
    static func runIfRequested(
        store: AppStore, router: AppRouter, hub: ChatHub, connectors: ConnectorsPageState, openSettings: OpenSettingsAction,
        daemon: DaemonController, notifier: AppNotifier, loginItem: LoginItem
    ) {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["APP_SNAPSHOT"], !dir.isEmpty else { return }
        // Snapshots never run against the real data.
        guard Store.isOverridden else {
            print("SNAPSHOT refused: set APP_STORE to a file in /tmp")
            NSApp.terminate(nil)
            return
        }
        let only = env["APP_SNAPSHOT_PAGES"].map { Set($0.split(separator: ",").map(String.init)) }
        let everything = only == nil || only?.contains("all") == true
        Task {
            await shootScenes(
                dir: dir, everything: everything, only: only, store: store, router: router, hub: hub, connectors: connectors,
                openSettings: openSettings, daemon: daemon, notifier: notifier)
            if everything || only?.contains("settings") == true {
                await SnapScene.overview.apply(
                    store: store, router: router, hub: hub, connectors: connectors, daemon: daemon, notifier: notifier)
                let main = mainWindow
                loginItem.preview(.off)
                openSettings()
                try? await Task.sleep(for: .seconds(1))
                for mode in ["light", "dark"] {
                    NSApp.appearance = NSAppearance(named: mode == "light" ? .aqua : .darkAqua)
                    for tab in SettingsTab.allCases {
                        router.settingsTab = tab
                        try? await Task.sleep(for: .seconds(0.8))
                        let settings = NSApp.windows.first {
                            $0.isVisible && $0 !== main && !$0.isSheet && $0.styleMask.contains(.titled)
                        }
                        capture(to: "\(dir)/settings-\(tab.rawValue)-\(mode).png", window: settings)
                    }
                    // Advanced (development), opened.
                    router.settingsTab = .advanced
                    router.advancedExpanded = true
                    try? await Task.sleep(for: .seconds(1))
                    let advanced = NSApp.windows.first {
                        $0.isVisible && $0 !== main && !$0.isSheet && $0.styleMask.contains(.titled)
                    }
                    capture(to: "\(dir)/settings-advanced-open-\(mode).png", window: advanced)
                    router.advancedExpanded = false
                    // Launch at login, waiting for the OK in System Settings.
                    router.settingsTab = .thisMac
                    loginItem.preview(.needsApproval)
                    try? await Task.sleep(for: .seconds(0.8))
                    let pending = NSApp.windows.first {
                        $0.isVisible && $0 !== main && !$0.isSheet && $0.styleMask.contains(.titled)
                    }
                    capture(to: "\(dir)/settings-login-approval-\(mode).png", window: pending)
                    loginItem.preview(.off)
                }
            }
            print("SNAPSHOT done \(dir)")
            // An open sheet refuses terminate, and the run would wait forever.
            for window in NSApp.windows { if let sheet = window.attachedSheet { window.endSheet(sheet) } }
            NSApp.terminate(nil)
        }
    }

    /// The app's main window (its title is the page now, so it registers itself), not Settings and not a sheet.
    static var mainWindow: NSWindow? {
        MainWindow.window ?? NSApp.windows.first { $0.canBecomeMain && !$0.isSheet }
    }

    /// Takes the picture of a scene whose state is already applied: the main window, the menu bar panel in a window of its
    /// own, or the Settings window. The debug bridge's `snapshot:<scene>` uses this too.
    static func captureSubject(
        of scene: SnapScene, to path: String, store: AppStore, router: AppRouter, openSettings: OpenSettingsAction
    ) async {
        switch scene.subject {
        case .window:
            capture(to: path)
        case .menuBar:
            // The menu bar panel belongs to the system; host the same view in a window to capture it.
            let window = MenuBarCapture.show(store: store, router: router)
            try? await Task.sleep(for: .seconds(0.6))
            capture(to: path, window: window)
            window.close()
        case .settings:
            let main = mainWindow
            openSettings()
            router.settingsTab = .general
            try? await Task.sleep(for: .seconds(1))
            let settings = NSApp.windows.first {
                $0.isVisible && $0 !== main && !$0.isSheet && $0.styleMask.contains(.titled)
            }
            capture(to: path, window: settings)
            settings?.close()
        }
    }

    /// Every scene in light and dark. The display stays awake for the whole run and the assertions are released when
    /// it ends, however it ends.
    private static func shootScenes(
        dir: String, everything: Bool, only: Set<String>?, store: AppStore, router: AppRouter, hub: ChatHub,
        connectors: ConnectorsPageState, openSettings: OpenSettingsAction, daemon: DaemonController, notifier: AppNotifier
    ) async {
        // A sleeping display gives screencapture nothing. Declaring user activity wakes it; the assertion keeps it on.
        var power = SnapshotPower()
        defer { power.release() }
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        try? await Task.sleep(for: .seconds(1.5))
        for mode in ["light", "dark"] {
            NSApp.appearance = NSAppearance(named: mode == "light" ? .aqua : .darkAqua)
            for scene in SnapScene.allCases where everything || only?.contains(scene.rawValue) == true {
                await scene.apply(store: store, router: router, hub: hub, connectors: connectors, daemon: daemon, notifier: notifier)
                try? await Task.sleep(for: .seconds(1.0))
                let path = "\(dir)/\(scene.rawValue)-\(mode).png"
                await captureSubject(of: scene, to: path, store: store, router: router, openSettings: openSettings)
            }
        }
        // Leave the app as the next phase of the run expects it.
        await SnapScene.overview.apply(
            store: store, router: router, hub: hub, connectors: connectors, daemon: daemon, notifier: notifier)
        power.release()
    }

    /// Captures the real window with screencapture, so glass, sidebars, and
    /// materials look exactly as on screen. That needs Screen Recording
    /// permission for the app that started this one (your terminal, or Claude).
    /// Without it, falls back to drawing the view hierarchy, which leaves
    /// sidebars and glass blank.
    static func capture(to path: String, window: NSWindow? = nil) {
        // Another app can hide this one while snapshots run (computer use hides
        // apps it does not control). Hidden windows cannot be captured.
        if NSApp.isHidden { NSApp.unhideWithoutActivation() }
        guard let window = window ?? mainWindow else {
            print("SNAPSHOT no window")
            return
        }
        // Active, like a window someone is using: real accent colors and focus rings.
        NSApp.activate()
        window.makeKeyAndOrderFront(nil)
        let size = window.contentView?.bounds.size ?? .zero
        GridProbes.shared.write(to: path.replacingOccurrences(of: ".png", with: "-layout.json"), window: size)
        try? FileManager.default.removeItem(atPath: path)
        // The first capture of a run sometimes misses the window. Try three times.
        for _ in 1...3 {
            let shot = Process()
            shot.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            shot.arguments = ["-x", "-o", "-l", String(window.windowNumber), path]
            try? shot.run()
            shot.waitUntilExit()
            if FileManager.default.fileExists(atPath: path) {
                print("SNAPSHOT \(path)")
                return
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        guard let view = window.contentView?.superview ?? window.contentView,
              let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        if let data = rep.representation(using: .png, properties: [:]) {
            try? data.write(to: URL(fileURLWithPath: path))
            print("SNAPSHOT \(path) (fallback: no Screen Recording permission, sidebars and glass are blank)")
        }
    }
}

/// The IOPM assertions of a snapshot run: user activity to wake the display, and "no display sleep" to keep it on. Released
/// once, in a `defer`, when the run ends.
struct SnapshotPower {
    private var activity: IOPMAssertionID = 0
    private var awake: IOPMAssertionID = 0
    private var released = false

    init() {
        IOPMAssertionDeclareUserActivity("Snapshots" as CFString, kIOPMUserActiveLocal, &activity)
        IOPMAssertionCreateWithName(
            kIOPMAssertionTypePreventUserIdleDisplaySleep as CFString, IOPMAssertionLevel(kIOPMAssertionLevelOn),
            "Snapshots" as CFString, &awake)
    }

    mutating func release() {
        guard !released else { return }
        released = true
        if activity != 0 { IOPMAssertionRelease(activity) }
        if awake != 0 { IOPMAssertionRelease(awake) }
    }
}

/// Hosts the menu bar panel's view in a window, so snapshot mode and the debug bridge can capture what the system's
/// panel shows.
enum MenuBarCapture {
    static func show(store: AppStore, router: AppRouter) -> NSWindow {
        let host = NSHostingView(rootView: MenuBarView().environment(store).environment(router))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: Layout.popoverWidth, height: 240), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        // Closing must not free it a second time.
        window.isReleasedWhenClosed = false
        window.title = "Menu bar"
        window.contentView = host
        window.setContentSize(host.fittingSize)
        window.center()
        window.orderFrontRegardless()
        return window
    }
}
