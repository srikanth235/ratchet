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
/// - external: a daemon already answers on the socket (`RATCHET_HOME` / `RATCHET_SOCKET` from a dev loop or CI), or the
///   bundle has no agent (a plain `swift run`). The controller only watches the socket.
/// - none: test and snapshot mode, sample data. Nothing to run; the phase is `running`.
@MainActor @Observable
final class DaemonController {
    enum Management { case managed, external, none }

    private(set) var lifecycle: DaemonLifecycle
    let location: DaemonLocation
    let management: Management
    @ObservationIgnored private var probeTask: Task<Void, Never>?
    @ObservationIgnored private var started = false

    /// Snapshot mode: show a status screen whatever the data says.
    private(set) var statusForced = false

    var phase: DaemonPhase { lifecycle.phase }

    /// The status screen replaces the window while the daemon is not answering and there is nothing to show yet; a
    /// daemon that needs the user, failed or was stopped always shows it.
    func showsStatusScreen(storeLoaded: Bool) -> Bool {
        if statusForced { return true }
        switch phase {
        case .running, .checking: return false
        case .starting: return !storeLoaded
        case .needsApproval, .failed, .stopped: return true
        }
    }

    /// Snapshot mode: pretend the daemon is in this phase.
    func preview(_ phase: DaemonPhase?) {
        if let phase {
            lifecycle = DaemonLifecycle(mode: mode, phase: phase)
            statusForced = true
        } else {
            lifecycle = DaemonLifecycle.running(mode: mode)
            statusForced = false
        }
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
        if external || !bundleHasAgent(mode) {
            return DaemonController(location: location, management: .external, mode: mode)
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
        Task {
            // A daemon that already answers needs no registration (a dev daemon, or the agent from the last launch).
            if await reachable() {
                run(.probed(reachable: true))
            } else if management == .managed {
                run(.launched(registration(of: mode)))
            } else {
                // An external daemon that is not up yet is waited for quietly, like a managed one.
                run(.launched(.enabled))
            }
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

    /// The system's view of the agent changed (the user came back from System Settings).
    func recheck() {
        guard management == .managed, phase == .needsApproval else { return }
        run(.registrationChecked(registration(of: mode)))
    }

    private func run(_ event: DaemonEvent) {
        for command in lifecycle.handle(event) { execute(command) }
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
            guard management == .managed else { return }
            register(mode)
        case .unregister:
            guard management == .managed else { return }
            let current = mode
            Task { try? await SMAppService.agent(plistName: current.plistName).unregister() }
        case .restart:
            guard management == .managed else { return }
            let current = mode
            Task {
                try? await SMAppService.agent(plistName: current.plistName).unregister()
                try? await Task.sleep(for: .seconds(1))
                register(current)
            }
        case .switchMode(let new):
            guard management == .managed else { return }
            Task {
                try? await SMAppService.agent(plistName: new.other.plistName).unregister()
                try? await Task.sleep(for: .seconds(1))
                register(new)
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
