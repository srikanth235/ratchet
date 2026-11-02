import Foundation

// Live updates: `events()` is the follower's stream (DaemonEventFollower.swift).

extension DaemonRatchetClient {
    public func events() async -> AsyncStream<RatchetEvent> {
        DaemonEventFollower(client: self, window: coalescing).stream()
    }
}
