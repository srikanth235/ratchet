import Foundation
import HTTPTypes
import Testing
@testable import RatchetKit

/// One contract for `ChatClient`, run against the mock and against `DaemonChatClient` over a daemon that speaks the wire
/// (`ChatDaemonFake`): the daemon client must behave like the mock the app is built against.
@Suite("Chat client contract")
struct ChatClientContractTests {
    private let now = Date(timeIntervalSince1970: 1_793_452_800)

    struct Subject: Sendable {
        var name: String
        var client: any ChatClient
        var scenario: any ChatScenario
        var fake: ChatDaemonFake?
    }

    private func subjects(ready: [Harness] = Harness.allCases) -> [Subject] {
        let plain = MockChatClient(now: now, readyHarnesses: ready)
        let behind = MockChatClient(now: now, readyHarnesses: ready)
        let fake = ChatDaemonFake(mock: behind)
        let daemon = DaemonChatClient(
            transport: fake, policy: SSEConnection.Policy(initialPause: .milliseconds(1), maxPause: .milliseconds(5), maxFailures: 3, sleep: { _ in }))
        return [
            Subject(name: "mock", client: plain, scenario: plain, fake: nil),
            Subject(name: "daemon", client: daemon, scenario: behind, fake: fake),
        ]
    }

    private func read(_ stream: AsyncThrowingStream<ChatFrame, any Error>, until stop: (ChatFrame) -> Bool) async throws -> [ChatFrame] {
        var frames: [ChatFrame] = []
        for try await frame in stream {
            frames.append(frame)
            if stop(frame) { break }
        }
        return frames
    }

    private func isDone(_ frame: ChatFrame) -> Bool {
        if case .done = frame.event { return true }
        return false
    }

    private func conversation(_ chat: Chat, _ frames: [ChatFrame]) -> ChatConversation {
        var value = ChatConversation(chat)
        for frame in frames { value.apply(frame) }
        return value
    }

    // MARK: Create, send, stream, history, delete

    @Test(.timeLimit(.minutes(1))) func aChatStartsWithItsHarnessAndASingleFirstUseNotice() async throws {
        for subject in subjects(ready: [.codex, .grok]) {
            let first = try await subject.client.startChat(NewChat(repo: "acme/shop"))
            #expect(first.harness == .codex && first.repo == "acme/shop" && first.state == .idle, "\(subject.name) harness")
            #expect(first.dataNotice?.contains("OpenAI") == true, "\(subject.name) first-use notice")
            let second = try await subject.client.startChat(NewChat(harness: .codex))
            #expect(second.dataNotice == nil, "\(subject.name) notice only once")
        }
    }

    @Test(.timeLimit(.minutes(1))) func noReadyAgentIsUnavailable() async throws {
        for subject in subjects(ready: []) {
            let client = subject.client
            await #expect(throws: RatchetClientError.self, "\(subject.name)") { try await client.startChat(NewChat()) }
            do {
                _ = try await client.startChat(NewChat())
                Issue.record("\(subject.name) started a chat with no agent")
            } catch let error as RatchetClientError {
                if case .unavailable = error {} else { Issue.record("\(subject.name) threw \(error)") }
            }
        }
    }

