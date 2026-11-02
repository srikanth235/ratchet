import AppKit
import RatchetKit
import SwiftUI

/// The debug bridge: a command channel that lets an agent (or a script) drive a running debug build and read its state.
/// Compiled in debug builds only (`./build.sh --debug`); a release build has no channel at all.
///
///   tools/ratchetctl.sh status                  JSON: page, selected optimization, daemon phase, notification auth, ...
///   tools/ratchetctl.sh open:connectors         a page, or an optimization id (the same names as `ratchet open`)
///   tools/ratchetctl.sh appearance:dark         light | dark
///   tools/ratchetctl.sh snapshot:menubar        a window PNG under <home>/bridge/snapshots/ (test mode)
///   tools/ratchetctl.sh selftest                the self-test, inside the running app (test mode, once per launch)
///
/// The protocol (`BridgeCommand`, `BridgeReply`, `BridgeChannel`) is in RatchetKit with tests. A command is a file in
/// `<home>/bridge/inbox/`, the answer a JSON file in `<home>/bridge/outbox/`; `<home>` is `$RATCHET_HOME`, else the
/// daemon's support directory. Scenes and the self-test change the sample data, so they only run with `APP_STORE` set.
#if DEBUG
@MainActor
final class DebugBridge {
    private static var running: DebugBridge?

    static func start(
        store: AppStore, router: AppRouter, connectors: ConnectorsPageState, daemon: DaemonController, notifier: AppNotifier,
        openSettings: OpenSettingsAction
    ) {
        guard running == nil else { return }
        let bridge = DebugBridge(
            store: store, router: router, connectors: connectors, daemon: daemon, notifier: notifier, openSettings: openSettings)
        running = bridge
        bridge.begin()
    }

    private let store: AppStore
    private let router: AppRouter
    private let connectors: ConnectorsPageState
    private let daemon: DaemonController
    private let notifier: AppNotifier
    private let openSettings: OpenSettingsAction
    private let channel = BridgeChannel()
    private var ranSelfTest = false

    private init(
        store: AppStore, router: AppRouter, connectors: ConnectorsPageState, daemon: DaemonController, notifier: AppNotifier,
        openSettings: OpenSettingsAction
    ) {
        self.store = store
        self.router = router
        self.connectors = connectors
        self.daemon = daemon
        self.notifier = notifier
        self.openSettings = openSettings
    }

