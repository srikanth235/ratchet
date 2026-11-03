import Foundation

// What a conversation view draws, as values: the timeline in order, the "Written by" line, and the buttons of a failed turn.
// Pure functions of a `ChatConversation`, so the words and the order are tested without a view.

/// One row of a conversation, top to bottom.
public enum ChatItem: Identifiable, Equatable, Sendable {
    case user(ChatMessage)
    /// A step of the harness ("Read acme/shop · main").
    case step(ChatToolStep)
    case reply(ChatMessage)
    case card(ChatCard)

    public var id: String {
        switch self {
        case .user(let message): "user-\(message.seq)"
        case .step(let step): "step-\(step.id)"
        case .reply(let message): "reply-\(message.seq)"
        case .card(let card): "card-\(card.id)"
        }
    }
}

public enum ChatTimeline {
    /// The messages in order, with the steps of each turn after the person's words and the cards after the reply they came with.
    /// A card of a turn that ended with no reply (a failed turn) still shows, before the next thing the person says.
    public static func items(_ conversation: ChatConversation) -> [ChatItem] {
        var items: [ChatItem] = []
        var cardsByAnchor: [Int64: [ChatCard]] = [:]
        for card in conversation.cards where card.kind == .draft {
            cardsByAnchor[conversation.cardAnchors[card.id] ?? 0, default: []].append(card)
        }
        var anchor: Int64 = 0
        var emitted = Set<Int64>()

        func flushCards(_ key: Int64) {
            guard !emitted.contains(key) else { return }
            emitted.insert(key)
            for card in cardsByAnchor[key] ?? [] { items.append(.card(card)) }
        }
        func steps(_ key: Int64) {
            var list = conversation.turnSteps[key] ?? []
            if key == conversation.currentAnchor { list += conversation.steps }
            for step in list { items.append(.step(step)) }
        }

        for message in conversation.chat.messages {
            switch message.role {
            case .user:
                flushCards(anchor)
                anchor = message.seq
                items.append(.user(message))
                steps(anchor)
            case .assistant:
                items.append(.reply(message))
                flushCards(anchor)
            case .tool, .card, .note:
                break
            }
        }
        if conversation.chat.messages.isEmpty { steps(0) }
        flushCards(anchor)
        // Cards that arrived before any user message (a chat opened on a draft) lead the list.
        if anchor != 0, let early = cardsByAnchor[0], !early.isEmpty, !emitted.contains(0) {
            items.insert(contentsOf: early.map { ChatItem.card($0) }, at: 0)
        }
        return items
    }
}

/// "Written by Claude Code · Opus", the line under a reply.
public struct ReplyLabel: Equatable, Sendable {
    public var harness: Harness
    public var model: String?

    public init(harness: Harness, model: String?) {
        self.harness = harness
        self.model = model
    }

    /// nil for a message nobody wrote with a harness (the person's own).
    public init?(_ message: ChatMessage, catalog: HarnessCatalog = .empty) {
        guard message.role == .assistant, let harness = message.harness else { return nil }
        self.init(harness: harness, model: Self.modelTitle(message.model, harness: harness, catalog: catalog))
    }

    /// The model's name in the catalog ("Opus"), else its id. The CLI's own default ("default", or none) is the catalog's default
    /// model of that harness, so the line always says which model wrote; nothing only when the catalog does not know it either.
    static func modelTitle(_ id: String?, harness: Harness, catalog: HarnessCatalog) -> String? {
        guard let id, !id.isEmpty, id != "default" else { return catalog.choice(for: harness).model?.title }
        return catalog.models(of: harness).first { $0.id == id }?.title ?? id
    }

    public var text: String {
        "Written by " + harness.title + (model.map { " · " + $0 } ?? "")
    }
}

/// A button under a failed turn.
public enum ChatErrorAction: Equatable, Sendable, Identifiable {
    case tryAgain(enabled: Bool)
    case signIn(Harness)
    case switchTo(Harness)

    public var id: String {
        switch self {
        case .tryAgain: "try-again"
        case .signIn(let harness): "sign-in-" + harness.rawValue
        case .switchTo(let harness): "switch-" + harness.rawValue
        }
    }

    public var title: String {
        switch self {
        case .tryAgain: "Try Again"
        case .signIn: "Sign In"
        case .switchTo(let harness): "Use " + harness.title
        }
    }

    public var isEnabled: Bool {
        if case .tryAgain(let enabled) = self { return enabled }
        return true
    }
}

/// A failed turn as the view shows it: what happened, when it can be tried again, and the buttons ("Try Again" is there but
/// disabled while a limit has not reset).
public struct ChatErrorPresentation: Equatable, Sendable {
    public var title: String
    public var message: String
    /// "Resets at 15:40" while a rate limit holds.
    public var note: String?
    public var actions: [ChatErrorAction]

    public init(_ error: ChatError, now: Date, timeZone: TimeZone = .current) {
        title = error.title
        message = error.message
        if error.isRateLimited, let resetsAt = error.retry?.resetsAt, resetsAt > now {
            note = "Resets at " + Elapsed.timeOfDay(resetsAt, timeZone: timeZone)
        } else {
            note = nil
        }
        var buttons: [ChatErrorAction] = []
        for fix in error.fixes {
            switch fix {
            case .retry: buttons.append(.tryAgain(enabled: error.canRetry(at: now)))
            case .signIn: buttons.append(.signIn(error.harness))
            case .switchHarness:
                if let other = error.switchTo { buttons.append(.switchTo(other)) }
            }
        }
        actions = buttons
    }
}
