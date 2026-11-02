import Foundation
import Observation

/// What the composer's one button does.
public enum ComposerButton: Equatable, Sendable {
    case send
    /// While the chat replies the send button becomes Stop.
    case stop
}

/// What a key press in a composer does.
public enum ComposerKey {
    public enum Action: Equatable, Sendable { case send, newline }

    /// Return and Command-Return send in a single-line composer (Home, the creation conversation); in the steering note composer
    /// ("Return adds a line · ⌘↩ sends") only Command-Return sends. Shift-Return always adds a line.
    public static func action(command: Bool, shift: Bool, multiline: Bool) -> Action {
        if command { return .send }
        if shift { return .newline }
        return multiline ? .newline : .send
    }

    /// The hint under the composer.
    public static func hint(multiline: Bool) -> String {
        multiline ? "Return adds a line · ⌘↩ sends" : "⌘↩ sends"
    }
}

/// One conversation: the timeline, the live reply, the composer's button, Retry and Copy under a reply, interrupt and the typed
/// error with its fixes. The views draw it and call its methods; every rule is here.
@MainActor @Observable
public final class ConversationModel {
    public let chatID: String
    /// The words being typed.
    public var draft = ""
    /// What the models of each harness are called ("Opus"), for the "Written by" line.
    public var catalog: HarnessCatalog
    /// "Sign In" opens Customize, Agents on this harness; set by the app.
    @ObservationIgnored public var onSignIn: (Harness) -> Void = { _ in }
    /// "Use Codex" started another chat; set by the app to show it.
    @ObservationIgnored public var onSwitched: (String) -> Void = { _ in }

    @ObservationIgnored private let store: ChatStore
    @ObservationIgnored private let now: @Sendable () -> Date

    public init(store: ChatStore, chatID: String, catalog: HarnessCatalog = .empty, now: @escaping @Sendable () -> Date = { Date() }) {
        self.store = store
        self.chatID = chatID
        self.catalog = catalog
        self.now = now
    }

    private var session: ChatSession? { store.session(chatID) }

    // MARK: Reading

    public var items: [ChatItem] {
        guard let session else { return [] }
        session.observeStructure()
        return ChatTimeline.items(session.conversation)
    }
    public var isReplying: Bool { session?.isReplying ?? false }

    /// The live reply: a separate observable, so the text changing does not redraw the list.
    public var live: LiveReply? { session?.reply }

    /// "Claude Code · Opus": the chat's harness and model, for the header and the first reply.
    public var harnessLabel: String {
        guard let session else { return "" }
        let model = ReplyLabel.modelTitle(session.model, harness: session.harness, catalog: catalog)
        return session.harness.title + (model.map { " · " + $0 } ?? "")
    }

    /// The first-use notice, until it is dismissed: "First time: your messages, the repo name and the files … go to Claude Code".
    public var dataNotice: String? { session?.dataNotice }

    public func dismissDataNotice() { session?.dismissDataNotice() }

    public func label(for message: ChatMessage) -> ReplyLabel? { ReplyLabel(message, catalog: catalog) }

    public var error: ChatErrorPresentation? {
        session?.error.map { ChatErrorPresentation($0, now: now()) }
    }

    /// A request of the person's that failed before the chat could answer.
    public var actionError: String? { session?.actionError }

    public var button: ComposerButton { isReplying ? .stop : .send }

    public var canSend: Bool {
        !isReplying && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && draft.count <= 8000
    }

    /// Retry sits under the newest reply, and only while nothing is being written.
    public func canRetry(_ message: ChatMessage) -> Bool {
        guard !isReplying, message.role == .assistant, let session else { return false }
        return session.messages.last { $0.role == .assistant }?.seq == message.seq
    }

    /// What Copy puts on the pasteboard.
    public func copyText(_ message: ChatMessage) -> String { message.text }

    // MARK: Acting

    /// Sends what is typed and clears it; the draft stays when the send is refused.
    public func send() async {
        guard canSend else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        draft = ""
        await store.send(chatID, text: text)
        if session?.actionError != nil { draft = text }
    }

    /// The composer's button: send, or stop while replying.
    public func pressButton() async {
        switch button {
        case .send: await send()
        case .stop: await stop()
        }
    }

    public func stop() async { await store.interrupt(chatID) }

    public func retry() async { await store.retry(chatID) }

    public func perform(_ action: ChatErrorAction) async {
        switch action {
        case .tryAgain:
            await store.retry(chatID)
        case .signIn(let harness):
            onSignIn(harness)
        case .switchTo(let harness):
            if let fresh = await store.switchHarness(chatID, to: harness) { onSwitched(fresh) }
        }
    }
}
