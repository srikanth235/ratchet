import AppKit
import Observation
import RatchetKit
import ServiceManagement

/// Runs `ratchetd` as a LaunchAgent registered with `SMAppService` and tells the app where it stands. The rules live in
/// `DaemonLifecycle` (RatchetKit, tested); this class turns its commands into system calls and feeds the answers back.
///
/// Three situations:
/// - managed: the app bundle carries the agent plists (`Contents/Library/LaunchAgents/`). The controller registers the
///   agent on first launch and keeps it running.
/// - external: a daemon already answers on the socket (`RATCHET_HOME` / `RATCHET_SOCKET` from a dev loop or CI). The
///   controller only watches the socket.
/// - unbundled: the bundle has no agent (a build with `RATCHET_SKIP_DAEMON=1`, a failed helper build, or a plain
///   `swift run`) and nothing points the app at a daemon. The controller only watches the socket, and says so at once:
///   the status screen explains that this build has no background service.
/// - none: test and snapshot mode, sample data. Nothing to run; the phase is `running`.
@MainActor @Observable
final class DaemonController {
    enum Management { case managed, external, unbundled, none }

    private(set) var lifecycle: DaemonLifecycle
    let location: DaemonLocation
    let management: Management
    @ObservationIgnored private var probeTask: Task<Void, Never>?
    /// The registration work in flight (launch, unregister, restart, switch). A newer command cancels the older one, so a
    /// restart and a switch to demo mode cannot run their unregister and register steps into each other.
    @ObservationIgnored private var registrationTask: Task<Void, Never>?
    @ObservationIgnored private var started = false
    @ObservationIgnored private var versionChecked = false

    /// Set when the app restarted the agent because it ran an older `ratchetd` than the one inside this build.
    private(set) var versionNote: String?

    /// This build has no `ratchetd` inside and nothing points the app at one: it cannot start a background service.
    var isUnbundled: Bool { management == .unbundled || unbundledPreview }
    private var unbundledPreview = false

    /// Snapshot mode: show a status screen whatever the data says.
    private(set) var statusForced = false

    var phase: DaemonPhase { lifecycle.phase }

    /// The status screen replaces the window while the daemon is not answering and there is nothing to show yet; a
    /// daemon that needs the user, failed or was stopped always shows it.
    func showsStatusScreen(storeLoaded: Bool) -> Bool {
        if statusForced { return true }
        switch phase {
        case .running, .checking: return false
        // With data on screen the page stays and a banner on every page says the service is down; without, the status screen.
        case .starting, .needsApproval, .failed, .stopped: return !storeLoaded
        }
    }

    /// Snapshot mode: pretend the daemon is in this phase.
    /// `statusScreen: false` shows the phase as the banner over the page does (the data stays on screen).
    func preview(_ phase: DaemonPhase?, statusScreen: Bool = true) {
        unbundledPreview = false
        if let phase {
            lifecycle = DaemonLifecycle(mode: mode, phase: phase)
            statusForced = statusScreen
        } else {
            lifecycle = DaemonLifecycle.running(mode: mode)
            statusForced = false
        }
    }
    /// Snapshot mode: the status screen of a build that has no `ratchetd` inside.
    func previewUnbundled() {
        preview(.failed("Ratchet's background service is missing from this copy of the app."))
        unbundledPreview = true
    }

    var mode: DaemonMode { lifecycle.mode }

    static let modeKey = "daemonMode"

    init(location: DaemonLocation, management: Management, mode: DaemonMode) {
        self.location = location
        self.management = management
        self.lifecycle = DaemonLifecycle(mode: mode)
        if management == .none { self.lifecycle = DaemonLifecycle.running(mode: mode) }
    }

