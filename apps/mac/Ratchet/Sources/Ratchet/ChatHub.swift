import Observation
import RatchetKit
import SwiftUI

/// What the Agents page tells the person after Sign In: the command to run in Terminal (a login cannot be done from here).
struct SignInHelp: Equatable, Identifiable {
    let harness: Harness
    let command: String

    var id: String { harness.rawValue }
}

/// The chat side of the app, in one place: the chat store, Home's composer, the value story, the Agents page's model, and the
/// per-conversation, per-run and per-optimization models the screens draw. Its own object, apart from `AppStore`, so a streamed
/// word redraws the live text and nothing else (P5a keeps each part its own observable; this only hands them out).
@MainActor @Observable
final class ChatHub {
    let chats: ChatStore
    let story: ValueStoryModel
    let agents: AgentsPageModel
    let composer: ComposerModel
    let harness: any HarnessClient
    let preferences: any HarnessPreferences
    /// How a snapshot or the self-test makes the sample chat say something particular; nil with the real daemon.
    let chatScenario: (any ChatScenario)?
    /// The sample value story, so a snapshot can say "no wins yet" (nil with the real daemon).
    let sampleStory: MockHarnessClient?

    /// The command to run after Sign In, shown under the row on the Agents page.
    var signInHelp: SignInHelp?
    /// Home's first-use line ("Your messages … go to Claude Code") was closed.
    var homeNoticeClosed: Bool
    /// "Ask about a run": the chat of each optimization asked about (one per optimization, kept while the app runs), and the
    /// optimization whose sheet is open.
    private(set) var askChats: [String: String] = [:]
    var asking: String?

    @ObservationIgnored private var conversations: [String: ConversationModel] = [:]
    @ObservationIgnored private var drafts: [String: DraftCardModel] = [:]
    @ObservationIgnored private var settings: [String: OptimizationSettingsModel] = [:]
    @ObservationIgnored private var steering: [String: SteeringModel] = [:]
    private static let noticeKey = "home.dataNoticeClosed"

    init(
        chatClient: any ChatClient, chatScenario: (any ChatScenario)?, harness: any HarnessClient,
        connectors: any ConnectorsClient, preferences: any HarnessPreferences, sampleStory: MockHarnessClient? = nil, now: @escaping @Sendable () -> Date = { Date() }
    ) {
        let chats = ChatStore(client: chatClient)
        self.chats = chats
        self.chatScenario = chatScenario
        self.sampleStory = sampleStory
        self.harness = harness
        self.preferences = preferences
        self.story = ValueStoryModel(client: harness)
        self.agents = AgentsPageModel(client: connectors, preferences: preferences)
        self.composer = ComposerModel(store: chats)
        self.homeNoticeClosed = !Store.isOverridden && UserDefaults.standard.bool(forKey: Self.noticeKey)
        self.now = now
        wireAgents()
    }

    @ObservationIgnored private let now: @Sendable () -> Date

    /// The sample chat and sample numbers in test and snapshot mode and in `APP_DEMO`, the daemon's own otherwise.
    static func make(for store: AppStore) -> ChatHub {
        if Store.isOverridden || ProcessInfo.processInfo.environment["APP_DEMO"] == "1" {
            let now = Store.isOverridden ? Store.fixedNow : Date()
            let chat = MockChatClient(now: now)
            let story = MockHarnessClient(scenario: .wins, now: now)
            return ChatHub(
                chatClient: chat, chatScenario: chat, harness: story, connectors: store.client,
                preferences: MemoryHarnessPreferences(), sampleStory: story, now: { now })
        }
        let location = DaemonLocation.standard()
        return ChatHub(
            chatClient: DaemonChatClient(location: location), chatScenario: nil, harness: DaemonHarnessClient(location: location),
            connectors: store.client, preferences: UserDefaultsHarnessPreferences())
    }

    // MARK: Loading

    /// Loads what the chat screens need once the daemon answers, and again when it comes back or the range changes.
    func refresh(store: AppStore) async {
        story.range = store.range
        await agents.load()
        await story.load()
        updateComposerOptions(store: store)
    }

    /// What the Home chips offer, from the repos, the AWS accounts and the harnesses on this Mac.
    func updateComposerOptions(store: AppStore) {
        let options = ComposerOptions(
            repos: store.repos.map(\.id), cloudAccounts: store.connectorStore.connectors(of: .aws).map(\.name),
            catalog: agents.catalog, chatDefault: agents.chatDefault, preferredOrder: agents.optimizationOrder)
        composer.update(options: options)
        for model in conversations.values { model.catalog = agents.catalog }
        for model in drafts.values { model.cloudAccounts = options.cloudAccounts }
        for model in settings.values { model.defaultOrder = agents.optimizationOrder }
    }

    // MARK: Models by id

