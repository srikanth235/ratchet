import Foundation
import OpenAPIRuntime

/// What an `SSEConnection` tells its handler.
public enum SSEItem: Sendable, Equatable {
    /// A connection opened. `isReconnect` is false for the first one; `resumedFrom` is the `Last-Event-ID` it was opened with.
    case connected(isReconnect: Bool, resumedFrom: String?)
    /// The messages that became complete in one read, in order.
    case messages([SSEMessage])
    /// The connection ended or could not be opened (`error` is nil for a clean end). It is tried again after a pause unless the
    /// task was cancelled or `Policy.maxFailures` is reached. `attempt` counts failures since the last good connection.
    case disconnected(error: String?, attempt: Int)
}

/// A server-sent event connection that stays up: it opens the stream, parses it, remembers the `id:` of the last message,
/// reconnects with `Last-Event-ID` after a growing pause, and stops when its task is cancelled. It knows nothing about what the
/// messages mean: the events stream of `ratchetd` and, later, each chat's stream are both handled by passing a different
/// `open` and a different handler.
public struct SSEConnection: Sendable {
    public struct Policy: Sendable {
        public var initialPause: Duration
        public var maxPause: Duration
        /// Give up after this many failures in a row; nil retries for ever.
        public var maxFailures: Int?
        /// Waits between attempts; replaced in tests so they need not wait.
        public var sleep: @Sendable (Duration) async throws -> Void

        public init(
            initialPause: Duration = .milliseconds(500), maxPause: Duration = .seconds(10), maxFailures: Int? = nil,
            sleep: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
        ) {
            self.initialPause = initialPause
            self.maxPause = maxPause
            self.maxFailures = maxFailures
            self.sleep = sleep
        }
    }

    /// Opens one connection: asks for the stream with `Last-Event-ID` when there is a position to resume from.
    public typealias Open = @Sendable (_ lastEventID: String?) async throws -> HTTPBody

    private let policy: Policy
    private let open: Open

    public init(policy: Policy = Policy(), open: @escaping Open) {
        self.policy = policy
        self.open = open
    }

    /// Runs until the task is cancelled (or `maxFailures` is reached), calling `handle` for everything that happens. `handle`
    /// gets the resume position, which already includes the `id:` of every message delivered, and may change it: set it to nil
    /// when the server says what was missed is gone (the next connection then starts from now).
    public func run(
        from lastEventID: String? = nil,
        handle: @Sendable (SSEItem, inout String?) async -> Void
    ) async {
        var position = lastEventID
        var pause = policy.initialPause
        var failures = 0
        var connectedBefore = false
        while !Task.isCancelled {
            var failure: String?
            do {
                let body = try await open(position)
                await handle(.connected(isReconnect: connectedBefore, resumedFrom: position), &position)
                connectedBefore = true
                failures = 0
                pause = policy.initialPause
                var parser = SSEParser(lastEventID: position)
                for try await chunk in body {
                    let messages = parser.feed(Array(chunk))
                    if messages.isEmpty { continue }
                    if let id = messages.last(where: { $0.id != nil })?.id { position = id }
                    await handle(.messages(messages), &position)
                }
            } catch is CancellationError {
                break
            } catch {
                failure = error.localizedDescription
            }
            if Task.isCancelled { break }
            if failure != nil { failures += 1 }
            await handle(.disconnected(error: failure, attempt: failures), &position)
            if let limit = policy.maxFailures, failures >= limit { break }
            do { try await policy.sleep(pause) } catch { break }
            pause = min(pause * 2, policy.maxPause)
        }
    }
}