    /// The controller for this launch: sample data in test mode, otherwise managed when the bundle has the plist.
    static func make() -> DaemonController {
        let mode: DaemonMode = UserDefaults.standard.string(forKey: modeKey) == DaemonMode.demo.rawValue ? .demo : .real
        let env = ProcessInfo.processInfo.environment
        if Store.isOverridden || env["APP_DEMO"] == "1" {
            return DaemonController(location: .standard(), management: .none, mode: mode)
        }
        let location = DaemonLocation.standard()
        // A dev loop or CI points the app at a daemon it started itself.
        let external = env["RATCHET_HOME"] != nil || env["RATCHET_SOCKET"] != nil
        if external { return DaemonController(location: location, management: .external, mode: mode) }
        if !bundleHasAgent(mode) {
            FileHandle.standardError.write(Data(
                "Ratchet: this build has no bundled ratchetd (Contents/Library/LaunchAgents/\(mode.plistName) is missing). Rebuild with ./build.sh and Rust installed.\n".utf8))
            return DaemonController(location: location, management: .unbundled, mode: mode)
        }
        return DaemonController(location: location, management: .managed, mode: mode)
    }

    private static func bundleHasAgent(_ mode: DaemonMode) -> Bool {
        let plist = Bundle.main.bundleURL.appending(path: "Contents/Library/LaunchAgents/" + mode.plistName)
        return FileManager.default.fileExists(atPath: plist.path)
    }

    // MARK: Driving

    /// Called once when the window appears.
    func start() {
        guard !started, management != .none else { return }
        started = true
        registrationTask = Task { [weak self] in
            guard let self else { return }
            // A daemon that already answers needs no registration (a dev daemon, or the agent from the last launch).
            if await self.reachable() {
                run(.probed(reachable: true))
            } else if management == .managed {
                let current = registration(of: mode)
                run(.launched(current))
                if current == .enabled { restartIfStillDown() }
            } else if management == .unbundled {
                // Nothing can start a daemon from this build: say so now, and keep watching the socket.
                run(.launched(.notFound))
            } else {
                // An external daemon that is not up yet is waited for quietly, like a managed one.
                run(.launched(.enabled))
            }
        }
    }