    func conversation(_ chatID: String, router: AppRouter) -> ConversationModel {
        if let model = conversations[chatID] { return model }
        let model = ConversationModel(store: chats, chatID: chatID, catalog: agents.catalog, now: now)
        model.onSignIn = { [weak router] _ in router?.show(.agents) }
        model.onSwitched = { [weak router] fresh in router?.openConversation(fresh) }
        conversations[chatID] = model
        return model
    }

    func draftCard(_ chatID: String) -> DraftCardModel {
        if let model = drafts[chatID] { return model }
        let model = DraftCardModel(store: chats, chatID: chatID, typicalRunCostUSD: story.typicalRunCostUSD)
        model.cloudAccounts = composer.options.cloudAccounts
        drafts[chatID] = model
        return model
    }

    func settings(for optimizationID: String) -> OptimizationSettingsModel {
        if let model = settings[optimizationID] { return model }
        let model = OptimizationSettingsModel(client: harness, optimizationID: optimizationID, defaultOrder: agents.optimizationOrder)
        settings[optimizationID] = model
        return model
    }

    func steering(for optimizationID: String) -> SteeringModel {
        if let model = steering[optimizationID] { return model }
        let model = SteeringModel(store: chats, optimizationID: optimizationID)
        steering[optimizationID] = model
        return model
    }

    /// Forgets what a finished conversation held.
    func forget(_ chatID: String) {
        conversations[chatID] = nil
        drafts[chatID] = nil
    }

    // MARK: Starting a creation conversation

    /// Home's Send: starts the chat and opens the conversation. The words stay in the composer when it could not start.
    @discardableResult
    func sendFromHome(store: AppStore, router: AppRouter) async -> String? {
        updateComposerOptions(store: store)
        // The cloud chip is not part of the first message; the card's Cloud row remembers it.
        let cloud = composer.cloudAccount
        guard let id = await composer.send() else { return nil }
        draftCard(id).cloudAccount = cloud
        router.openConversation(id)
        return id
    }

    /// "Use Template…": this conversation, with the template already chosen. The setup agent drafts from it.
    @discardableResult
    func startFromTemplate(_ template: Template, store: AppStore, router: AppRouter) async -> String? {
        updateComposerOptions(store: store)
        let catalog = agents.catalog
        let harness = agents.chatDefault ?? catalog.ready.first
        guard let id = await chats.start(NewChat(harness: harness, model: harness.flatMap { catalog.choice(for: $0).model?.id })) else {
            store.errors.report("Couldn't start the conversation. " + (chats.startError ?? "No agent is signed in."))
            return nil
        }
        await chats.send(id, text: "Draft an optimization from the “\(template.title)” template. \(template.summary) It measures \(template.measures).")
        if let failure = chats.session(id)?.actionError {
            store.errors.report("Couldn't start the conversation. " + failure)
            await chats.end(id)
            return nil
        }
        router.openConversation(id)
        return id
    }

    // MARK: Ask about a run

    /// "Ask…" on a run: a chat bound to that optimization on the default agent (the daemon gives it the run's context), shown in a
    /// sheet. One chat per optimization while the app runs. When no agent is signed in the error banner says so.
    func askAboutRun(_ optimizationID: String, store: AppStore) async {
        updateComposerOptions(store: store)
        if askChats[optimizationID] == nil {
            let catalog = agents.catalog
            let harness: Harness? = agents.chatDefault ?? catalog.ready.first
            var model: String?
            if let harness { model = catalog.choice(for: harness).model?.id }
            let new = NewChat(optimization: optimizationID, harness: harness, model: model)
            guard let id = await chats.start(new) else {
                store.errors.report("Couldn't start the conversation. " + (chats.startError ?? "No agent is signed in."))
                return
            }
            askChats[optimizationID] = id
        }
        asking = optimizationID
    }

    /// The conversation of an optimization's ask sheet; "Use Codex" after a failed turn moves the sheet to the new chat.
    func askConversation(_ optimizationID: String, router: AppRouter) -> ConversationModel? {
        guard let chatID = askChats[optimizationID] else { return nil }
        let model = conversation(chatID, router: router)
        model.onSwitched = { [weak self] fresh in self?.askChats[optimizationID] = fresh }
        return model
    }

    /// Start Over: the conversation ends and Home comes back with the first words restored.
    func startOver(chatID: String, router: AppRouter) async {
        let prompt = await draftCard(chatID).startOver()
        forget(chatID)
        router.closeConversation()
        router.newOptimization()
        if let prompt { composer.restore(prompt: prompt) }
    }

    func closeHomeNotice() {
        homeNoticeClosed = true
        if !Store.isOverridden { UserDefaults.standard.set(true, forKey: Self.noticeKey) }
    }

    // MARK: Agents page hooks

    /// Wires the Agents page's "Sign In" and the model's help, once.
    func wireAgents() {
        agents.onSignIn = { [weak self] harness, command in self?.signInHelp = SignInHelp(harness: harness, command: command) }
    }
}
