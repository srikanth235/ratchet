import Foundation

/// Where `ratchetd` listens and where it leaves the token of this boot.
public struct DaemonLocation: Sendable, Equatable {
    public var socketPath: String
    public var tokenPath: String

    public init(socketPath: String, tokenPath: String) {
        self.socketPath = socketPath
        self.tokenPath = tokenPath
    }

    /// `~/Library/Application Support/Ratchet/ratchetd.sock` and `ratchetd.token` next to it, as the daemon writes them.
    /// `RATCHET_HOME` moves the directory and `RATCHET_SOCKET` names the socket, as for the daemon itself.
    public static func standard(environment: [String: String] = ProcessInfo.processInfo.environment) -> DaemonLocation {
        let directory: URL
        if let home = environment["RATCHET_HOME"], !home.isEmpty {
            directory = URL(fileURLWithPath: home, isDirectory: true)
        } else {
            directory = FileManager.default.homeDirectoryForCurrentUser
                .appending(path: "Library/Application Support/Ratchet", directoryHint: .isDirectory)
        }
        if let socket = environment["RATCHET_SOCKET"], !socket.isEmpty {
            let parent = URL(fileURLWithPath: socket).deletingLastPathComponent()
            return DaemonLocation(socketPath: socket, tokenPath: parent.appending(path: "ratchetd.token").path)
        }
        return DaemonLocation(
            socketPath: directory.appending(path: "ratchetd.sock").path,
            tokenPath: directory.appending(path: "ratchetd.token").path)
    }

    /// True when a daemon accepts connections at the socket right now.
    public var isReachable: Bool { UnixSocket.canConnect(path: socketPath) }

    /// The token of this boot, read from its file each time (the daemon writes a new one at every start).
    public func readToken() -> String? {
        guard let text = try? String(contentsOfFile: tokenPath, encoding: .utf8) else { return nil }
        let token = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return token.isEmpty ? nil : token
    }
}
