import Foundation
import Observation
import Testing
@testable import RatchetKit

/// A flag a `withObservationTracking` change handler can set (the handler is `@Sendable`).
private final class Flag: @unchecked Sendable {  // test-only: written once from the handler, read after the change
    private let lock = NSLock()
    private var set = false
    var value: Bool { lock.withLock { set } }
    func raise() { lock.withLock { set = true } }
}

private let utc = TimeZone(identifier: "UTC") ?? .current
private let epoch = Date(timeIntervalSince1970: 1_793_452_800)

private func readyCatalog(_ harnesses: [Harness] = Harness.allCases, models: [Harness: [HarnessModel]] = [:]) -> HarnessCatalog {
    HarnessCatalog(harnesses.map {
        DetectedHarness(harness: $0, installed: true, version: "1.0", auth: .signedIn, models: models[$0] ?? [], billing: .subscription)
    })
}

@MainActor
private struct Rig {
    let store: ChatStore
    let mock: MockChatClient

    init(ready: [Harness] = Harness.allCases, coalescing: Duration = .zero) {
        mock = MockChatClient(now: epoch, readyHarnesses: ready)
        store = ChatStore(client: mock, coalescing: coalescing, pause: { _ in })
    }

    /// Waits (up to four seconds) for something the stream will deliver.
    func settle(_ condition: @MainActor () -> Bool) async {
        for _ in 0..<400 {
            if condition() { return }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    func follow(_ id: String) -> Task<Void, Never> {
        let store = self.store
        return Task { await store.follow(id) }
    }
}

// MARK: Sessions and frames

@MainActor
@Suite("Chat session")
struct ChatSessionTests {
    private func session() -> ChatSession {
        ChatSession(ChatConversation(Chat(id: "c1", harness: .claudeCode)))
    }

    @Test func aDeltaDoesNotInvalidateTheMessagesTheCardsOrTheComposer() {
        let session = session()
        let flag = Flag()
        withObservationTracking {
            _ = session.messages
            _ = session.cards
            _ = session.steps
            _ = session.error
            _ = session.isReplying
        } onChange: { flag.raise() }
        session.apply([ChatFrame(id: 1, event: .delta("Hel")), ChatFrame(id: 2, event: .delta("lo"))])
        #expect(!flag.value)
        #expect(session.reply.text == "Hello")
    }

    @Test func aCardInvalidatesTheCardsAndNotTheLiveText() {
        let session = session()
        let flag = Flag()
        withObservationTracking { _ = session.reply.text } onChange: { flag.raise() }
        session.apply([ChatFrame(id: 1, event: .card(ChatCard(id: "d1", title: "Checkout"), replaces: nil))])
        #expect(!flag.value)
        #expect(session.cards.map(\.id) == ["d1"])
    }

    @Test func theLiveTextInvalidatesOnlyItsOwnReader() {
        let session = session()
        let live = Flag()
        withObservationTracking { _ = session.reply.text } onChange: { live.raise() }
        session.apply([ChatFrame(id: 1, event: .delta("a"))])
        #expect(live.value)
    }

    @Test func aBatchPublishesOnceAndAnEqualValueDoesNotRepublish() {
        let session = session()
        session.apply([ChatFrame(id: 1, event: .tool(ChatToolStep(id: "t", name: "x", state: .running, summary: "x")))])
        let flag = Flag()
        withObservationTracking { _ = session.steps } onChange: { flag.raise() }
        session.apply([ChatFrame(id: 2, event: .tool(ChatToolStep(id: "t", name: "x", state: .running, summary: "x")))])
        #expect(!flag.value)
    }

    @Test func textFramesAreHeldUntilTheIntervalPassesAndTheLastWordsAreNeverLost() async {
        let session = session()
        let batch = FrameBatcher(session: session, interval: .seconds(3600))
        for part in ["a", "b", "c", "d"] { batch.push(ChatFrame(event: .delta(part))) }
        #expect(session.reply.text == "a")  // the first goes at once; the rest wait for the interval
        batch.flush()
        #expect(session.reply.text == "abcd")

        let held = session
        let trailing = FrameBatcher(session: held, interval: .milliseconds(20))
        trailing.push(ChatFrame(event: .delta("e")))
        trailing.push(ChatFrame(event: .delta("f")))
        for _ in 0..<200 where held.reply.text != "abcdef" { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(held.reply.text == "abcdef")
    }

    @Test func anyOtherFrameFlushesTheHeldText() {
        let session = session()
        let batch = FrameBatcher(session: session, interval: .seconds(3600))
        batch.push(ChatFrame(event: .delta("a")))
        batch.push(ChatFrame(event: .delta("b")))
        batch.push(ChatFrame(id: 1, event: .message(ChatMessage(seq: 1, role: .assistant, text: "ab", at: epoch))))
        #expect(session.messages.count == 1)
        #expect(session.reply.text == "")
    }

    @Test func theDataNoticeIsShownUntilDismissed() {
        let chat = Chat(id: "c1", harness: .codex, dataNotice: "First time: …")
        let session = ChatSession(ChatConversation(chat))
        #expect(session.dataNotice == "First time: …")
        session.dismissDataNotice()
        #expect(session.dataNotice == nil && session.conversation.chat.dataNotice == nil)
    }
}

// MARK: The conversation

@MainActor
@Suite("Conversation model")
struct ConversationModelTests {
    @Test func aReplyIsMarkedWithItsHarnessAndModel() async throws {
        let rig = Rig()
        let id = try #require(await rig.store.start(NewChat(harness: .claudeCode, model: "opus")))
        let task = rig.follow(id)
        let catalog = readyCatalog(models: [.claudeCode: [HarnessModel(id: "opus", title: "Opus", harness: .claudeCode)]])
        let model = ConversationModel(store: rig.store, chatID: id, catalog: catalog)
        model.draft = "hello"
        #expect(model.canSend)
        await model.send()
        #expect(model.draft == "")
        await rig.settle { model.items.contains { if case .reply = $0 { true } else { false } } && !model.isReplying }
        task.cancel()
        let reply = try #require(model.items.compactMap { item -> ChatMessage? in if case .reply(let message) = item { message } else { nil } }.first)
        #expect(model.label(for: reply)?.text == "Written by Claude Code · Opus")
        #expect(model.harnessLabel == "Claude Code · Opus")
        #expect(model.canRetry(reply) && model.copyText(reply) == "Noted: hello")
        #expect(model.label(for: ChatMessage(seq: 9, role: .user, text: "x", at: epoch)) == nil)
    }

    @Test func theCliDefaultModelHasNoModelInTheLabel() {
        let message = ChatMessage(seq: 1, role: .assistant, text: "x", at: epoch, harness: .grok, model: "default")
        #expect(ReplyLabel(message)?.text == "Written by Grok")
        let unknown = ChatMessage(seq: 1, role: .assistant, text: "x", at: epoch, harness: .grok, model: "grok-9")
        #expect(ReplyLabel(unknown)?.text == "Written by Grok · grok-9")
    }

    @Test func theSendButtonBecomesStopWhileReplyingAndStopKeepsWhatWasWritten() async throws {
        let rig = Rig()
        let id = try #require(await rig.store.start())
        let task = rig.follow(id)
        let model = ConversationModel(store: rig.store, chatID: id)
        model.draft = "[simulate:slow]"
        await model.pressButton()
        await rig.settle { model.isReplying && model.live?.text == "Working on it" }
        #expect(model.button == .stop && !model.canSend)
        model.draft = "again"
        #expect(!model.canSend)
        await model.pressButton()  // Stop
        await rig.settle { !model.isReplying }
        task.cancel()
        #expect(model.button == .send)
        let kept = model.items.contains { if case .reply(let message) = $0 { message.text == "Working on it" } else { false } }
        #expect(kept)
        let last = try #require(model.items.compactMap { item -> ChatMessage? in if case .reply(let message) = item { message } else { nil } }.last)
        #expect(model.canRetry(last))
    }

    @Test func aRateLimitShowsItsResetTimeAndSwitchingStartsTheSameWordsOnAnotherAgent() async throws {
        let rig = Rig()
        let limited = ChatError(
            code: .rateLimited, harness: .claudeCode, title: "Claude Code reached its usage limit", message: "It resets in an hour.",
            fixes: [.retry, .switchHarness], retry: ChatRetry(canRetryNow: false, resetsAt: epoch.addingTimeInterval(3600)), switchTo: .codex)
        await rig.mock.queue(ChatReplyScript(error: limited))
        let id = try #require(await rig.store.start(NewChat(harness: .claudeCode)))
        let task = rig.follow(id)
        let model = ConversationModel(store: rig.store, chatID: id, now: { epoch })
        model.draft = "do the thing"
        await model.send()
        await rig.settle { model.error != nil }
        let error = try #require(model.error)
        #expect(error.title == "Claude Code reached its usage limit")
        #expect(error.note != nil && error.note?.hasPrefix("Resets at ") == true)
        #expect(error.actions == [.tryAgain(enabled: false), .switchTo(.codex)])
        #expect(error.actions.map(\.title) == ["Try Again", "Use Codex"] && error.actions[0].isEnabled == false)

        var switched: String?
        model.onSwitched = { switched = $0 }
        await model.perform(.switchTo(.codex))
        task.cancel()
        let fresh = try #require(switched)
        #expect(rig.store.session(fresh)?.harness == .codex)
        let follower = rig.follow(fresh)
        await rig.settle { rig.store.session(fresh)?.messages.contains { $0.role == .assistant } == true }
        follower.cancel()
        #expect(rig.store.session(fresh)?.messages.first?.text == "do the thing")
        #expect(rig.store.order == [id, fresh])
    }

    @Test func anExpiredLoginOffersSignInWhichHandsTheHarnessToTheApp() async throws {
        let rig = Rig()
        let id = try #require(await rig.store.start(NewChat(harness: .codex)))
        let task = rig.follow(id)
        let model = ConversationModel(store: rig.store, chatID: id)
        var asked: Harness?
        model.onSignIn = { asked = $0 }
        model.draft = "[simulate:auth-expired]"
        await model.send()
        await rig.settle { model.error != nil }
        task.cancel()
        let error = try #require(model.error)
        #expect(error.actions == [.signIn(.codex)] && error.actions[0].title == "Sign In" && error.note == nil)
        await model.perform(error.actions[0])
        #expect(asked == .codex)
    }

    @Test func aRefusedSendKeepsTheWordsInTheComposer() async throws {
        let rig = Rig()
        let id = try #require(await rig.store.start())
        let model = ConversationModel(store: rig.store, chatID: id)
        model.draft = "[simulate:slow]"
        await model.send()
        model.draft = "second"
        await model.send()
        #expect(model.draft == "second" && model.actionError != nil)
        await model.stop()
    }

    @Test func theFirstUseNoticeShowsOnceThenDismisses() async throws {
        let rig = Rig()
        let id = try #require(await rig.store.start(NewChat(harness: .claudeCode)))
        let model = ConversationModel(store: rig.store, chatID: id)
        #expect(model.dataNotice?.contains("Anthropic") == true)
        model.dismissDataNotice()
        #expect(model.dataNotice == nil)
    }

    @Test func returnAndCommandReturnSendInOneLineAndOnlyCommandReturnInMultiLine() {
        #expect(ComposerKey.action(command: false, shift: false, multiline: false) == .send)
        #expect(ComposerKey.action(command: true, shift: false, multiline: false) == .send)
        #expect(ComposerKey.action(command: false, shift: true, multiline: false) == .newline)
        #expect(ComposerKey.action(command: false, shift: false, multiline: true) == .newline)
        #expect(ComposerKey.action(command: true, shift: false, multiline: true) == .send)
        #expect(ComposerKey.hint(multiline: true) == "Return adds a line · ⌘↩ sends" && ComposerKey.hint(multiline: false) == "⌘↩ sends")
    }
}

// MARK: Composer, draft card, steering

@MainActor
@Suite("Composer and draft card")
struct DraftFlowTests {
    private func start(_ rig: Rig, text: String = "Make checkout p95 faster", configure: (ComposerModel) -> Void = { _ in }) async throws
        -> (id: String, card: DraftCardModel, task: Task<Void, Never>)
    {
        let composer = ComposerModel(store: rig.store, options: ComposerOptions(repos: ["acme/shop"], catalog: readyCatalog()))
        composer.repo = "acme/shop"
        composer.text = text
        configure(composer)
        let id = try #require(await composer.send())
        let task = rig.follow(id)
        let card = DraftCardModel(store: rig.store, chatID: id)
        await rig.settle { card.card != nil && !card.isDrafting }
        return (id, card, task)
    }

    @Test func theChipsSayWhatTheWordsDoNotAndStartTheChatOnTheDefaultAgent() async throws {
        let rig = Rig()
        let composer = ComposerModel(
            store: rig.store, options: ComposerOptions(repos: ["acme/shop"], catalog: readyCatalog(), chatDefault: .codex, preferredOrder: [.grok, .claudeCode]))
        composer.repo = "acme/shop"
        composer.limits = LimitsEditor(budgetUSD: 20, hours: 1)
        composer.schedule = .weekdays
        composer.text = "Make checkout p95 faster."
        #expect(composer.agents.order == [.grok, .claudeCode, .codex])
        #expect(composer.title(of: .repo) == "acme/shop" && composer.title(of: .limits) == "$20 · 1 h per run" && composer.title(of: .schedule) == "Weekdays at night")
        #expect(composer.title(of: .agents) == "Grok + 2 more")
        #expect(composer.isSet(.limits) && !composer.isSet(.cloud))
        #expect(composer.message() == "Make checkout p95 faster. Each run may use up to $20 and 1 h. Run it on weekdays.")
        composer.agents.placement = .cloud
        #expect(composer.message().hasSuffix("Use Grok, then Claude Code, then Codex. Run them in the cloud."))
        composer.agents.placement = .local
        let id = try #require(await composer.send())
        #expect(rig.store.session(id)?.harness == .codex)
        #expect(composer.text == "" && composer.failure == nil)
    }

    @Test func wordsThatAlreadySayItAreNotRepeated() {
        let composer = ComposerModel(store: ChatStore(client: MockChatClient()), options: ComposerOptions(catalog: readyCatalog([.claudeCode])))
        composer.limits = LimitsEditor(budgetUSD: 20, hours: 1)
        composer.schedule = .nightly
        composer.text = "Make it faster, $50 a run, every night"
        #expect(composer.message() == "Make it faster, $50 a run, every night")
    }

    @Test func sendNeedsWordsAndASignedInAgentAndSaysWhyNot() async {
        let rig = Rig(ready: [])
        let composer = ComposerModel(store: rig.store, options: ComposerOptions(catalog: HarnessCatalog()))
        #expect(!composer.canSend && composer.blockedReason == nil)
        composer.text = "Make it faster"
        #expect(!composer.canSend && composer.blockedReason?.contains("No agent is signed in") == true)
        #expect(await composer.send() == nil && composer.text == "Make it faster")
        #expect(ComposerModel.hint == "⌘↩ sends")
    }

    @Test func aFailedStartKeepsTheWordsAndShowsWhy() async {
        let rig = Rig(ready: [])
        let composer = ComposerModel(store: rig.store, options: ComposerOptions(catalog: readyCatalog()))
        composer.text = "Make it faster"
        #expect(await composer.send() == nil)
        #expect(composer.failure?.contains("No agent is signed in") == true && composer.text == "Make it faster")
        #expect(rig.store.order.isEmpty)
    }

    @Test func theChipChoicesReachTheDraftCard() async throws {
        let rig = Rig()
        let (_, model, task) = try await start(rig) { $0.limits = LimitsEditor(budgetUSD: 20, hours: 1); $0.schedule = .weekdays }
        task.cancel()
        let rows = Dictionary(uniqueKeysWithValues: model.rows.map { ($0.row.key, $0.row.value) })
        #expect(rows["limits"] == "up to $20 and 1 h" && rows["schedule"] == "weekdays at 02:00")
        #expect(model.rows.map(\.changeTitle) == [
            "Change repository", "Change goal", "Change measure", "Change agents", "Change cloud", "Change limits", "Change schedule",
            "Change output", "Change config",
        ])
        // Repository and Cloud are on every card: the repo is read, not changed here; the cloud row follows the Home chip.
        #expect(rows["repo"] == "acme/shop" && rows["cloud"] == "None · agents run on this Mac")
        #expect(model.rows.first { $0.row.key == "repo" }?.isEditable == false)
        #expect(model.rows.filter(\.isUpdated).isEmpty)
        #expect(model.canCreate && model.createTitle == "Create Optimization")
    }

    @Test func changingARowShowsUpdatedAndUndoPutsItBack() async throws {
        let rig = Rig()
        let (_, model, task) = try await start(rig)
        guard case .agents(var editor)? = model.editor(for: "agents") else { Issue.record("no agents editor"); task.cancel(); return }
        #expect(editor.order == [.claudeCode] && editor.placement == .local && editor.addable == [.codex, .grok])
        editor.add(.codex)
        editor.moveUp(at: 1)
        #expect(editor.summary == "Codex → Claude Code · on this Mac")
        let before = model.card?.id
        await model.edit("agents", .agents(editor))
        await rig.settle { model.card?.id != before && !model.isDrafting }
        let agents = try #require(model.rows.first { $0.row.key == "agents" })
        #expect(agents.row.value == "Codex → Claude Code · on this Mac" && agents.isUpdated && agents.canUndo)
        #expect(model.rows.filter(\.isUpdated).count == 1)
        let replaced = rig.store.session(model.chatID)?.cards.first?.status
        #expect(replaced == .replaced)

        let second = model.card?.id
        await model.undo("agents")
        await rig.settle { model.card?.id != second && !model.isDrafting }
        task.cancel()
        let again = try #require(model.rows.first { $0.row.key == "agents" })
        #expect(again.row.value == "Claude Code · on this Mac" && !again.isUpdated)
    }

    @Test func limitsAndScheduleEditorsSayTheirChangeInWords() async throws {
        let rig = Rig()
        let (_, model, task) = try await start(rig)
        guard case .limits(var limits)? = model.editor(for: "limits") else { Issue.record("no limits editor"); task.cancel(); return }
        #expect(limits == LimitsEditor(budgetUSD: 50, hours: 2))
        limits.budgetUSD = 20
        limits.hours = 4
        #expect(DraftRowEdit.limits(limits).instruction == "Change each run to up to $20 and 4 h.")
        let before = model.card?.id
        await model.edit("limits", .limits(limits))
        await rig.settle { model.card?.id != before && !model.isDrafting }
        #expect(model.rows.first { $0.row.key == "limits" }?.row.value == "up to $20 and 4 h")
        let next = model.card?.id
        await model.edit("schedule", .schedule(.weekly))
        await rig.settle { model.card?.id != next && !model.isDrafting }
        task.cancel()
        #expect(model.rows.first { $0.row.key == "schedule" }?.row.value == "weekly at 02:00")
        #expect(model.editor(for: "output") == nil)
    }

    @Test func createAndCreateAndRunOnceShowTheirCost() async throws {
        let rig = Rig()
        let (_, model, task) = try await start(rig)
        #expect(model.createAndRunOnceTitle == "Create and Run Once · up to $50")
        model.typicalRunCostUSD = 38
        #expect(model.createAndRunOnceTitle == "Create and Run Once · ≈$38")
        await model.createAndRunOnce()
        await rig.settle { model.status == .created && !model.isDrafting }
        task.cancel()
        #expect(model.createdOptimizationID != nil && !model.canCreate)
        let schedule = rig.store.session(model.chatID)?.cards.last?.rows.first { $0.key == "schedule" }?.raw
        #expect(schedule == .null)  // run once: no schedule is added
    }

    @Test func createMakesTheOptimizationAndLeavesTheScheduleAlone() async throws {
        let rig = Rig()
        let (_, model, task) = try await start(rig)
        await model.create()
        await rig.settle { model.status == .created && !model.isDrafting }
        task.cancel()
        #expect(model.createdOptimizationID?.hasPrefix("opt-created") == true)
        let schedule = rig.store.session(model.chatID)?.cards.last?.rows.first { $0.key == "schedule" }?.value
        #expect(schedule == "every night at 02:00")
    }

    @Test func aFailedEnvironmentCheckDisablesBothCreateButtonsAndOffersTwoFixes() async throws {
        let rig = Rig()
        let (_, model, task) = try await start(rig, text: "Make it faster [simulate:env-fail]")
        #expect(model.status == .failed && !model.canCreate)
        #expect(model.failure?.title == "Couldn't lock the environment")
        #expect(model.fixes.map(\.title) == ["Use Smaller Setup", "Try Again"])
        await model.create()  // refused: nothing is sent
        #expect(rig.store.session(model.chatID)?.messages.filter { $0.role == .user }.count == 1)
        await model.apply(.useSmallerSetup)
        await rig.settle { model.status == .draft && !model.isDrafting }
        task.cancel()
        #expect(model.failure == nil && model.canCreate && model.fixes.isEmpty)
    }

    @Test func stopEndsTheDraftAndKeepsTheStepsSoFar() async throws {
        let rig = Rig()
        let composer = ComposerModel(store: rig.store, options: ComposerOptions(catalog: readyCatalog()))
        composer.text = "[simulate:slow]"
        let id = try #require(await composer.send())
        let task = rig.follow(id)
        let model = DraftCardModel(store: rig.store, chatID: id)
        await rig.settle { model.isDrafting && rig.store.session(id)?.reply.text == "Working on it" }
        #expect(model.isDrafting)
        await model.stop()
        await rig.settle { !model.isDrafting }
        task.cancel()
        #expect(!model.isDrafting && rig.store.session(id)?.lastOutcome == .interrupted)
    }

    @Test func startOverEndsTheConversationAndBringsThePromptBack() async throws {
        let rig = Rig()
        let (id, model, task) = try await start(rig, text: "Make checkout p95 faster in shop-api")
        task.cancel()
        let prompt = await model.startOver()
        #expect(prompt == "Make checkout p95 faster in shop-api")
        #expect(rig.store.session(id) == nil && rig.store.order.isEmpty)
        let composer = ComposerModel(store: rig.store, options: ComposerOptions(catalog: readyCatalog()))
        composer.restore(prompt: prompt ?? "")
        #expect(composer.text == "Make checkout p95 faster in shop-api")
    }
}

@MainActor
@Suite("Steering")
struct SteeringTests {
    @Test func aNoteIsQueuedThenReadAndCanBeTakenBackOnlyUntilThen() async throws {
        let rig = Rig()
        let model = SteeringModel(store: rig.store, optimizationID: "opt-1", timeZone: utc)
        #expect(model.hint == "Return adds a line · ⌘↩ sends")
        model.draft = "focus on database queries"
        #expect(model.canSend)
        await model.send()
        let id = try #require(model.chatID)
        let task = rig.follow(id)
        await rig.settle { !model.entries.isEmpty }
        let first = try #require(model.entries.first)
        #expect(first.text == "focus on database queries" && first.status == .queued && first.canUndo)
        #expect(first.statusLine == "Queued · read at the next candidate")
        #expect(model.draft == "")

        model.draft = "skip the cache layer"
        await model.send()
        await rig.settle { model.entries.count == 2 }
        // The first is taken back while it is still queued.
        await model.undo(first.id)
        await rig.settle { model.entries.first?.status == .withdrawn }
        #expect(model.entries.first?.statusLine == "Withdrawn" && model.entries.first?.canUndo == false)

        // An agent reads the second one.
        await rig.mock.readNotes(chatID: id, by: .claudeCode, at: epoch)
        await rig.settle { if case .read? = model.entries.last?.status { true } else { false } }
        let read = try #require(model.entries.last)
        #expect(read.statusLine == "Read by Claude Code at 13:20" && !read.canUndo)
        await model.undo(read.id)  // too late: nothing happens
        try? await Task.sleep(for: .milliseconds(50))
        task.cancel()
        #expect(model.entries.last?.statusLine == "Read by Claude Code at 13:20")
    }

    @Test func aChatThatCannotOpenKeepsTheNoteAndSaysWhy() async {
        let rig = Rig(ready: [])
        let model = SteeringModel(store: rig.store, optimizationID: "opt-1")
        model.draft = "focus on queries"
        await model.send()
        #expect(model.chatID == nil && model.draft == "focus on queries" && model.failure?.contains("No agent is signed in") == true)
    }
}
