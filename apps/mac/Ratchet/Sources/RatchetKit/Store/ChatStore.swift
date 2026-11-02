import Foundation
import Observation

/// The open chats: one `ChatSession` per chat, the actions on them, and the stream that keeps each one current. Its own
/// `@Observable`, apart from `AppStore`; and each chat is its own observable too, so a delta arriving in one chat re-renders the
/// live text of that chat and nothing else (frames are also coalesced: at most one publication per `coalescing` interval).
@MainActor @Observable
public final class ChatStore {
    /// Chat ids, oldest first.
    public private(set) var order: [String] = []
    /// Why the last attempt to start a chat failed ("No agent is signed in..."); nil when it worked.
    public private(set) var startError: String?

    @ObservationIgnored private var sessions: [String: ChatSession] = [:]
    @ObservationIgnored private let client: any ChatClient
    @ObservationIgnored private let pause: @Sendable (Duration) async throws -> Void
    @ObservationIgnored private let coalescing: Duration

    public init(
        client: any ChatClient, coalescing: Duration = .milliseconds(33),
        pause: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    ) {
        self.client = client
        self.coalescing = coalescing
        self.pause = pause
    }

    public func session(_ id: String) -> ChatSession? { sessions[id] }

    /// The conversation of a chat as a value.
    public func conversation(_ id: String) -> ChatConversation? { sessions[id]?.conversation }

    // MARK: Actions

    /// Starts a chat and returns its id, or nil with `startError` set.
    @discardableResult
    public func start(_ new: NewChat = NewChat()) async -> String? {
        do {
            let chat = try await client.startChat(new)
            sessions[chat.id] = ChatSession(ChatConversation(chat))
            order.append(chat.id)
            startError = nil
            return chat.id
        } catch {
            startError = error.localizedDescription
            return nil
        }
    }

    /// Says something. A refusal (the chat is still replying) is kept on the session, not thrown at the view.
    public func send(_ id: String, text: String) async {
        guard let session = sessions[id] else { return }
        session.setActionError(nil)
        do {
            _ = try await client.send(chatID: id, text: text)
        } catch {
            session.setActionError(error.localizedDescription)
        }
    }

    /// "Try again" after a failed turn: the last thing the person said, once more.
    public func retry(_ id: String) async {
        guard let last = sessions[id]?.core.chat.messages.last(where: { $0.role == .user }) else { return }
        await send(id, text: last.text)
    }

    /// "Use Codex" after a limit: the same conversation starts again on another harness and the last thing said is sent to it.
    /// Returns the new chat's id.
    @discardableResult
    public func switchHarness(_ id: String, to harness: Harness) async -> String? {
        guard let old = sessions[id] else { return nil }
        let last = old.core.chat.messages.last { $0.role == .user }?.text
        guard let fresh = await start(NewChat(repo: old.repo, optimization: old.optimization, harness: harness)) else { return nil }
        if let last { await send(fresh, text: last) }
        return fresh
    }

    public func interrupt(_ id: String) async {
        do {
            try await client.interrupt(chatID: id)
        } catch {
            sessions[id]?.setActionError(error.localizedDescription)
        }
    }

    /// Takes a steering note back, which works only while no agent has read it.
    public func withdrawNote(_ id: String, noteID: String) async {
        guard let session = sessions[id], let note = session.cards.first(where: { $0.id == noteID }), note.status == .queued else { return }
        do {
            try await client.withdrawNote(chatID: id, noteID: noteID, text: note.title)
        } catch {
            session.setActionError(error.localizedDescription)
        }
    }

    /// Ends the chat and forgets it.
    public func end(_ id: String) async {
        do {
            try await client.end(chatID: id)
            sessions[id] = nil
            order.removeAll { $0 == id }
        } catch {
            sessions[id]?.setActionError(error.localizedDescription)
        }
    }

    /// Fetches the chat again and replaces what is shown (after a resync).
    public func reload(_ id: String) async {
        guard let session = sessions[id] else { return }
        do {
            let fresh = try await client.chat(id: id, after: nil, limit: 500)
            session.replace(with: fresh)
        } catch {
            session.setActionError(error.localizedDescription)
        }
    }

    // MARK: The stream

    /// Keeps `id` current until the calling task is cancelled (give it to a view's `.task(id:)`): connects, applies the frames
    /// in batches, reloads when the daemon says what was missed is gone, and reconnects from the last event it saw after a
    /// growing pause (the daemon client reconnects by itself; this covers a stream that ends).
    public func follow(_ id: String) async {
        var pace = Duration.milliseconds(500)
        while !Task.isCancelled, let session = sessions[id] {
            // Always resume from what the conversation holds, even from nothing: events sent before this connected are replayed.
            let position: Int64? = session.core.chat.lastEventID
            let frames = await client.stream(chatID: id, lastEventID: position)
            let batch = FrameBatcher(session: session, interval: coalescing)
            do {
                for try await frame in frames {
                    batch.push(frame)
                    pace = .milliseconds(500)
                    if session.core.needsReload {
                        batch.flush()
                        await reload(id)
                    }
                }
                batch.flush()
            } catch is CancellationError {
                batch.flush()
                return
            } catch {
                batch.flush()
                // A chat that is gone ends the loop below; anything else is tried again.
                if sessions[id] == nil { return }
            }
            if Task.isCancelled { return }
            do { try await pause(pace) } catch { return }
            pace = min(pace * 2, .seconds(10))
        }
    }
}

/// Collects the frames of a stream and applies them together: text frames at most once per interval (with a trailing flush so
/// the last words are never held back), everything else at once.
@MainActor
final class FrameBatcher {
    private let session: ChatSession
    private let interval: Duration
    private let clock = ContinuousClock()
    private var buffer: [ChatFrame] = []
    private var lastFlush: ContinuousClock.Instant
    private var trailing: Task<Void, Never>?

    init(session: ChatSession, interval: Duration) {
        self.session = session
        self.interval = interval
        self.lastFlush = clock.now.advanced(by: .seconds(-10))
    }

    func push(_ frame: ChatFrame) {
        buffer.append(frame)
        switch frame.event {
        case .delta, .thinking:
            if clock.now - lastFlush >= interval {
                flush()
            } else if trailing == nil {
                let wait = interval
                trailing = Task { [weak self] in
                    try? await Task.sleep(for: wait)
                    self?.flush()
                }
            }
        default:
            flush()
        }
    }

    func flush() {
        trailing?.cancel()
        trailing = nil
        guard !buffer.isEmpty else { return }
        let frames = buffer
        buffer = []
        lastFlush = clock.now
        session.apply(frames)
    }
}
