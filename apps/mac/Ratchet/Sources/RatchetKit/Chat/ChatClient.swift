import Foundation

/// Conversations with the user's own harness (Claude Code, Codex or Grok), through `ratchetd`. Kept apart from
/// `RatchetClient`: chats have their own stream per conversation and their own lifecycle. This is the protocol and its mock
/// (`MockChatClient`) only; the daemon implementation and the UI come with P5.
public protocol ChatClient: Sendable {
    /// Starts a chat. Throws `RatchetClientError.unavailable` when no harness is signed in.
    func startChat(_ new: NewChat) async throws -> Chat
    /// The chat with its messages, oldest first, and the reply being written. `after` pages forward from a message number.
    func chat(id: String, after seq: Int64?, limit: Int) async throws -> Chat
    /// Says something. Returns at once; the reply arrives on the stream. Throws `RatchetClientError.invalid` while replying.
    func send(chatID: String, text: String) async throws -> ChatReceipt
    /// The chat's live stream. The first event is `.ready`. `lastEventID` continues a stream that dropped: events after it are
    /// replayed, or `.ready` says `resync` when they are gone. A failed turn arrives as `.failed` followed by `.done`.
    func stream(chatID: String, lastEventID: Int64?) async -> AsyncThrowingStream<ChatFrame, any Error>
    /// Stops the reply being written. What was written stays.
    func interrupt(chatID: String) async throws
    /// Ends the chat and forgets its history.
    func end(chatID: String) async throws
}

/// One scripted reply for a test, a snapshot or the self-test: what the next turn of a chat says, optionally with a card, or fails
/// like this. Reached through `ChatScenario`, so nothing downcasts a `ChatClient` to its mock.
public struct ChatReplyScript: Sendable, Equatable {
    public var text: String
    public var card: ChatCard?
    public var error: ChatError?

    public init(text: String = "", card: ChatCard? = nil, error: ChatError? = nil) {
        self.text = text
        self.card = card
        self.error = error
    }
}

/// How a test, a snapshot or the self-test makes a sample chat client say something particular.
public protocol ChatScenario: Sendable {
    /// The next turn (of any chat) answers like this instead of its default script. Scripts play in the order they were queued.
    func queue(_ reply: ChatReplyScript) async
}
