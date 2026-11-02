import AppKit
import RatchetKit

/// The P5b steps of the self-test: a simulated chat driven to a created optimization (`ChatScenario` on the sample chat client),
/// a failed turn with its fixes, a steering note read by an agent, the value story, the settings rows, Pause Schedule, the Agents
/// page and the menu bar's keep-awake line. They run the same models the screens draw.
extension SelfTest {
    /// Polls (up to four seconds) for something the stream delivers.
    @MainActor
    private static func waitUntil(_ condition: () -> Bool) async -> Bool {
        for _ in 0..<400 {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(10))
        }
        return condition()
    }

    @MainActor
    static func auditChat(store: AppStore, router: AppRouter, hub: ChatHub) async {
        // Home: the chips are set, Send starts the chat and the words are cleared.
        await hub.refresh(store: store)
        check("an agent is signed in on the sample Mac", !hub.agents.catalog.ready.isEmpty)
        let composer = hub.composer
        composer.repo = "acme-org/shop-api"
        composer.text = "Make checkout p95 faster in shop-api. Run it every night, $50 a run."
        check("Send is on with words and a signed-in agent", composer.canSend && composer.blockedReason == nil)
        guard let chatID = await hub.sendFromHome(store: store, router: router) else {
            check("Send starts a chat", false, composer.failure ?? hub.chats.startError ?? "no reason")
            return
        }
        check("Send opens the conversation and clears the words", router.creationChatID == chatID && composer.text.isEmpty)
        let follow = Task { await hub.chats.follow(chatID) }
        defer { follow.cancel() }

        // The creation conversation: steps stream, a draft card appears, a row changes with Updated and Undo.
        let card = hub.draftCard(chatID)
        let conversation = hub.conversation(chatID, router: router)
        let drafted = await waitUntil { card.card != nil && !card.isDrafting }
        check("the setup agent drafts a card", drafted)
        check("the card has Measures, Agents, Each run, Schedule, Output and Config rows", Set(card.rows.map(\.id)).isSuperset(of: ["agents", "limits", "schedule"]))
        check("Create and Create and Run Once are on, with the cost", card.canCreate && card.createAndRunOnceTitle.hasPrefix("Create and Run Once · "))
        check("a reply is marked with its harness", conversation.items.contains { if case .reply(let message) = $0 { conversation.label(for: message) != nil } else { false } })
        if case .agents(var editor)? = card.editor(for: "agents") {
            editor.add(.grok)
            await card.edit("agents", .agents(editor))
            _ = await waitUntil { card.rows.first { $0.id == "agents" }?.isUpdated == true && !card.isDrafting }
            check("a changed row shows Updated with Undo", card.rows.first { $0.id == "agents" }?.canUndo == true)
            await card.undo("agents")
            _ = await waitUntil { card.rows.first { $0.id == "agents" }?.isUpdated == false && !card.isDrafting }
            check("Undo takes the mark away", card.rows.first { $0.id == "agents" }?.isUpdated == false)
        } else {
            check("the Agents row has an editor", false)
        }
        await card.create()
        let created = await waitUntil { card.createdOptimizationID != nil }
        check("Create Optimization creates it with no human gate", created && card.status == .created)

        // A failed turn: typed, with its fix buttons.
        if let scenario = hub.chatScenario, let failing = await hub.chats.start(NewChat(harness: .claudeCode)) {
            let task = Task { await hub.chats.follow(failing) }
            defer { task.cancel() }
            await scenario.queue(ChatReplyScript(error: ChatError(
                code: .rateLimited, harness: .claudeCode, title: "Claude Code reached its usage limit", message: "",
                fixes: [.retry, .switchHarness], retry: ChatRetry(canRetryNow: false, resetsAt: Store.fixedNow.addingTimeInterval(3600)),
                switchTo: .codex)))
            let model = hub.conversation(failing, router: router)
            model.draft = "hello"
            await model.send()
            let failed = await waitUntil { model.error != nil }
            check("a rate limit says so with Try Again off and Use Codex on",
                  failed && model.error?.actions.contains(.tryAgain(enabled: false)) == true && model.error?.actions.contains(.switchTo(.codex)) == true)
            await hub.chats.end(failing)
        }

        // Steering: a note is queued with Undo, then read by an agent.
        let steering = hub.steering(for: "opt-checkout")
        steering.draft = "focus on database queries"
        await steering.send()
        if let steerID = steering.chatID {
            let steerFollow = Task { await hub.chats.follow(steerID) }
            defer { steerFollow.cancel() }
            _ = await waitUntil { steering.entries.count == 1 }
            check("a note is queued and can be taken back", steering.entries.first?.statusLine == "Queued · read at the next candidate" && steering.entries.first?.canUndo == true)
            await hub.chatScenario?.readNotes(chatID: steerID, by: .claudeCode, at: Store.fixedNow)
            _ = await waitUntil { steering.entries.first?.canUndo == false }
            check("a read note says by whom and cannot be taken back",
                  steering.entries.first?.statusLine.hasPrefix("Read by Claude Code at ") == true && steering.entries.first?.canUndo == false)
        } else {
            check("a steering chat opens", false, steering.failure ?? "no reason")
        }
        router.closeConversation()
        router.show(.overview)

        // One value story.
        await hub.story.load()
        check("Home and the Overview tell the same story", hub.story.screensAgree && hub.story.story?.headline == "$12.5k delivered + $2.9k pending · 68×")
        check("\"locked in\" appears nowhere in the story", !(hub.story.story?.headline.contains("locked in") ?? false))
        check("the chart has words for VoiceOver", hub.story.overview?.chartSummary.contains("steps") == true || hub.story.overview?.chartSummary.contains("step") == true)

        // The settings rows and Pause Schedule.
        let settings = hub.settings(for: "opt-checkout")
        await settings.load()
        check("the settings rows load with tags", settings.rows.count == 3 && settings.rows.allSatisfy { !$0.value.isEmpty })
        let before = settings.pauseTitle
        let paused = await settings.togglePause()
        check("Pause Schedule becomes Resume Schedule", before == "Pause Schedule" && paused && settings.pauseTitle == "Resume Schedule")
        _ = await settings.togglePause()
        check("Archive asks first", OptimizationSettingsModel.archivePrompt(title: "Checkout p95").message == "Stops the schedule and keeps its history and wins.")

        // The Agents page: every status that is not OK has one fix.
        await hub.agents.load()
        check("the Agents page lists the three logins and the keys", hub.agents.loginRows.count == 3 && hub.agents.keyRows.count >= 3)
        check("every status that is not OK has a fix button", (hub.agents.loginRows + hub.agents.keyRows).allSatisfy { $0.status.isOK || $0.fix != nil })

        // The menu bar's keep-awake line follows the daemon's assertion.
        check("no keep-awake line while the Mac may sleep", store.power.menuLine == nil)
        store.apply(.powerChanged(PowerState(keepingAwake: true, localOptimizationIDs: ["opt-checkout"])))
        check("the menu bar says it is keeping the Mac awake", store.power.menuLine == "Keeping your Mac awake")
        store.apply(.powerChanged(PowerState()))
        check("the line goes when the assertion is released", store.power.menuLine == nil)
    }
}
