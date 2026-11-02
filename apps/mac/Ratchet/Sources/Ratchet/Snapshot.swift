import AppKit
import IOKit.pwr_mgt
import RatchetKit
import SwiftUI

/// What snapshot mode shoots, one PNG per scene and mode.
enum SnapScene: String, CaseIterable {
    case overview, optimizations, benchmarks, connectors
    case detailLive = "detail-live"
    case detailResult = "detail-result"
    case detailNoWin = "detail-nowin"
    // The live log, the Result details and the benchmark run (a section far down its page, scrolled into view).
    case detailLiveLog = "detail-live-log"
    case detailResultTime = "detail-result-time"
    case detailResultSpend = "detail-result-spend"
    case benchmarksRun = "benchmarks-run"
    case benchmarksRunP99 = "benchmarks-run-p99"
    case newGoal = "new-goal"
    case newConnections = "new-connections"
    case newEnvironment = "new-environment"
    case newBudget = "new-budget"
    // The states the bundled daemon and the notifications add.
    case daemonStarting = "daemon-starting"
    case daemonApproval = "daemon-approval"
    case notificationPrompt = "notification-prompt"
    case connectorSignIn = "connector-signin"
    // A failed action: the dismissible banner above the page (Stop on an optimization that already ended).
    case errorBanner = "error-banner"
    // Setup: finding the repo, "What's missing", the Connectors forms, "Use template".
    case newRepoFound = "new-repo-found"
    case newRepoInvalid = "new-repo-invalid"
    case newRepoTemplate = "new-repo-template"
    case newLocalFolder = "new-local-folder"
    case newLocalNoRemote = "new-local-noremote"
    case newMissing = "new-missing"
    case connectorsAddGithub = "connectors-add-github"
    case connectorsAddAWS = "connectors-add-aws"
    case connectorsAddAgent = "connectors-add-agent"
    case connectorsEdit = "connectors-edit"
    case connectorsRename = "connectors-rename"
    case connectorsCheck = "connectors-check"
    case connectorsKeychain = "connectors-keychain"
    case templatePR = "template-pr"
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
        case .menuBar, .menuBarEmpty: .menuBar
        case .notificationsDenied: .settings
        default: .window
        }
    }

    /// Scenes that show the app with nothing in it.
    var isEmpty: Bool {
        switch self {
        case .emptyOverview, .emptyOptimizations, .emptyBenchmarks, .menuBarEmpty: true
        default: false
        }
    }

    /// Puts the window in this scene's state.
    func apply(
        store: AppStore, router: AppRouter, connectors: ConnectorsPageState, daemon: DaemonController, notifier: AppNotifier
    ) async {
        // Every scene starts from the calm state.
        SnapshotFocus.shared.target = nil
        SnapshotFocus.shared.metric = nil
        daemon.preview(nil)
        notifier.preview(.allowed, prompt: false)
        store.dismissError()
        await store.connectorStore.cancelGithubSignIn()
        KitGalleryState.shared.isShown = gallerySection != nil
        KitGalleryState.shared.section = gallerySection
        router.show(.overview)
        connectors.reset()
        router.templateProposal = nil
        router.advancedExpanded = false
        let mock = store.scenario
        await mock?.simulate(keychainAvailable: true)
        await mock?.simulate(preflight: nil)
        // Empty scenes empty the sample world; every other scene brings the sample data back.
        await mock?.simulate(empty: isEmpty)
        await store.refresh()
        switch self {
        case .overview, .emptyOverview, .menuBar, .menuBarEmpty, .galleryActions, .galleryChoices, .galleryRows, .galleryInputs,
            .galleryStatus, .galleryIdentity, .galleryType, .galleryTokens:
            break
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
        case .newRepoFound, .newRepoInvalid, .newRepoTemplate, .newLocalFolder, .newLocalNoRemote, .newMissing:
            let flow = NewOptimizationFlow(client: store.client)
            router.show(.optimizations)
            router.wizard = flow
            await flow.load()
            flow.goal = "Make checkout p95 faster"
            switch self {
            case .newRepoFound:
                flow.showsYAML = true
            case .newRepoInvalid:
                flow.repoQuery = "search"
                await flow.search()
                await flow.selectRepo("acme-org/search-svc")
            case .newRepoTemplate:
                await flow.selectRepo("acme-org/web-app")
                await flow.proposeTemplate("build-size")
            case .newLocalFolder:
                await flow.chooseLocalFolder("/Users/dev/shop-api")
            case .newLocalNoRemote:
                await flow.chooseLocalFolder("/Users/dev/scratch-no-remote")
            default:
                await mock?.simulate(preflight: [
                    PreflightItem(id: "codex", label: "Codex is not signed in", state: .fail, detail: "Run `codex login` in a terminal, or add an API key in Connectors."),
                    PreflightItem(id: "aws", label: "acme-staging needs signing in again", state: .warn, detail: "Run `aws sso login --profile acme-staging` before a long run."),
                ])
                await flow.go(to: .connections)
            }
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
        case .templatePR:
            router.show(.benchmarks)
            if let template = store.templates.first(where: { $0.id == "build-size" }) {
                let flow = TemplateProposalFlow(template: template, client: store.client)
                await flow.load()
                if let repo = flow.repos.first(where: { $0.id == "acme-org/web-app" }) { flow.select(repo) }
                await flow.openPullRequest()
                router.templateProposal = flow
            }
        case .newGoal, .newConnections, .newEnvironment, .newBudget:
            let flow = NewOptimizationFlow(client: store.client)
            router.show(.optimizations)
            router.wizard = flow
            await flow.load()
            flow.goal = "Make checkout p95 faster"
            switch self {
            case .newConnections:
                await flow.go(to: .connections)
            case .newEnvironment:
                await flow.go(to: .environment)
            case .newBudget:
                await flow.go(to: .environment)
                await flow.approveDraft()
                await flow.go(to: .budget)
            default:
                break
            }
        }
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
        store: AppStore, router: AppRouter, connectors: ConnectorsPageState, openSettings: OpenSettingsAction,
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
                dir: dir, everything: everything, only: only, store: store, router: router, connectors: connectors,
                openSettings: openSettings, daemon: daemon, notifier: notifier)
            if everything || only?.contains("settings") == true {
                await SnapScene.overview.apply(
                    store: store, router: router, connectors: connectors, daemon: daemon, notifier: notifier)
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
                    router.settingsTab = .general
                    router.advancedExpanded = true
                    try? await Task.sleep(for: .seconds(1))
                    let advanced = NSApp.windows.first {
                        $0.isVisible && $0 !== main && !$0.isSheet && $0.styleMask.contains(.titled)
                    }
                    capture(to: "\(dir)/settings-advanced-\(mode).png", window: advanced)
                    router.advancedExpanded = false
                    // Launch at login, waiting for the OK in System Settings.
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

    /// The app's main window: titled "Ratchet", not Settings and not a sheet.
    static var mainWindow: NSWindow? {
        NSApp.windows.first { $0.canBecomeMain && !$0.isSheet && $0.title == "Ratchet" }
            ?? NSApp.windows.first { $0.canBecomeMain && !$0.isSheet }
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
        dir: String, everything: Bool, only: Set<String>?, store: AppStore, router: AppRouter,
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
                await scene.apply(store: store, router: router, connectors: connectors, daemon: daemon, notifier: notifier)
                try? await Task.sleep(for: .seconds(1.0))
                let path = "\(dir)/\(scene.rawValue)-\(mode).png"
                await captureSubject(of: scene, to: path, store: store, router: router, openSettings: openSettings)
            }
        }
        // Leave the app as the next phase of the run expects it.
        await SnapScene.overview.apply(
            store: store, router: router, connectors: connectors, daemon: daemon, notifier: notifier)
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
