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
    /// The user message (by `seq`) each card answers: a card is shown under the reply of that turn.
    public private(set) var cardAnchors: [String: Int64] = [:]
    /// The steps of finished turns by the user message they belong to; `steps` holds the turn in progress until it is over.
    public private(set) var turnSteps: [Int64: [ChatToolStep]] = [:]
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
        thinking = ""
        steps = []
        needsReload = false
        // The stored history carries its cards and steps as messages: rebuild what the stream would have built.
        cards = []
        cardAnchors = [:]
        turnSteps = [:]
        var anchor: Int64 = 0
        for message in fresh.messages {
            switch message.role {
            case .user:
                anchor = message.seq
            case .tool:
                if let step = message.tool { turnSteps[anchor, default: []].append(step) }
            case .card, .note:
                if let card = message.card { place(card, anchor: anchor) }
            case .assistant:
                break
            }
        }
    }

    /// The seq of the newest user message: the turn in progress belongs to it.
    public var currentAnchor: Int64 { chat.messages.last { $0.role == .user }?.seq ?? 0 }

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
                steps = []
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
            place(card, anchor: currentAnchor)
        case .note(let card):
            place(card, anchor: currentAnchor)
        case .usage(let usage):
            lastUsage = usage
        case .done(_, let outcome, let usage):
            chat.state = .idle
            lastOutcome = outcome
            lastUsage = usage
            reply = ""
            thinking = ""
            if !steps.isEmpty { turnSteps[currentAnchor] = steps }
            steps = []
        case .failed(let failure):
            error = failure
        }
    }

    /// A card in its place: new cards take the turn they arrived in, an update keeps where its card already is.
    private mutating func place(_ card: ChatCard, anchor: Int64) {
        if let at = cards.firstIndex(where: { $0.id == card.id }) {
            cards[at] = card
        } else {
            cards.append(card)
            cardAnchors[card.id] = anchor
        }
    }
}
