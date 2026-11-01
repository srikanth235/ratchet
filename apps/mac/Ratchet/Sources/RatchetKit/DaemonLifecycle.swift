import Foundation

/// What the system says about a registered LaunchAgent (`SMAppService.Status`, without importing ServiceManagement here).
public enum AgentRegistration: Sendable, Equatable {
    case notRegistered
    case enabled
    /// The user must allow it in System Settings, Login Items.
    case requiresApproval
    /// The plist is not in the app bundle (a development build without the helper).
    case notFound
}

/// Which daemon the LaunchAgent runs: the real engine, or the simulation with demo data.
public enum DaemonMode: String, Sendable, Equatable, Codable {
    case real, demo

    /// The plist in `Contents/Library/LaunchAgents/`.
    public var plistName: String { self == .real ? "com.ratchet.ratchetd.plist" : "com.ratchet.ratchetd.demo.plist" }

    public var other: DaemonMode { self == .real ? .demo : .real }
}

public enum DaemonPhase: Sendable, Equatable {
    /// Asking the system whether the agent is registered.
    case checking
    /// Registered, but the user has to allow it in Login Items.
    case needsApproval
    /// Registered; waiting for the socket to accept connections.
    case starting
    case running
    /// The user stopped it from Settings.
    case stopped
    case failed(String)

    public var isRunning: Bool { self == .running }
}

public enum DaemonLifecycleEvent: Sendable, Equatable {
    /// The app launched (or the controller was created) and read the registration.
    case launched(AgentRegistration)
    case registered(AgentRegistration)
    case registrationFailed(String)
    /// The registration was read again (the app became active while waiting for approval).
    case registrationChecked(AgentRegistration)
    case probed(reachable: Bool)
    case stopRequested
    case restartRequested
    case modeChanged(DaemonMode)
    case startRequested
}

public enum DaemonCommand: Sendable, Equatable {
    case register
    case unregister
    /// Unregister, then register again (a restart; the system starts it).
    case restart
    /// Look at the socket after `delay` seconds.
    case probe(after: Double)
    case checkRegistration
    /// Unregister the other mode's agent and register this mode's.
    case switchMode(DaemonMode)
}

/// The daemon's lifecycle as a plain state machine: events in, a phase and commands out. The controller in the app turns
/// commands into `SMAppService` and socket calls and feeds the results back as events, so the rules are testable
/// without a Mac.
public struct DaemonLifecycle: Sendable, Equatable {
    public private(set) var phase: DaemonPhase = .checking
    public private(set) var mode: DaemonMode
    /// Probes in a row that found no daemon while `starting`.
    public private(set) var attempts = 0

    /// About 40 seconds of waiting before the start counts as failed.
    public static let maxAttempts = 40

    public init(mode: DaemonMode = .real, phase: DaemonPhase = .checking) {
        self.mode = mode
        self.phase = phase
    }

    public mutating func handle(_ event: DaemonLifecycleEvent) -> [DaemonCommand] {
        switch event {
        case .launched(let registration):
            switch registration {
            case .enabled: return begin(probeAfter: 0)
            case .notRegistered: phase = .starting; attempts = 0; return [.register]
            case .requiresApproval: phase = .needsApproval; return [.probe(after: 0)]
            case .notFound:
                phase = .failed("Ratchet's background service is missing from this copy of the app.")
                return [.probe(after: 0)]
            }
        case .registered(let registration), .registrationChecked(let registration):
            switch registration {
            case .enabled:
                if phase == .running { return [] }
                return begin(probeAfter: 0)
            case .requiresApproval:
                phase = .needsApproval
                return [.probe(after: 2)]
            case .notRegistered:
                if phase == .stopped { return [] }
                phase = .failed("Ratchet's background service could not be turned on.")
                return []
            case .notFound:
                phase = .failed("Ratchet's background service is missing from this copy of the app.")
                return []
            }
        case .registrationFailed(let message):
            phase = .failed(message)
            return []
        case .probed(let reachable):
            switch phase {
            case .running:
                if reachable { return [.probe(after: 5)] }
                // It went away (a crash, a restart): the system restarts it; wait quietly.
                phase = .starting
                attempts = 1
                return [.probe(after: 1)]
            case .starting:
                if reachable {
                    phase = .running
                    attempts = 0
                    return [.probe(after: 5)]
                }
                attempts += 1
                if attempts > Self.maxAttempts {
                    phase = .failed("Ratchet's background service did not start.")
                    return []
                }
                return [.probe(after: attempts < 5 ? 0.5 : 1)]
            case .needsApproval:
                // The socket may answer anyway (a daemon started by hand); otherwise ask again whether it was approved.
                if reachable { phase = .running; return [.probe(after: 5)] }
                return [.checkRegistration]
            case .checking, .failed:
                if reachable { phase = .running; return [.probe(after: 5)] }
                return []
            case .stopped:
                return []
            }
        case .stopRequested:
            phase = .stopped
            attempts = 0
            return [.unregister]
        case .restartRequested:
            phase = .starting
            attempts = 0
            return [.restart]
        case .startRequested:
            phase = .starting
            attempts = 0
            return [.register]
        case .modeChanged(let newMode):
            guard newMode != mode else { return [] }
            mode = newMode
            phase = .starting
            attempts = 0
            return [.switchMode(newMode)]
        }
    }

    private mutating func begin(probeAfter delay: Double) -> [DaemonCommand] {
        phase = .starting
        attempts = 0
        return [.probe(after: delay)]
    }
}
