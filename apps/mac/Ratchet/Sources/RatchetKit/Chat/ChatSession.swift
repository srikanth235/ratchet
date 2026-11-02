import Foundation
import Observation

/// The reply being written, apart from everything else: token-rate changes land here and invalidate only the view that draws
/// the live text, never the message list, the cards or the composer.
@MainActor @Observable
public final class LiveReply {
    public internal(set) var text = ""
    public internal(set) var thinking = ""

    public init() {}
}

/// One open chat as the views observe it. The truth is a `ChatConversation` (a pure value); what views read are separate
/// properties that change only when their own part changed, so a delta does not re-render the card and a new card does not
/// re-render the live text. Frames are applied in batches (`apply(_:)`): one publication per batch.
@MainActor @Observable
public final class ChatSession: Identifiable {
    public let id: String
    public let reply = LiveReply()

    public private(set) var messages: [ChatMessage]
    public private(set) var steps: [ChatToolStep] = []
    public private(set) var cards: [ChatCard] = []
    public private(set) var error: ChatError?
    public private(set) var isReplying: Bool
    public private(set) var lastOutcome: ChatOutcome?
    public private(set) var lastUsage: ChatUsage?
    public internal(set) var actionError: String?
    /// The first-use notice (what leaves this Mac) until the person dismisses it.
    public private(set) var dataNotice: String?
    public private(set) var harness: Harness
    public private(set) var model: String?
    public let repo: String?
    public let optimization: String?

    /// The pure state behind the published parts.
    @ObservationIgnored public private(set) var core: ChatConversation

    public init(_ conversation: ChatConversation) {
        let chat = conversation.chat
        id = chat.id
        core = conversation
        messages = chat.messages
        isReplying = conversation.isReplying
        dataNotice = chat.dataNotice
        harness = chat.harness
        model = chat.model
        repo = chat.repo
        optimization = chat.optimization
        reply.text = conversation.reply
        publish(structural: true)
    }

    /// The conversation as a value (for tests and for code that wants everything at once).
    public var conversation: ChatConversation { core }

    public func dismissDataNotice() {
        dataNotice = nil
        core.chat.dataNotice = nil
    }

    // MARK: Applying

    /// Applies frames in order and publishes once. Only text frames (`delta`, `thinking`) leave the structure alone.
    public func apply(_ frames: [ChatFrame]) {
        guard !frames.isEmpty else { return }
        var structural = false
        for frame in frames {
            core.apply(frame)
            switch frame.event {
            case .delta, .thinking: break
            default: structural = true
            }
        }
        publish(structural: structural)
    }

    public func replace(with fresh: Chat) {
        core.replace(with: fresh)
        publish(structural: true)
    }

    public func setActionError(_ message: String?) {
        if actionError != message { actionError = message }
        core.actionError = message
    }

    private func publish(structural: Bool) {
        if reply.text != core.reply { reply.text = core.reply }
        if reply.thinking != core.thinking { reply.thinking = core.thinking }
        guard structural else { return }
        if messages != core.chat.messages { messages = core.chat.messages }
        if steps != core.steps { steps = core.steps }
        if cards != core.cards { cards = core.cards }
        if error != core.error { error = core.error }
        if isReplying != core.isReplying { isReplying = core.isReplying }
        if lastOutcome != core.lastOutcome { lastOutcome = core.lastOutcome }
        if lastUsage != core.lastUsage { lastUsage = core.lastUsage }
        if harness != core.chat.harness { harness = core.chat.harness }
        if model != core.chat.model { model = core.chat.model }
    }
}
