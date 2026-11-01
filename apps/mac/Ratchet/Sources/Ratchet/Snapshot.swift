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

    /// Puts the window in this scene's state.
    func apply(store: AppStore, router: AppRouter, daemon: DaemonController, notifier: AppNotifier) async {
        // Every scene starts from the calm state.
        SnapshotFocus.shared.target = nil
        SnapshotFocus.shared.metric = nil
        daemon.preview(nil)
        notifier.preview(.allowed, prompt: false)
        await store.cancelGithubSignIn()
        router.show(.overview)
        switch self {
        case .overview: break
        case .daemonStarting: daemon.preview(.starting)
        case .daemonApproval: daemon.preview(.needsApproval)
        case .notificationPrompt: notifier.preview(.notDetermined, prompt: true)
        case .connectorSignIn:
            router.show(.connectors)
            await store.startGithubSignIn()
        case .optimizations: router.show(.optimizations)
        case .benchmarks: router.show(.benchmarks)
        case .connectors: router.show(.connectors)
        case .detailLive: router.open(optimization: "opt-checkout")
        case .detailResult: router.open(optimization: "opt-forecast")
        case .detailNoWin: router.open(optimization: "opt-bundle")
        case .detailLiveLog:
            router.open(optimization: "opt-checkout")
            await store.loadDetail(id: "opt-checkout")
            await store.loadAgentLog("agent-claude-login")
            SnapshotFocus.shared.target = "log"
        case .detailResultTime:
            router.open(optimization: "opt-cli")
            await store.loadDetail(id: "opt-cli")
            await store.loadInsights("opt-cli")
            SnapshotFocus.shared.target = "time"
        case .detailResultSpend:
            router.open(optimization: "opt-forecast")
            await store.loadDetail(id: "opt-forecast")
            await store.loadInsights("opt-forecast")
            SnapshotFocus.shared.target = "spend"
        case .benchmarksRun, .benchmarksRunP99:
            router.show(.benchmarks)
            await store.loadBenchmark("opt-checkout", metric: nil)
            if self == .benchmarksRunP99 { SnapshotFocus.shared.metric = "p99" }
            SnapshotFocus.shared.target = "run"
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
/// finished and without a win, the four New optimization steps), and settings-<tab>-light.png and -dark.png for
/// every Settings tab, then quits. Each PNG gets a <scene>-<mode>-layout.json with the grid probes
/// (tools/check-layout.swift).
/// APP_SNAPSHOT_PAGES=overview,new-budget limits the scenes. "all" means every scene; add "settings" for Settings.
enum Snapshot {
    static func runIfRequested(
        store: AppStore, router: AppRouter, openSettings: OpenSettingsAction, daemon: DaemonController, notifier: AppNotifier
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
            // A sleeping display gives screencapture nothing. Declaring user
            // activity wakes it; the assertion keeps it on until the run quits.
            var activity: IOPMAssertionID = 0
            IOPMAssertionDeclareUserActivity("Snapshots" as CFString, kIOPMUserActiveLocal, &activity)
            var awake: IOPMAssertionID = 0
            IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep as CFString,
                                        IOPMAssertionLevel(kIOPMAssertionLevelOn), "Snapshots" as CFString, &awake)
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            try? await Task.sleep(for: .seconds(1.5))
            for mode in ["light", "dark"] {
                NSApp.appearance = NSAppearance(named: mode == "light" ? .aqua : .darkAqua)
                for scene in SnapScene.allCases where everything || only?.contains(scene.rawValue) == true {
                    await scene.apply(store: store, router: router, daemon: daemon, notifier: notifier)
                    try? await Task.sleep(for: .seconds(1.0))
                    capture(to: "\(dir)/\(scene.rawValue)-\(mode).png")
                }
            }
            if everything || only?.contains("settings") == true {
                await SnapScene.overview.apply(store: store, router: router, daemon: daemon, notifier: notifier)
                let main = NSApp.windows.first { $0.canBecomeMain && !$0.isSheet }
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
                }
            }
            print("SNAPSHOT done \(dir)")
            // An open sheet refuses terminate, and the run would wait forever.
            for window in NSApp.windows { if let sheet = window.attachedSheet { window.endSheet(sheet) } }
            NSApp.terminate(nil)
        }
    }

    /// Captures the real window with screencapture, so glass, sidebars, and
    /// materials look exactly as on screen. That needs Screen Recording
    /// permission for the app that started this one (your terminal, or Claude).
    /// Without it, falls back to drawing the view hierarchy, which leaves
    /// sidebars and glass blank.
    private static func capture(to path: String, window: NSWindow? = nil) {
        // Another app can hide this one while snapshots run (computer use hides
        // apps it does not control). Hidden windows cannot be captured.
        if NSApp.isHidden { NSApp.unhideWithoutActivation() }
        guard let window = window ?? NSApp.windows.first(where: { $0.canBecomeMain && !$0.isSheet }) else {
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
