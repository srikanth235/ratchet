import Foundation
import HTTPTypes

/// A new build can find an old `ratchetd` still running (the LaunchAgent outlives the app). The app compares the version
/// the running daemon reports with the one inside the bundle, and restarts the agent when they differ.
public enum DaemonVersionCheck: Equatable, Sendable {
    case matches
    case mismatch(running: String, bundled: String)
    /// One of the two could not be read: nothing is restarted on a guess.
    case unknown

    public static func compare(running: String?, bundled: String?) -> DaemonVersionCheck {
        guard let running = clean(running), let bundled = clean(bundled) else { return .unknown }
        return running == bundled ? .matches : .mismatch(running: running, bundled: bundled)
    }

    public var needsRestart: Bool {
        if case .mismatch = self { return true }
        return false
    }

    /// The version in `ratchetd --version` output ("ratchetd 0.1.0"): the last word.
    public static func parse(versionOutput: String) -> String? {
        let line = versionOutput.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
        return clean(line.split(separator: " ").last.map(String.init))
    }

    private static func clean(_ version: String?) -> String? {
        let trimmed = version?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return trimmed.isEmpty ? nil : trimmed
    }
}

extension DaemonRatchetClient {
    /// The version `GET /v1/status` reports (the crate version of the running `ratchetd`).
    public func daemonVersion() async throws -> String {
        let data = try await jsonCall(.get, "/v1/status", operationID: "getStatus")
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let version = object["version"] as? String
        else { throw RatchetClientError.unavailable("ratchetd answered with something the app cannot read.") }
        return version
    }
}
