import Foundation

/// A `ChatClient` with no model behind it, for tests, snapshots and demo mode. It follows the daemon's scripted harness
/// (`docs/ratchet-chat.md`, "Simulation"): events carry ids and are replayed from `lastEventID`; a message with
/// `[simulate:rate-limit]`, `[simulate:auth-expired]`, `[simulate:crash]` or `[simulate:slow]` produces that failure; a message
/// that mentions a "draft" (or any message of a chat on a repo) produces a draft card, "create it" creates it, a message in a
/// chat on an optimization is a steering note, and `[simulate:env-fail]` fails the draft's environment check.
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
        var draft: DraftState?
        var createdCount = 0
    }

    /// What the scripted harness remembers of the draft it is talking about, so "use Grok too" changes the next card.
    private struct DraftState: Sendable {
        var cardID: String
        var agents: [Harness]
        var budget = 50
        var schedule: String? = "every night at 02:00"
        var smaller = false
        var created = false
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

    public func withdrawNote(chatID: String, noteID: String, text: String) async throws {
        guard let stored = chats[chatID] else { throw RatchetClientError.notFound("chat \(chatID)") }
        guard var note = stored.conversation.cards.first(where: { $0.id == noteID && $0.kind == .note }) else {
            throw RatchetClientError.notFound("note \(noteID)")
        }
        guard note.status == .queued else { throw RatchetClientError.invalid("An agent has already read that note.") }
        note.status = .replaced
        record(chatID, .note(note))
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

    public func readNotes(chatID: String, by harness: Harness, at date: Date) {
        guard let stored = chats[chatID] else { return }
        for var note in stored.conversation.cards where note.kind == .note && note.status == .queued {
            note.status = .read
            note.readBy = harness.title
            note.readAt = date
            record(chatID, .note(note))
        }
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
        } else if let optimization = chat.optimization {
            steer(id, chat: chat, optimization: optimization, text: text, turn: turn)
        } else if isCreate(lower), let draft = chats[id]?.draft, !draft.created {
            create(id, chat: chat, draft: draft, once: lower.contains("once"), turn: turn)
        } else if lower.contains("draft") || lower.contains("[simulate:env-fail]") || chat.repo != nil || chats[id]?.draft != nil {
            makeDraft(id, chat: chat, text: text, lower: lower, turn: turn)
        } else {
            say(id, chat: chat, text: "Noted: " + text, turn: turn)
        }
    }

    private func isCreate(_ lower: String) -> Bool {
        let trimmed = lower.trimmingCharacters(in: .whitespacesAndNewlines.union(.punctuationCharacters))
        return trimmed.hasPrefix("create it") || trimmed == "go ahead" || trimmed == "yes"
    }

    private func steps(_ id: String, turn: Int) {
        record(id, .tool(ChatToolStep(id: "t\(turn)", name: "list_templates", state: .running, summary: "Looking at the templates")))
        record(id, .tool(ChatToolStep(id: "t\(turn)", name: "list_templates", state: .done, summary: "Looked at the templates")))
    }

    /// A reply with text, word by word, and the end of the turn.
    private func say(_ id: String, chat: Chat, text answer: String, turn: Int) {
        steps(id, turn: turn)
        finish(id, chat: chat, answer: answer, turn: turn)
    }

    private func finish(_ id: String, chat: Chat, answer: String, turn: Int) {
        for word in answer.split(separator: " ", omittingEmptySubsequences: false) {
            record(id, .delta(String(word) + " "))
        }
        record(id, .message(ChatMessage(
            seq: takeSeq(id), role: .assistant, text: answer, at: now, harness: chat.harness, model: chat.model)))
        let usage = ChatUsage(inputTokens: 120, outputTokens: 40, costUSD: 0, billing: .subscription)
        record(id, .usage(usage))
        record(id, .done(turn: turn, outcome: .completed, usage: usage))
    }

    // MARK: Drafts

    private func makeDraft(_ id: String, chat: Chat, text: String, lower: String, turn: Int) {
        steps(id, turn: turn)
        var draft = chats[id]?.draft ?? DraftState(cardID: "", agents: [chat.harness])
        let previous = draft.cardID.isEmpty ? nil : draft.cardID
        draft.cardID = "draft-\(turn)"
        draft.created = false
        if let budget = Self.dollars(in: lower) { draft.budget = budget }
        if lower.contains("every night") || lower.contains("nightly") { draft.schedule = "every night at 02:00" }
        if lower.contains("weekdays") { draft.schedule = "weekdays at 02:00" }
        if lower.contains("once") && !lower.contains("every") { draft.schedule = nil }
        for other in Harness.allCases where lower.contains("use " + other.title.lowercased()) && ready.contains(other) {
            if !draft.agents.contains(other) { draft.agents.append(other) }
        }
        if lower.contains("smaller") { draft.smaller = true }
        chats[id]?.draft = draft
        var card = Self.card(draft, chat: chat, ready: ready)
        var answer = "Here is a draft. Say \"create it\" to start."
        if lower.contains("[simulate:env-fail]") {
            card.status = .failed
            card.errorTitle = "Couldn't lock the environment"
            card.errorMessage = "A smaller setup (API + Postgres, no load generator host) fits under $1/h but measures less."
            answer = "The environment needs a smaller setup. Pick a fix below, or say what to change."
        }
        record(id, .card(card, replaces: previous))
        finish(id, chat: chat, answer: answer, turn: turn)
    }

    private func create(_ id: String, chat: Chat, draft: DraftState, once: Bool, turn: Int) {
        steps(id, turn: turn)
        var next = draft
        next.created = true
        if once { next.schedule = nil }
        chats[id]?.draft = next
        chats[id]?.createdCount += 1
        var card = Self.card(next, chat: chat, ready: ready)
        card.status = .created
        card.optimization = "opt-created-\(chats[id]?.createdCount ?? 1)"
        record(id, .card(card, replaces: nil))
        finish(id, chat: chat, answer: "Created. The first run starts now.", turn: turn)
    }

    private func steer(_ id: String, chat: Chat, optimization: String, text: String, turn: Int) {
        let seq = takeSeq(id)
        let note = ChatCard(
            id: "note-\(seq)", kind: .note, status: .queued, title: text, optimization: optimization)
        record(id, .note(note))
        finish(id, chat: chat, answer: "Noted. An agent reads it at its next candidate.", turn: turn)
    }

    /// The first whole number after a "$" in the text.
    static func dollars(in text: String) -> Int? {
        guard let dollar = text.firstIndex(of: "$") else { return nil }
        let digits = text[text.index(after: dollar)...].prefix { $0.isNumber }
        return Int(digits)
    }

    private static func card(_ draft: DraftState, chat: Chat, ready: [Harness]) -> ChatCard {
        let agentTitles = draft.agents.map(\.title).joined(separator: " → ")
        let options = ready.map { ChatCardOption(label: $0.title, value: $0.rawValue) }
        let schedule = draft.schedule ?? "run once, now"
        let rows: [ChatCardRow] = [
            ChatCardRow(key: "goal", label: "Goal", value: "Make it faster", editable: true, raw: .string("Make it faster")),
            ChatCardRow(
                key: "measure", label: "Measures", value: "p95 of POST /checkout · lower is better", editable: true,
                raw: .object(["metric": .string("p95"), "direction": .string("minimize")])),
            ChatCardRow(
                key: "agents", label: "Agents", value: agentTitles + " · on this Mac", editable: true, options: options,
                raw: .array(draft.agents.map { .string($0.rawValue) })),
            ChatCardRow(
                key: "limits", label: "Each run", value: "up to $\(draft.budget) and 2 h", editable: true,
                raw: .object(["budgetUsd": .number(Double(draft.budget))])),
            ChatCardRow(
                key: "schedule", label: "Schedule", value: schedule, editable: true,
                raw: draft.schedule.map { .string($0) } ?? .null),
            ChatCardRow(key: "output", label: "Output", value: "a pull request with verified wins"),
            ChatCardRow(
                key: "config", label: "Config", value: "ratchet.yaml · bench/latency.js → branch ratchet/config",
                raw: .object(["branch": .string("ratchet/config")])),
        ]
        return ChatCard(
            id: draft.cardID, title: "Make it faster", subtitle: "up to $\(draft.budget) per run", repo: chat.repo, rows: rows)
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