    private func begin() {
        let manager = FileManager.default
        for directory in [channel.inbox, channel.outbox, channel.snapshots] {
            try? manager.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        // Commands left over from an earlier run are stale.
        for url in (try? manager.contentsOfDirectory(at: channel.inbox, includingPropertiesForKeys: nil)) ?? [] {
            try? manager.removeItem(at: url)
        }
        writeStatusFile()
        try? Data("\(ProcessInfo.processInfo.processIdentifier)\n".utf8).write(to: channel.readyFile)
        FileHandle.standardError.write(Data("Ratchet debug bridge: \(channel.root.path)\n".utf8))
        Task { await loop() }
    }

    private func loop() async {
        while !Task.isCancelled {
            await drain()
            try? await Task.sleep(for: .milliseconds(150))
        }
    }

    /// Runs the waiting commands, oldest name first, one at a time.
    private func drain() async {
        let manager = FileManager.default
        let names = ((try? manager.contentsOfDirectory(atPath: channel.inbox.path)) ?? []).sorted()
        for name in names {
            guard let id = BridgeChannel.id(ofCommandFile: name) else { continue }
            let file = channel.inbox.appending(path: name)
            let line = (try? String(contentsOf: file, encoding: .utf8)) ?? ""
            try? manager.removeItem(at: file)
            let reply: BridgeReply
            if let command = BridgeCommand(line: line) {
                reply = await run(command)
            } else {
                reply = BridgeReply(
                    ok: false, command: line.trimmingCharacters(in: .whitespacesAndNewlines),
                    message: "Unknown command. Use status, open:<page|id>, appearance:light|dark, snapshot:<scene>, selftest.")
            }
            try? reply.json().write(to: channel.replyFile(id: id), options: .atomic)
        }
    }

    // MARK: Commands

    private func run(_ command: BridgeCommand) async -> BridgeReply {
        switch command {
        case .status:
            writeStatusFile()
            return BridgeReply(ok: true, command: command.line, message: "ok", status: status())
        case .open(let target):
            return open(target, command: command)
        case .appearance(let appearance):
            NSApp.appearance = NSAppearance(named: appearance == .dark ? .darkAqua : .aqua)
            return BridgeReply(ok: true, command: command.line, message: "appearance is \(appearance.rawValue)", status: status())
        case .snapshot(let name):
            return await snapshot(name, command: command)
        case .selftest:
            return await selftest(command: command)
        }
    }

    private func open(_ target: String, command: BridgeCommand) -> BridgeReply {
        let pages = Page.sidebar.map(\.rawValue)
        switch AppLink.open(target).resolve(pages: pages) {
        case .page(let name):
            guard let page = Page(rawValue: name) else {
                return BridgeReply(ok: false, command: command.line, message: "No page named \(name).")
            }
            router.show(page)
        case .destination(let destination):
            notifier.open(destination)
        }
        return BridgeReply(ok: true, command: command.line, message: "opened \(target)", status: status())
    }

    private func snapshot(_ name: String, command: BridgeCommand) async -> BridgeReply {
        guard Store.isOverridden else {
            return BridgeReply(ok: false, command: command.line, message: "Scenes change the sample data: start the app with APP_STORE=/tmp/<file>.")
        }
        guard let scene = SnapScene(rawValue: name) else {
            let known = SnapScene.allCases.map(\.rawValue).joined(separator: ", ")
            return BridgeReply(ok: false, command: command.line, message: "No scene named \(name). Scenes: \(known)")
        }
        await scene.apply(store: store, router: router, connectors: connectors, daemon: daemon, notifier: notifier)
        try? await Task.sleep(for: .seconds(1))
        let path = channel.snapshots.appending(path: "\(scene.rawValue)-\(appearanceName).png").path
        try? FileManager.default.removeItem(atPath: path)
        await Snapshot.captureSubject(of: scene, to: path, store: store, router: router, openSettings: openSettings)
        let written = FileManager.default.fileExists(atPath: path)
        return BridgeReply(
            ok: written, command: command.line, message: written ? "saved \(path)" : "the capture failed (no window, or no Screen Recording permission)",
            status: status(), files: written ? [path] : [])
    }

    private func selftest(command: BridgeCommand) async -> BridgeReply {
        guard Store.isOverridden else {
            return BridgeReply(ok: false, command: command.line, message: "The self-test changes the sample data: start the app with APP_STORE=/tmp/<file>.")
        }
        guard !ranSelfTest else {
            return BridgeReply(ok: false, command: command.line, message: "The self-test already ran in this launch and left the sample data changed. Quit and open the app again.")
        }
        ranSelfTest = true
        let result = await SelfTest.runNow(store: store, router: router, notifier: notifier)
        return BridgeReply(
            ok: result.failures == 0, command: command.line,
            message: result.failures == 0 ? "SELFTEST PASS \(result.checks) checks" : "SELFTEST FAIL \(result.failures) of \(result.checks) checks",
            status: status(), checks: result.checks, failures: result.failures, lines: result.lines)
    }

    // MARK: Status

    private var appearanceName: String {
        NSApp.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? "dark" : "light"
    }

    private func status() -> BridgeStatus {
        BridgeStatus(
            page: router.page.rawValue, selectedOptimization: router.optimizationID, wizardOpen: router.wizard != nil,
            daemonPhase: daemon.phase.bridgeName, daemonDetail: daemon.phase.failureDetail ?? daemon.versionNote,
            notificationAuth: notifier.authorization.bridgeName, appearance: appearanceName,
            optimizations: store.optimizationStore.optimizations.count, running: store.optimizationStore.runningOptimizations.count, needsYou: store.needsYouCount,
            lastError: store.lastError, testMode: Store.isOverridden, pid: ProcessInfo.processInfo.processIdentifier)
    }

    private func writeStatusFile() {
        let reply = BridgeReply(ok: true, command: "status", message: "ok", status: status())
        try? reply.json().write(to: channel.statusFile, options: .atomic)
    }
}

extension AppNotifier.Authorization {
    /// The state as a plain word, for the bridge's `status`.
    var bridgeName: String {
        switch self {
        case .unknown: "unknown"
        case .notDetermined: "notDetermined"
        case .denied: "denied"
        case .allowed: "allowed"
        }
    }
}
#else
/// Release builds have no debug bridge.
enum DebugBridge {
    static func start(
        store: AppStore, router: AppRouter, connectors: ConnectorsPageState, daemon: DaemonController, notifier: AppNotifier,
        openSettings: OpenSettingsAction
    ) {}
}
#endif
