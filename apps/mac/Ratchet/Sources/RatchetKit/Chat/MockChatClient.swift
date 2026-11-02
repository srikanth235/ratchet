import Foundation

/// A `ChatClient` with no model behind it, for tests, snapshots and demo mode. It follows the daemon's scripted harness
/// (`docs/ratchet-chat.md`, "Simulation"): events carry ids and are replayed from `lastEventID`; a message with
/// `[simulate:rate-limit]`, `[simulate:auth-expired]`, `[simulate:crash]` or `[simulate:slow]` produces that failure; a message
/// that mentions a "draft" produces a draft card.
public actor MockChatClient: ChatClient, ChatScenario {
    private struct Entry: Sendable {
        var id: Int64
        var event: ChatEvent
    }

    private struct Stored: Sendable {
        var conversation: ChatConversation
        var turn = 0
        var nextSeq: Int64 = 1
        var nextEvent: Int64 = 1
        var log: [Entry] = []
    }

    private var chats: [String: Stored] = [:]
    private var subscribers: [String: [UUID: AsyncThrowingStream<ChatFrame, any Error>.Continuation]] = [:]
    private var counter = 0
    private var queued: [ChatReplyScript] = []
    private var noticedHarnesses: Set<Harness> = []
    private let now: Date
    private let ready: [Harness]
    /// How many events a chat remembers for `lastEventID`, as the daemon does.
    static let retained = 256

    public init(now: Date = Date(timeIntervalSince1970: 1_793_452_800), readyHarnesses: [Harness] = Harness.allCases) {
        self.now = now
        self.ready = readyHarnesses
    }

    // MARK: ChatClient

    public func startChat(_ new: NewChat) async throws -> Chat {
        let harness = new.harness ?? HarnessCatalog.order.first { ready.contains($0) }
        guard let harness, ready.contains(harness) else {
            throw RatchetClientError.unavailable("No agent is signed in. Sign in to Claude Code, Codex or Grok first.")
        }
        counter += 1
        let id = "chat-\(counter)"
        var chat = Chat(id: id, repo: new.repo, optimization: new.optimization, harness: harness, model: new.model ?? "default", updatedAt: now)
        if noticedHarnesses.insert(harness).inserted {
            chat.dataNotice = "Your messages, the repo name and the files \(harness.title) reads go to \(harness.vendor). Ratchet sends nothing else."
        }
        chats[id] = Stored(conversation: ChatConversation(chat))
        if new.repo != nil {
            record(id, .tool(ChatToolStep(id: "checkout", name: "checkout", state: .done, summary: "Reading " + (new.repo ?? ""))))
        }
        return view(of: id) ?? chat
    }

    public func chat(id: String, after seq: Int64?, limit: Int) async throws -> Chat {
        guard var chat = view(of: id) else { throw RatchetClientError.notFound("chat \(id)") }
        if let seq { chat.messages = chat.messages.filter { $0.seq > seq } }
        if limit > 0, chat.messages.count > limit {
            chat.messages = Array(chat.messages.prefix(limit))
            chat.hasMore = true
        }
        return chat
    }

    public func send(chatID: String, text: String) async throws -> ChatReceipt {
        guard let stored = chats[chatID] else { throw RatchetClientError.notFound("chat \(chatID)") }
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty, body.count <= 8000 else { throw RatchetClientError.invalid("A message is 1 to 8000 characters.") }
        guard !stored.conversation.isReplying else { throw RatchetClientError.invalid("The chat is still replying.") }

        let chat = stored.conversation.chat
        chats[chatID]?.turn += 1
        let turn = chats[chatID]?.turn ?? 1
        let userSeq = takeSeq(chatID)
        record(chatID, .message(ChatMessage(seq: userSeq, role: .user, text: body, at: now)))
        script(chatID, chat: chat, text: body, turn: turn)
        return ChatReceipt(seq: userSeq, turn: turn)
    }

    public func stream(chatID: String, lastEventID: Int64?) async -> AsyncThrowingStream<ChatFrame, any Error> {
        let (stream, continuation) = AsyncThrowingStream<ChatFrame, any Error>.makeStream()
        guard let stored = chats[chatID] else {
            continuation.finish(throwing: RatchetClientError.notFound("chat \(chatID)"))
            return stream
        }
        let newest = stored.nextEvent - 1
        let oldest = stored.log.first?.id ?? stored.nextEvent
        let gone = lastEventID.map { $0 < newest && oldest > $0 + 1 } ?? false
        let replay = (lastEventID != nil && !gone) ? stored.log.filter { $0.id > (lastEventID ?? 0) } : []
        let pending = (lastEventID == nil && !stored.conversation.reply.isEmpty) ? stored.conversation.reply : nil
        continuation.yield(ChatFrame(event: .ready(ChatReady(
            state: stored.conversation.chat.state, lastSeq: stored.conversation.lastSeq, lastEventID: newest, resync: gone,
            pending: pending))))
        for entry in replay { continuation.yield(ChatFrame(id: entry.id, event: entry.event)) }

        let token = UUID()
        subscribers[chatID, default: [:]][token] = continuation
        continuation.onTermination = { @Sendable [weak self] _ in
            Task { await self?.drop(chatID, token) }
        }
        return stream
    }

    public func interrupt(chatID: String) async throws {
        guard let stored = chats[chatID] else { throw RatchetClientError.notFound("chat \(chatID)") }
        guard stored.conversation.isReplying else { return }
        let chat = stored.conversation.chat
        let written = stored.conversation.reply
        if !written.isEmpty {
            record(chatID, .message(ChatMessage(
                seq: takeSeq(chatID), role: .assistant, text: written, at: now, harness: chat.harness, model: chat.model)))
        }
        record(chatID, .done(turn: stored.turn, outcome: .interrupted, usage: ChatUsage()))
    }

    public func end(chatID: String) async throws {
        guard chats.removeValue(forKey: chatID) != nil else { throw RatchetClientError.notFound("chat \(chatID)") }
        if let open = subscribers.removeValue(forKey: chatID) {
            for continuation in open.values { continuation.finish() }
        }
    }

    // MARK: Script

    public func queue(_ reply: ChatReplyScript) {
        queued.append(reply)
    }

    private func play(_ script: ChatReplyScript, _ id: String, chat: Chat, turn: Int) {
        if let error = script.error {
            record(id, .failed(error))
            record(id, .done(turn: turn, outcome: .failed, usage: ChatUsage()))
            return
        }
        if let card = script.card { record(id, .card(card, replaces: nil)) }
        for word in script.text.split(separator: " ", omittingEmptySubsequences: false) {
            record(id, .delta(String(word) + " "))
        }
        record(id, .message(ChatMessage(
            seq: takeSeq(id), role: .assistant, text: script.text, at: now, harness: chat.harness, model: chat.model)))
        record(id, .done(turn: turn, outcome: .completed, usage: ChatUsage()))
    }

    private func script(_ id: String, chat: Chat, text: String, turn: Int) {
        if !queued.isEmpty {
            play(queued.removeFirst(), id, chat: chat, turn: turn)
            return
        }
        let lower = text.lowercased()
        let harness = chat.harness
        if lower.contains("[simulate:rate-limit]") {
            let other = HarnessCatalog.order.first { $0 != harness && ready.contains($0) }
            record(id, .failed(ChatError(
                code: .rateLimited, harness: harness, title: harness.title + " hit its usage limit", message: "It resets in an hour.",
                fixes: other == nil ? [.retry] : [.retry, .switchHarness],
                retry: ChatRetry(canRetryNow: false, resetsAt: now.addingTimeInterval(3600)), switchTo: other)))
            record(id, .done(turn: turn, outcome: .failed, usage: ChatUsage()))
        } else if lower.contains("[simulate:auth-expired]") {
            record(id, .failed(ChatError(
                code: .authExpired, harness: harness, title: harness.title + " needs you to sign in again",
                message: "Its login has expired.", fixes: [.signIn])))
            record(id, .done(turn: turn, outcome: .failed, usage: ChatUsage()))
        } else if lower.contains("[simulate:crash]") {
            record(id, .failed(ChatError(
                code: .failed, harness: harness, title: harness.title + " stopped unexpectedly", message: "", fixes: [.retry])))
            record(id, .done(turn: turn, outcome: .failed, usage: ChatUsage()))
        } else if lower.contains("[simulate:slow]") {
            record(id, .delta("Working on it"))
        } else {
            reply(id, chat: chat, text: text, turn: turn)
        }
    }

    private func reply(_ id: String, chat: Chat, text: String, turn: Int) {
        record(id, .tool(ChatToolStep(id: "t\(turn)", name: "list_templates", state: .running, summary: "Looking at the templates")))
        record(id, .tool(ChatToolStep(id: "t\(turn)", name: "list_templates", state: .done, summary: "Looked at the templates")))
        var answer = "Noted: " + text
        if text.lowercased().contains("draft") {
            let card = ChatCard(
                id: "draft-\(turn)", title: "Make it faster", subtitle: chat.repo ?? "No repo", repo: chat.repo,
                rows: [
                    ChatCardRow(key: "goal", label: "Goal", value: "Make it faster"),
                    ChatCardRow(key: "limits", label: "Each run", value: "$50 · 2 h", editable: true),
                ])
            let replaces = turn > 1 ? "draft-\(turn - 1)" : nil
            record(chat.id, .card(card, replaces: replaces))
            answer = "Here is a draft. Say \"create it\" to start."
        }
        for word in answer.split(separator: " ", omittingEmptySubsequences: false) {
            record(id, .delta(String(word) + " "))
        }
        record(id, .message(ChatMessage(
            seq: takeSeq(id), role: .assistant, text: answer, at: now, harness: chat.harness, model: chat.model)))
        let usage = ChatUsage(inputTokens: 120, outputTokens: 40, costUSD: 0, billing: .subscription)
        record(id, .usage(usage))
        record(id, .done(turn: turn, outcome: .completed, usage: usage))
    }

    // MARK: Bookkeeping

    private func takeSeq(_ id: String) -> Int64 {
        let seq = chats[id]?.nextSeq ?? 1
        chats[id]?.nextSeq = seq + 1
        return seq
    }

    /// Applies an event to the stored chat, keeps it for replay and sends it to the open streams.
    private func record(_ id: String, _ event: ChatEvent) {
        guard var stored = chats[id] else { return }
        let entry = Entry(id: stored.nextEvent, event: event)
        stored.nextEvent += 1
        stored.conversation.apply(event)
        stored.conversation.chat.lastEventID = entry.id
        stored.log.append(entry)
        if stored.log.count > Self.retained { stored.log.removeFirst(stored.log.count - Self.retained) }
        chats[id] = stored
        if let open = subscribers[id] {
            for continuation in open.values { continuation.yield(ChatFrame(id: entry.id, event: event)) }
        }
    }

    private func view(of id: String) -> Chat? {
        guard let stored = chats[id] else { return nil }
        var chat = stored.conversation.chat
        chat.pending = stored.conversation.reply.isEmpty ? nil : stored.conversation.reply
        return chat
    }

    private func drop(_ chatID: String, _ token: UUID) {
        subscribers[chatID]?[token] = nil
    }
}
