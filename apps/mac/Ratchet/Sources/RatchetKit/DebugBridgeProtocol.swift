import Foundation

/// The debug bridge's protocol: how a script (or an agent) drives a running debug build of the app.
///
/// A command is a file with one line in `<home>/bridge/inbox/<id>.cmd`; the app answers with a JSON file in
/// `<home>/bridge/outbox/<id>.json` and removes the command. `status` also leaves `<home>/bridge/status.json`. `<home>`
/// is `$RATCHET_HOME`, else the daemon's support directory. The channel exists only in debug builds
/// (`apps/mac/Ratchet/CLAUDE.md`, "Debug bridge"; `tools/ratchetctl.sh` is the client).
public enum BridgeCommand: Equatable, Sendable {
    case status
    /// A page name or an optimization id (see `AppLink`).
    case open(String)
    case appearance(BridgeAppearance)
    /// A snapshot scene: window PNG in `<home>/bridge/snapshots/`.
    case snapshot(String)
    case selftest

    public init?(line: String) {
        let text = line.trimmingCharacters(in: .whitespacesAndNewlines)
        switch text {
        case "status": self = .status
        case "selftest": self = .selftest
        default:
            guard let colon = text.firstIndex(of: ":") else { return nil }
            let verb = text[..<colon]
            let argument = text[text.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            guard !argument.isEmpty else { return nil }
            switch verb {
            case "open": self = .open(argument)
            case "appearance":
                guard let appearance = BridgeAppearance(rawValue: argument) else { return nil }
                self = .appearance(appearance)
            case "snapshot": self = .snapshot(argument)
            default: return nil
            }
        }
    }

    /// The command as the line a client writes.
    public var line: String {
        switch self {
        case .status: "status"
        case .open(let target): "open:" + target
        case .appearance(let appearance): "appearance:" + appearance.rawValue
        case .snapshot(let scene): "snapshot:" + scene
        case .selftest: "selftest"
        }
    }
}

public enum BridgeAppearance: String, Codable, Sendable, CaseIterable {
    case light, dark
}

/// What the app is showing, for `status`.
public struct BridgeStatus: Codable, Equatable, Sendable {
    public var page: String
    public var selectedOptimization: String?
    public var wizardOpen: Bool
    /// `checking`, `needsApproval`, `starting`, `running`, `stopped` or `failed`.
    public var daemonPhase: String
    public var daemonDetail: String?
    /// `unknown`, `notDetermined`, `denied` or `allowed`.
    public var notificationAuth: String
    public var appearance: String
    public var optimizations: Int
    public var running: Int
    public var needsYou: Int
    public var lastError: String?
    /// True when the app runs on the sample client (`APP_STORE`): scenes and the self-test are allowed.
    public var testMode: Bool
    public var pid: Int32

    public init(
        page: String, selectedOptimization: String?, wizardOpen: Bool, daemonPhase: String, daemonDetail: String? = nil,
        notificationAuth: String, appearance: String, optimizations: Int, running: Int, needsYou: Int, lastError: String?,
        testMode: Bool, pid: Int32
    ) {
        self.page = page
        self.selectedOptimization = selectedOptimization
        self.wizardOpen = wizardOpen
        self.daemonPhase = daemonPhase
        self.daemonDetail = daemonDetail
        self.notificationAuth = notificationAuth
        self.appearance = appearance
        self.optimizations = optimizations
        self.running = running
        self.needsYou = needsYou
        self.lastError = lastError
        self.testMode = testMode
        self.pid = pid
    }
}

/// The answer to one command.
public struct BridgeReply: Codable, Equatable, Sendable {
    public var ok: Bool
    public var command: String
    /// A sentence: what happened, or why it did not.
    public var message: String
    public var status: BridgeStatus?
    /// Files written (snapshot PNGs).
    public var files: [String]
    /// `selftest`: how many checks ran and failed, and the PASS / FAIL lines.
    public var checks: Int?
    public var failures: Int?
    public var lines: [String]?

    public init(
        ok: Bool, command: String, message: String, status: BridgeStatus? = nil, files: [String] = [], checks: Int? = nil,
        failures: Int? = nil, lines: [String]? = nil
    ) {
        self.ok = ok
        self.command = command
        self.message = message
        self.status = status
        self.files = files
        self.checks = checks
        self.failures = failures
        self.lines = lines
    }

    public func json() -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return (try? encoder.encode(self)) ?? Data("{\"ok\":false}".utf8)
    }
}

/// The bridge's files under one home directory.
public struct BridgeChannel: Equatable, Sendable {
    public let home: URL

    public init(home: URL) { self.home = home }

    /// `$RATCHET_HOME`, else the daemon's support directory (`~/Library/Application Support/Ratchet`).
    public init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        if let path = environment["RATCHET_HOME"], !path.isEmpty {
            self.home = URL(fileURLWithPath: path, isDirectory: true)
        } else {
            self.home = FileManager.default.homeDirectoryForCurrentUser
                .appending(path: "Library/Application Support/Ratchet", directoryHint: .isDirectory)
        }
    }

    public var root: URL { home.appending(path: "bridge", directoryHint: .isDirectory) }
    public var inbox: URL { root.appending(path: "inbox", directoryHint: .isDirectory) }
    public var outbox: URL { root.appending(path: "outbox", directoryHint: .isDirectory) }
    public var snapshots: URL { root.appending(path: "snapshots", directoryHint: .isDirectory) }
    public var statusFile: URL { root.appending(path: "status.json") }
    /// Written when the app listens; holds the process id.
    public var readyFile: URL { root.appending(path: "ready") }

    public static let commandExtension = "cmd"

    /// The id of a command file (`<id>.cmd`), or nil for any other file.
    public static func id(ofCommandFile name: String) -> String? {
        guard name.hasSuffix("." + commandExtension), !name.hasPrefix(".") else { return nil }
        let id = String(name.dropLast(commandExtension.count + 1))
        return id.isEmpty ? nil : id
    }

    public func replyFile(id: String) -> URL { outbox.appending(path: id + ".json") }
}

extension DaemonPhase {
    /// The phase as a plain word, for the bridge's `status`.
    public var bridgeName: String {
        switch self {
        case .checking: "checking"
        case .needsApproval: "needsApproval"
        case .starting: "starting"
        case .running: "running"
        case .stopped: "stopped"
        case .failed: "failed"
        }
    }

    public var failureDetail: String? {
        if case .failed(let why) = self { return why }
        return nil
    }
}
