import Foundation

/// Everything a conversation view shows, as a value: the messages, the reply being written, the steps of this turn, the
/// cards, and the last error. `apply` is pure and total, so the stream rules are tested without a view. Per-conversation
/// state lives here (one value per chat), not in a shared `lastError` string.
public struct ChatConversation: Sendable, Equatable {
    public var chat: Chat
    /// The reply being written: replaces itself with the finished message.
    public var reply = ""
    public var thinking = ""
    /// The steps of the turn in progress, in the order they started.
    public var steps: [ChatToolStep] = []
    /// Draft and note cards by id, in the order they first appeared.
    public var cards: [ChatCard] = []
    /// The last turn failed like this; cleared when the person says something again.
    public var error: ChatError?
    public var lastUsage: ChatUsage?
    public var lastOutcome: ChatOutcome?
    /// The stream could not replay what was missed: fetch the chat again (`ChatClient.chat`) and `replace(with:)`.
    public var needsReload = false
    /// A request of the person's failed before the chat could answer (sending while it replies, a chat that is gone).
    public var actionError: String?

    public init(_ chat: Chat) {
        self.chat = chat
        self.reply = chat.pending ?? ""
    }

    public var isReplying: Bool { chat.state == .replying }

    /// The newest message number applied.
    public var lastSeq: Int64 { chat.messages.last?.seq ?? 0 }

    /// The chat fetched again after a resync.
    public mutating func replace(with fresh: Chat) {
        chat = fresh
        reply = fresh.pending ?? ""
        needsReload = false
    }

    /// Applies an event with its place in the stream: the resume position moves to its id.
    public mutating func apply(_ frame: ChatFrame) {
        apply(frame.event)
        if let id = frame.id { chat.lastEventID = max(chat.lastEventID, id) }
    }

    public mutating func apply(_ event: ChatEvent) {
        switch event {
        case .ready(let ready):
            chat.state = ready.state
            chat.lastEventID = max(chat.lastEventID, ready.lastEventID)
            if ready.resync { needsReload = true }
            if let pending = ready.pending { reply = pending }
        case .message(let message):
            if message.role == .user {
                error = nil
                actionError = nil
                lastOutcome = nil
            }
            if message.role == .assistant {
                reply = ""
                thinking = ""
            }
            if let at = chat.messages.firstIndex(where: { $0.seq == message.seq }) {
                chat.messages[at] = message
            } else {
                chat.messages.append(message)
                chat.messages.sort { $0.seq < $1.seq }
            }
        case .delta(let text):
            chat.state = .replying
            reply += text
        case .thinking(let text):
            thinking += text
        case .tool(let step):
            if let at = steps.firstIndex(where: { $0.id == step.id }) {
                steps[at] = step
            } else {
                steps.append(step)
            }
        case .card(let card, let replaces):
            if let replaces, let at = cards.firstIndex(where: { $0.id == replaces }) {
                cards[at].status = .replaced
            }
            upsert(card)
        case .note(let card):
            upsert(card)
        case .usage(let usage):
            lastUsage = usage
        case .done(_, let outcome, let usage):
            chat.state = .idle
            lastOutcome = outcome
            lastUsage = usage
            reply = ""
            thinking = ""
            steps = []
        case .failed(let failure):
            error = failure
        }
    }

    private mutating func upsert(_ card: ChatCard) {
        if let at = cards.firstIndex(where: { $0.id == card.id }) {
            cards[at] = card
        } else {
            cards.append(card)
        }
    }
}
