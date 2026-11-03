import Foundation
import Testing
@testable import RatchetKit

@Suite("Chat client and conversation")
struct ChatTests {
    private let now = Date(timeIntervalSince1970: 1_793_452_800)

    private func make(ready: [Harness] = Harness.allCases) -> MockChatClient {
        MockChatClient(now: now, readyHarnesses: ready)
    }

    private func read(
        _ stream: AsyncThrowingStream<ChatFrame, any Error>, until stop: (ChatFrame) -> Bool
    ) async throws -> [ChatFrame] {
        var events: [ChatFrame] = []
        for try await event in stream {
            events.append(event)
            if stop(event) { break }
        }
        return events
    }

    private func isDone(_ frame: ChatFrame) -> Bool {
        if case .done = frame.event { return true }
        return false
    }

    // MARK: Starting

    @Test func aChatStartsOnTheFirstReadyHarness() async throws {
        let chat = try await make(ready: [.codex, .grok]).startChat(NewChat())
        #expect(chat.harness == .codex)
        #expect(chat.state == .idle && chat.messages.isEmpty)
    }

    @Test func noReadyHarnessIsUnavailable() async throws {
        let client = make(ready: [])
        await #expect(throws: RatchetClientError.self) { try await client.startChat(NewChat()) }
        let other = make(ready: [.grok])
        await #expect(throws: RatchetClientError.self) { try await other.startChat(NewChat(harness: .codex)) }
    }

    @Test func theFirstChatWithAHarnessSaysWhatLeavesTheMac() async throws {
        let client = make()
        let first = try await client.startChat(NewChat(harness: .claudeCode))
        let second = try await client.startChat(NewChat(harness: .claudeCode))
        #expect(first.dataNotice?.contains("Anthropic") == true)
        #expect(second.dataNotice == nil)
    }