    /// The agent is registered and approved, yet nothing answers: launchd no longer has the job (`build.sh --open` stops it
    /// with `launchctl bootout` before it replaces the bundle, and a reboot can lose it). Registering again loads it from
    /// this bundle. Once, a few seconds after launch, and only while the app is still waiting for the daemon.
    private func restartIfStillDown() {
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(4))
            guard let self, self.phase == .starting, !(await self.reachable()) else { return }
            self.versionChecked = true  // the restart starts this bundle's daemon; there is nothing to compare
            self.restart()
        }
    }

    func restart() { run(.restartRequested) }
    func stop() { run(.stopRequested) }
    func startAgain() { run(.startRequested) }

    func setMode(_ new: DaemonMode) {
        UserDefaults.standard.set(new.rawValue, forKey: Self.modeKey)
        run(.modeChanged(new))
    }

    func openLoginItems() {
        SMAppService.openSystemSettingsLoginItems()
    }

    /// The app came forward again (the user is back from System Settings or a terminal): look at the agent's approval
    /// and at the socket again, whatever the phase says. Probing restarts the lifecycle's own probe chain, never doubles it.
    func recheck() {
        guard management != .none, started else { return }
        if management == .managed, phase == .needsApproval {
            run(.registrationChecked(registration(of: mode)))
        } else if phase != .stopped {
            probeNow()
        }
    }

    /// Look at the socket now (the status screen's "Check again" on a build without a daemon).
    func probeNow() {
        guard management != .none else { return }
        execute(.probe(after: 0))
    }

    private func run(_ event: DaemonLifecycleEvent) {
        for command in lifecycle.handle(event) { execute(command) }
        if case .probed(reachable: true) = event, phase == .running { checkVersionOnce() }
    }

    // MARK: Version

    /// A new build can find the previous build's `ratchetd` still running: the LaunchAgent outlives the app. Once per
    /// launch, ask the running daemon for its version and compare it with the one inside this bundle; restart the agent
    /// when they differ, so the new app never talks to an old daemon.
    private func checkVersionOnce() {
        guard management == .managed, !versionChecked else { return }
        versionChecked = true
        let location = self.location
        Task { [weak self] in
            let running = try? await DaemonRatchetClient(location: location).daemonVersion()
            let bundled = await Task.detached { Self.bundledVersion() }.value
            guard let self else { return }
            let check = DaemonVersionCheck.compare(running: running, bundled: bundled)
            if case .mismatch(let old, let new) = check {
                self.versionNote = "Restarted the background service: it was \(old), this build has \(new)."
                self.restart()
            }
        }
    }

    /// The version of the `ratchetd` inside this app (`ratchetd --version`), or nil when it cannot be run.
    nonisolated static func bundledVersion() -> String? {
        let binary = Bundle.main.bundleURL.appending(path: "Contents/Helpers/ratchetd")
        guard FileManager.default.isExecutableFile(atPath: binary.path) else { return nil }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["--version"]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        return DaemonVersionCheck.parse(versionOutput: String(decoding: data, as: UTF8.self))
    }

    private func execute(_ command: DaemonCommand) {
        switch command {
        case .probe(let delay):
            probeTask?.cancel()
            probeTask = Task { [weak self] in
                if delay > 0 { try? await Task.sleep(for: .seconds(delay)) }
                guard !Task.isCancelled, let self else { return }
                let ok = await self.reachable()
                guard !Task.isCancelled else { return }
                self.run(.probed(reachable: ok))
            }
        case .checkRegistration:
            guard management == .managed else { return }
            run(.registrationChecked(registration(of: mode)))
        case .register:
            // A daemon started by hand cannot be registered; look at the socket instead of waiting for nothing.
            guard management == .managed else { if management != .none { execute(.probe(after: 0.5)) }; return }
            register(mode)
        case .unregister:
            guard management == .managed else { return }
            let current = mode
            registrationTask?.cancel()
            registrationTask = Task { try? await SMAppService.agent(plistName: current.plistName).unregister() }
        case .restart:
            guard management == .managed else { return }
            let current = mode
            registrationTask?.cancel()
            registrationTask = Task { [weak self] in
                try? await SMAppService.agent(plistName: current.plistName).unregister()
                try? await Task.sleep(for: .seconds(1))
                if !Task.isCancelled { self?.register(current) }
            }
        case .switchMode(let new):
            guard management == .managed else { return }
            registrationTask?.cancel()
            registrationTask = Task { [weak self] in
                try? await SMAppService.agent(plistName: new.other.plistName).unregister()
                try? await Task.sleep(for: .seconds(1))
                if !Task.isCancelled { self?.register(new) }
            }
        }
    }

    private func register(_ target: DaemonMode) {
        let service = SMAppService.agent(plistName: target.plistName)
        do {
            try service.register()
            run(.registered(map(service.status)))
        } catch {
            // Asking the user to allow it in Login Items is reported as an error, with the status `requiresApproval`.
            if service.status == .requiresApproval {
                run(.registered(.requiresApproval))
            } else {
                run(.registrationFailed(error.localizedDescription))
            }
        }
    }

    private func registration(of target: DaemonMode) -> AgentRegistration {
        map(SMAppService.agent(plistName: target.plistName).status)
    }

    private func map(_ status: SMAppService.Status) -> AgentRegistration {
        switch status {
        case .enabled: .enabled
        case .requiresApproval: .requiresApproval
        case .notFound: .notFound
        default: .notRegistered
        }
    }

    private func reachable() async -> Bool {
        let location = self.location
        return await Task.detached { location.isReachable }.value
    }
}

extension DaemonLifecycle {
    /// A lifecycle that is already running (sample data needs no daemon).
    static func running(mode: DaemonMode) -> DaemonLifecycle {
        var lifecycle = DaemonLifecycle(mode: mode)
        _ = lifecycle.handle(.probed(reachable: true))
        return lifecycle
    }
}
