import Foundation
import Observation

/// The open chats: one `ChatConversation` per chat, the actions on them, and the stream that keeps each one current. Its own
/// `@Observable`, apart from `AppStore`: a delta arriving in a chat re-renders that chat's view and nothing else. No view uses
/// it yet (the Home composer and the conversation are P5).
@MainActor @Observable
public final class ChatStore {
    public private(set) var conversations: [String: ChatConversation] = [:]
    /// Chat ids, oldest first.
    public private(set) var order: [String] = []
    /// Why the last attempt to start a chat failed ("No agent is signed in..."); nil when it worked.
    public private(set) var startError: String?

    private let client: any ChatClient
    private let pause: @Sendable (Duration) async throws -> Void

    public init(client: any ChatClient, pause: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }) {
        self.client = client
        self.pause = pause
    }

    public func conversation(_ id: String) -> ChatConversation? { conversations[id] }

    // MARK: Actions

    /// Starts a chat and returns its id, or nil with `startError` set.
    @discardableResult
    public func start(_ new: NewChat = NewChat()) async -> String? {
        do {
            let chat = try await client.startChat(new)
            conversations[chat.id] = ChatConversation(chat)
            order.append(chat.id)
            startError = nil
            return chat.id
        } catch {
            startError = error.localizedDescription
            return nil
        }
    }

    /// Says something. A refusal (the chat is still replying) is kept on the conversation, not thrown at the view.
    public func send(_ id: String, text: String) async {
        guard conversations[id] != nil else { return }
        conversations[id]?.actionError = nil
        do {
            _ = try await client.send(chatID: id, text: text)
        } catch {
            conversations[id]?.actionError = error.localizedDescription
        }
    }

    /// "Try again" after a failed turn: the last thing the person said, once more.
    public func retry(_ id: String) async {
        guard let last = conversations[id]?.chat.messages.last(where: { $0.role == .user }) else { return }
        await send(id, text: last.text)
    }

    public func interrupt(_ id: String) async {
        do {
            try await client.interrupt(chatID: id)
        } catch {
            conversations[id]?.actionError = error.localizedDescription
        }
    }

    /// Ends the chat and forgets it.
    public func end(_ id: String) async {
        do {
            try await client.end(chatID: id)
            conversations[id] = nil
            order.removeAll { $0 == id }
        } catch {
            conversations[id]?.actionError = error.localizedDescription
        }
    }

    /// Fetches the chat again and replaces what is shown (after a resync).
    public func reload(_ id: String) async {
        guard conversations[id] != nil else { return }
        do {
            let fresh = try await client.chat(id: id, after: nil, limit: 500)
            conversations[id]?.replace(with: fresh)
        } catch {
            conversations[id]?.actionError = error.localizedDescription
        }
    }

    // MARK: The stream

    /// Keeps `id` current until the calling task is cancelled (give it to a view's `.task(id:)`): connects, applies every frame,
    /// reloads when the daemon says what was missed is gone, and reconnects from the last event it saw after a growing pause.
    public func follow(_ id: String) async {
        var pace = Duration.milliseconds(500)
        while !Task.isCancelled, let known = conversations[id] {
            let position: Int64? = known.chat.lastEventID > 0 ? known.chat.lastEventID : nil
            let frames = await client.stream(chatID: id, lastEventID: position)
            do {
                for try await frame in frames {
                    conversations[id]?.apply(frame)
                    pace = .milliseconds(500)
                    if conversations[id]?.needsReload == true { await reload(id) }
                }
            } catch is CancellationError {
                return
            } catch {
                // A chat that is gone ends the loop below; anything else is tried again.
                if conversations[id] == nil { return }
            }
            if Task.isCancelled { return }
            do { try await pause(pace) } catch { return }
            pace = min(pace * 2, .seconds(10))
        }
    }
}