    @Test func aRepoChatStartsWithItsCheckoutStep() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat(repo: "acme/shop"))
        let stream = await client.stream(chatID: chat.id, lastEventID: 0)
        let events = try await read(stream, until: { if case .tool = $0.event { return true } else { return false } })
        #expect(events.first?.event == ChatEvent.ready(ChatReady(state: .idle, lastSeq: 0, lastEventID: 1)))
        if case .tool(let step)? = events.last?.event { #expect(step.summary == "Reading acme/shop") } else { Issue.record("no checkout step") }
    }

    // MARK: A turn

    @Test func aTurnStreamsDeltasThenTheFinishedMessageThenDone() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        let stream = await client.stream(chatID: chat.id, lastEventID: nil)
        let receipt = try await client.send(chatID: chat.id, text: "hello there")
        #expect(receipt.turn == 1)
        let events = try await read(stream, until: isDone)

        guard case .ready(let ready)? = events.first?.event else { Issue.record("not ready first"); return }
        #expect(ready.state == .idle && !ready.resync)
        #expect(events.contains { if case .delta = $0.event { return true } else { return false } })
        var conversation = ChatConversation(chat)
        for event in events { conversation.apply(event) }
        #expect(conversation.chat.messages.map(\.role) == [.user, .assistant])
        #expect(conversation.chat.messages.last?.text == "Noted: hello there")
        #expect(conversation.chat.messages.last?.harness == chat.harness)
        #expect(conversation.reply.isEmpty && conversation.steps.isEmpty)
        #expect(!conversation.isReplying && conversation.lastOutcome == .completed)
        #expect(conversation.lastUsage?.billing == .subscription)
    }

    @Test func theStoredChatMatchesWhatTheStreamBuilt() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        _ = try await client.send(chatID: chat.id, text: "one")
        _ = try await client.send(chatID: chat.id, text: "two")
        let stored = try await client.chat(id: chat.id, after: nil, limit: 50)
        #expect(stored.messages.map(\.text) == ["one", "Noted: one", "two", "Noted: two"])
        #expect(stored.messages.map(\.seq) == [1, 2, 3, 4])
        let tail = try await client.chat(id: chat.id, after: 2, limit: 50)
        #expect(tail.messages.map(\.seq) == [3, 4])
        let page = try await client.chat(id: chat.id, after: nil, limit: 2)
        #expect(page.messages.count == 2 && page.hasMore)
    }

    @Test func aMessageNeedsTextAndAnIdleChat() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        await #expect(throws: RatchetClientError.self) { try await client.send(chatID: chat.id, text: "   ") }
        _ = try await client.send(chatID: chat.id, text: "[simulate:slow]")
        await #expect(throws: RatchetClientError.self) { try await client.send(chatID: chat.id, text: "again") }
        await #expect(throws: RatchetClientError.self) { try await client.send(chatID: "nope", text: "hi") }
    }

    @Test func interruptingKeepsWhatWasWritten() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        let stream = await client.stream(chatID: chat.id, lastEventID: nil)
        _ = try await client.send(chatID: chat.id, text: "[simulate:slow]")
        try await client.interrupt(chatID: chat.id)
        let events = try await read(stream, until: isDone)
        var conversation = ChatConversation(chat)
        for event in events { conversation.apply(event) }
        #expect(conversation.lastOutcome == .interrupted)
        #expect(conversation.chat.messages.last?.text == "Working on it")
        #expect(!conversation.isReplying)
        // Idle again: the person can say something.
        _ = try await client.send(chatID: chat.id, text: "go on")
    }

    @Test func aDraftReplacesTheOlderDraft() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat(repo: "acme/shop"))
        let stream = await client.stream(chatID: chat.id, lastEventID: nil)
        _ = try await client.send(chatID: chat.id, text: "draft it")
        _ = try await client.send(chatID: chat.id, text: "draft it again")
        var conversation = ChatConversation(chat)
        var dones = 0
        for try await event in stream {
            conversation.apply(event)
            if isDone(event) { dones += 1 }
            if dones == 2 { break }
        }
        #expect(conversation.cards.map(\.id) == ["draft-1", "draft-2"])
        #expect(conversation.cards.map(\.status) == [.replaced, .draft])
    }

    // MARK: Failures are typed

    @Test func aRateLimitSaysWhenItResets() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat(harness: .claudeCode))
        let stream = await client.stream(chatID: chat.id, lastEventID: nil)
        _ = try await client.send(chatID: chat.id, text: "[simulate:rate-limit]")
        let events = try await read(stream, until: isDone)
        var conversation = ChatConversation(chat)
        for event in events { conversation.apply(event) }
        let error = try #require(conversation.error)
        #expect(error.code == .rateLimited && error.isRateLimited && !error.isAuthExpired)
        #expect(error.harness == .claudeCode && error.switchTo == .codex)
        #expect(error.fixes == [.retry, .switchHarness])
        #expect(!error.canRetry(at: now))
        #expect(error.canRetry(at: now.addingTimeInterval(3601)))
        #expect(error.resetNote(now: now)?.hasPrefix("Resets at ") == true)
        #expect(error.resetNote(now: now.addingTimeInterval(3601)) == nil)
        #expect(conversation.lastOutcome == .failed)
    }

    @Test func anExpiredLoginOffersSignInNotRetry() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat(harness: .codex))
        let stream = await client.stream(chatID: chat.id, lastEventID: nil)
        _ = try await client.send(chatID: chat.id, text: "[simulate:auth-expired]")
        let events = try await read(stream, until: isDone)
        var conversation = ChatConversation(chat)
        for event in events { conversation.apply(event) }
        let error = try #require(conversation.error)
        #expect(error.isAuthExpired && error.fixes == [.signIn])
        #expect(!error.canRetry(at: now))
    }

    @Test func theErrorClearsWhenThePersonSaysSomethingAgain() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        let stream = await client.stream(chatID: chat.id, lastEventID: nil)
        _ = try await client.send(chatID: chat.id, text: "[simulate:crash]")
        _ = try await client.send(chatID: chat.id, text: "try again")
        var conversation = ChatConversation(chat)
        var dones = 0
        for try await event in stream {
            conversation.apply(event)
            if isDone(event) { dones += 1 }
            if dones == 2 { break }
        }
        #expect(conversation.error == nil)
        #expect(conversation.lastOutcome == .completed)
    }

    // MARK: Resuming

    @Test func aStreamContinuesFromLastEventID() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        _ = try await client.send(chatID: chat.id, text: "one")
        let seen = try await client.chat(id: chat.id, after: nil, limit: 50).lastEventID
        _ = try await client.send(chatID: chat.id, text: "two")
        let stream = await client.stream(chatID: chat.id, lastEventID: seen)
        let events = try await read(stream, until: isDone)
        guard case .ready(let ready)? = events.first?.event else { Issue.record("not ready first"); return }
        #expect(!ready.resync)
        // Only the second turn is replayed: the user's echo comes first.
        if case .message(let message) = events[1].event { #expect(message.text == "two") } else { Issue.record("replay began with \(events[1].event)") }
    }

    @Test func aStreamTooFarBehindMustResync() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        for turn in 0..<40 { _ = try await client.send(chatID: chat.id, text: "message \(turn)") }
        let stream = await client.stream(chatID: chat.id, lastEventID: 1)
        let events = try await read(stream, until: { _ in true })
        guard case .ready(let ready)? = events.first?.event else { Issue.record("not ready first"); return }
        #expect(ready.resync)
        var conversation = ChatConversation(chat)
        conversation.apply(.ready(ready))
        #expect(conversation.needsReload)
        conversation.replace(with: try await client.chat(id: chat.id, after: nil, limit: 500))
        #expect(!conversation.needsReload && conversation.chat.messages.count == 80)
    }

    @Test func endingAChatForgetsIt() async throws {
        let client = make()
        let chat = try await client.startChat(NewChat())
        try await client.end(chatID: chat.id)
        await #expect(throws: RatchetClientError.self) { try await client.chat(id: chat.id, after: nil, limit: 10) }
        await #expect(throws: RatchetClientError.self) { try await client.end(chatID: chat.id) }
    }

    // MARK: Scripted replies

    @Test func aQueuedReplyAnswersTheNextTurnInsteadOfTheDefaultScript() async throws {
        let mock = make()
        let scenario: any ChatScenario = mock
        let card = ChatCard(id: "d1", title: "Checkout p95", rows: [ChatCardRow(key: "goal", label: "Goal", value: "p95")])
        await scenario.queue(ChatReplyScript(text: "Here is a draft.", card: card))
        let chat = try await mock.startChat(NewChat())
        let stream = await mock.stream(chatID: chat.id, lastEventID: nil)
        _ = try await mock.send(chatID: chat.id, text: "anything")
        let events = try await read(stream, until: isDone)
        var conversation = ChatConversation(chat)
        for event in events { conversation.apply(event) }
        #expect(conversation.cards.map(\.id) == ["d1"])
        #expect(conversation.chat.messages.last?.text == "Here is a draft.")
        // The queue is used up: the default script is back.
        _ = try await mock.send(chatID: chat.id, text: "again")
        let after = try await mock.chat(id: chat.id, after: nil, limit: 50)
        #expect(after.messages.last?.text == "Noted: again")
    }

    @Test func aQueuedErrorFailsTheTurnWithThatError() async throws {
        let mock = make()
        let error = ChatError(code: .unavailable, harness: .grok, title: "Grok is offline", message: "", fixes: [.retry])
        await mock.queue(ChatReplyScript(error: error))
        let chat = try await mock.startChat(NewChat(harness: .grok))
        let stream = await mock.stream(chatID: chat.id, lastEventID: nil)
        _ = try await mock.send(chatID: chat.id, text: "hi")
        let events = try await read(stream, until: isDone)
        var conversation = ChatConversation(chat)
        for event in events { conversation.apply(event) }
        #expect(conversation.error == error)
        #expect(conversation.lastOutcome == .failed)
    }
}
