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
    case newGoal = "new-goal"
    case newConnections = "new-connections"
    case newEnvironment = "new-environment"
    case newBudget = "new-budget"
    // The states the bundled daemon and the notifications add.
    case daemonStarting = "daemon-starting"
    case daemonApproval = "daemon-approval"
    case notificationPrompt = "notification-prompt"
    case connectorSignIn = "connector-signin"
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

    /// Puts the window in this scene's state.
    func apply(store: AppStore, router: AppRouter, daemon: DaemonController, notifier: AppNotifier) async {
        // Every scene starts from the calm state.
        daemon.preview(nil)
        notifier.preview(.allowed, prompt: false)
        await store.cancelGithubSignIn()
        router.show(.overview)
        router.rowModes = [:]
        router.rowNotes = [:]
        router.connectorSheet = nil
        router.templateProposal = nil
        router.advancedExpanded = false
        let mock = store.client as? MockRatchetClient
        await mock?.simulate(keychainAvailable: true)
        await mock?.simulate(preflight: nil)
        await store.refresh()
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
            case .connectorsAddGithub: router.connectorSheet = ConnectorSheetRequest(kind: .github, showsToken: true)
            case .connectorsAddAWS: router.connectorSheet = ConnectorSheetRequest(kind: .aws, pick: "sandbox-eu")
            case .connectorsAddAgent: router.connectorSheet = ConnectorSheetRequest(kind: .agent)
            case .connectorsEdit: router.rowModes["aws-acme-prod"] = .edit
            case .connectorsRename: router.rowModes["gh-acme-org"] = .rename
            case .connectorsCheck:
                router.rowModes["aws-acme-staging"] = .ssoHint
                router.rowNotes["gh-srikanth235"] = ConnectorNote(ok: true, text: "GitHub says you are signed in as srikanth235.")
                router.rowNotes["agent-codex-login"] = ConnectorNote(ok: false, text: "Codex is not signed in. Run `codex login` in a terminal.")
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
                    // Advanced (development), opened.
                    router.settingsTab = .general
                    router.advancedExpanded = true
                    try? await Task.sleep(for: .seconds(1))
                    let advanced = NSApp.windows.first {
                        $0.isVisible && $0 !== main && !$0.isSheet && $0.styleMask.contains(.titled)
                    }
                    capture(to: "\(dir)/settings-advanced-\(mode).png", window: advanced)
                    router.advancedExpanded = false
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