    @Test(.timeLimit(.minutes(1))) func aNormalDraftThenCreate() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat(repo: "acme/shop"))
            let stream = await subject.client.stream(chatID: chat.id, lastEventID: chat.lastEventID)
            _ = try await subject.client.send(chatID: chat.id, text: "Make checkout faster. Run it every night, $20 a run.")
            var frames = try await read(stream, until: isDone)
            guard case .ready? = frames.first?.event else { Issue.record("\(subject.name) not ready first"); continue }
            #expect(frames.contains { if case .tool = $0.event { true } else { false } }, "\(subject.name) steps")
            var state = conversation(chat, frames)
            let draft = try #require(state.cards.first, "\(subject.name) draft")
            #expect(draft.status == .draft && draft.kind == .draft, "\(subject.name) draft status")
            #expect(draft.rows.map(\.key) == ["goal", "measure", "agents", "limits", "schedule", "output", "config"], "\(subject.name) rows")
            let limits = try #require(draft.rows.first { $0.key == "limits" })
            #expect(limits.raw["budgetUsd"]?.number == 20 && limits.editable, "\(subject.name) raw limits")
            let agents = try #require(draft.rows.first { $0.key == "agents" })
            #expect(agents.options.map(\.value) == ["claude-code", "codex", "grok"], "\(subject.name) agent options")
            #expect(state.chat.messages.last?.harness == chat.harness, "\(subject.name) reply names its harness")

            _ = try await subject.client.send(chatID: chat.id, text: "create it")
            let more = try await read(stream, until: isDone)
            frames += more
            for frame in more { state.apply(frame) }
            #expect(state.cards.first?.status == .created, "\(subject.name) created")
            #expect(state.cards.first?.optimization != nil, "\(subject.name) names the optimization")
            #expect(state.cards.count == 1, "\(subject.name) the same card updates in place")
        }
    }

    @Test(.timeLimit(.minutes(1))) func aChangeDraftsAgainAndTheOldDraftIsReplaced() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat(repo: "acme/shop"))
            let stream = await subject.client.stream(chatID: chat.id, lastEventID: chat.lastEventID)
            _ = try await subject.client.send(chatID: chat.id, text: "Make checkout faster")
            var frames = try await read(stream, until: isDone)
            _ = try await subject.client.send(chatID: chat.id, text: "use Grok too and weekdays")
            frames += try await read(stream, until: isDone)
            let state = conversation(chat, frames)
            #expect(state.cards.count == 2 && state.cards[0].status == .replaced && state.cards[1].status == .draft, "\(subject.name)")
            let agents = try #require(state.cards.last?.rows.first { $0.key == "agents" })
            #expect(agents.raw.strings.contains("grok"), "\(subject.name) the agents row has Grok")
            #expect(state.cards.last?.rows.first { $0.key == "schedule" }?.value.contains("weekdays") == true, "\(subject.name) schedule")
        }
    }

    // MARK: Typed failures

    @Test(.timeLimit(.minutes(1))) func rateLimitedAuthExpiredAndCrashAreTyped() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat(harness: .claudeCode))
            let stream = await subject.client.stream(chatID: chat.id, lastEventID: chat.lastEventID)
            var state = ChatConversation(chat)

            _ = try await subject.client.send(chatID: chat.id, text: "[simulate:rate-limit]")
            for frame in try await read(stream, until: isDone) { state.apply(frame) }
            let limited = try #require(state.error, "\(subject.name) rate limit")
            #expect(limited.code == .rateLimited && limited.fixes == [.retry, .switchHarness] && limited.switchTo == .codex, "\(subject.name)")
            #expect(limited.retry?.resetsAt == now.addingTimeInterval(3600) && limited.retry?.canRetryNow == false, "\(subject.name) reset time")
            #expect(ChatErrorPresentation(limited, now: now).actions.map(\.title) == ["Try Again", "Use Codex"], "\(subject.name) buttons")

            _ = try await subject.client.send(chatID: chat.id, text: "[simulate:auth-expired]")
            for frame in try await read(stream, until: isDone) { state.apply(frame) }
            let expired = try #require(state.error, "\(subject.name) auth")
            #expect(expired.code == .authExpired && expired.fixes == [.signIn], "\(subject.name)")

            _ = try await subject.client.send(chatID: chat.id, text: "[simulate:crash]")
            for frame in try await read(stream, until: isDone) { state.apply(frame) }
            let crashed = try #require(state.error, "\(subject.name) crash")
            #expect(crashed.code == .failed && crashed.fixes == [.retry] && state.lastOutcome == .failed, "\(subject.name)")
        }
    }

    // MARK: Slow, interrupt, refusals

    @Test(.timeLimit(.minutes(1))) func aSlowReplyCanBeInterruptedAndKeepsWhatWasWritten() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat())
            let stream = await subject.client.stream(chatID: chat.id, lastEventID: chat.lastEventID)
            _ = try await subject.client.send(chatID: chat.id, text: "[simulate:slow]")
            var state = ChatConversation(chat)
            var seenDelta = false
            for try await frame in stream {
                state.apply(frame)
                if case .delta = frame.event { seenDelta = true; break }
            }
            #expect(seenDelta && state.isReplying, "\(subject.name) replying")
            await #expect(throws: RatchetClientError.self, "\(subject.name) a second message while replying") {
                try await subject.client.send(chatID: chat.id, text: "again")
            }
            try await subject.client.interrupt(chatID: chat.id)
            for try await frame in stream {
                state.apply(frame)
                if isDone(frame) { break }
            }
            #expect(state.lastOutcome == .interrupted && !state.isReplying, "\(subject.name) interrupted")
            #expect(state.chat.messages.last?.text == "Working on it", "\(subject.name) kept what was written")
            try await subject.client.interrupt(chatID: chat.id)  // idle: a no-op
        }
    }

    // MARK: Resume, history, delete

    @Test(.timeLimit(.minutes(1))) func aStreamResumesFromLastEventID() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat())
            _ = try await subject.client.send(chatID: chat.id, text: "one")
            let seen = try await subject.client.chat(id: chat.id, after: nil, limit: 50).lastEventID
            #expect(seen > 0, "\(subject.name) event id")
            _ = try await subject.client.send(chatID: chat.id, text: "two")
            let frames = try await read(await subject.client.stream(chatID: chat.id, lastEventID: seen), until: isDone)
            guard case .ready(let ready)? = frames.first?.event else { Issue.record("\(subject.name) not ready first"); continue }
            #expect(!ready.resync, "\(subject.name) no resync")
            if case .message(let message) = frames[1].event { #expect(message.text == "two", "\(subject.name) replay") } else { Issue.record("\(subject.name) replay began with \(frames[1].event)") }
            if let fake = subject.fake, let name = HTTPField.Name("Last-Event-ID") {
                let header = fake.requests.last { $0.path.hasSuffix("/stream") }?.headers[name]
                #expect(header == String(seen), "daemon sends Last-Event-ID")
            }
        }
    }

    @Test(.timeLimit(.minutes(1))) func aStreamTooFarBehindAsksForAResync() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat())
            for turn in 0..<40 { _ = try await subject.client.send(chatID: chat.id, text: "message \(turn)") }
            let frames = try await read(await subject.client.stream(chatID: chat.id, lastEventID: 1), until: { _ in true })
            guard case .ready(let ready)? = frames.first?.event else { Issue.record("\(subject.name) not ready first"); continue }
            #expect(ready.resync, "\(subject.name) resync")
            var state = ChatConversation(chat)
            state.apply(.ready(ready))
            state.replace(with: try await subject.client.chat(id: chat.id, after: nil, limit: 500))
            #expect(!state.needsReload && state.chat.messages.count == 80, "\(subject.name) history after a resync")
        }
    }

    @Test(.timeLimit(.minutes(1))) func historyPagesForward() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat())
            for turn in 0..<3 { _ = try await subject.client.send(chatID: chat.id, text: "m\(turn)") }
            let page = try await subject.client.chat(id: chat.id, after: nil, limit: 2)
            #expect(page.messages.count == 2 && page.hasMore, "\(subject.name) first page")
            let rest = try await subject.client.chat(id: chat.id, after: page.messages.last?.seq, limit: 50)
            #expect(rest.messages.count == 4 && !rest.hasMore, "\(subject.name) the rest")
            #expect(rest.messages.first.map { $0.seq > (page.messages.last?.seq ?? 0) } == true, "\(subject.name) after")
        }
    }

    @Test(.timeLimit(.minutes(1))) func endingAChatForgetsItAndAGoneChatIsNotFound() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat())
            try await subject.client.end(chatID: chat.id)
            let client = subject.client
            await #expect(throws: RatchetClientError.self, "\(subject.name) history") { try await client.chat(id: chat.id, after: nil, limit: 10) }
            await #expect(throws: RatchetClientError.self, "\(subject.name) end again") { try await client.end(chatID: chat.id) }
            await #expect(throws: RatchetClientError.self, "\(subject.name) send") { try await client.send(chatID: chat.id, text: "hi") }
            // Following a gone chat ends the stream with the error; it does not retry for ever.
            do {
                for try await _ in await client.stream(chatID: chat.id, lastEventID: chat.lastEventID) {}
                Issue.record("\(subject.name) streamed a gone chat")
            } catch is RatchetClientError {
            } catch {
                Issue.record("\(subject.name) threw \(error)")
            }
        }
    }

    @Test(.timeLimit(.minutes(1))) func aSteeringChatQueuesANoteThenSeesItRead() async throws {
        for subject in subjects() {
            let chat = try await subject.client.startChat(NewChat(optimization: "opt-1"))
            let stream = await subject.client.stream(chatID: chat.id, lastEventID: chat.lastEventID)
            _ = try await subject.client.send(chatID: chat.id, text: "focus on database queries")
            var state = ChatConversation(chat)
            for frame in try await read(stream, until: isDone) { state.apply(frame) }
            let note = try #require(state.cards.first, "\(subject.name) note")
            #expect(note.kind == .note && note.status == .queued && note.title == "focus on database queries" && note.optimization == "opt-1", "\(subject.name)")
            await subject.scenario.readNotes(chatID: chat.id, by: .claudeCode, at: now)
            for try await frame in stream {
                state.apply(frame)
                if case .note = frame.event { break }
            }
            #expect(state.cards.first?.status == .read && state.cards.first?.readBy == "Claude Code", "\(subject.name) read receipt")
            #expect(state.cards.first?.readAt == now, "\(subject.name) read time")
        }
    }
}
