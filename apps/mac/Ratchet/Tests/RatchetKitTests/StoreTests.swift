import Foundation
import Testing
@testable import RatchetKit

/// The stores AppStore is made of, and the work that moved off the main actor or out of the way.
@MainActor
@Suite("Stores")
struct StoreTests {
    private let now = Date(timeIntervalSince1970: 1_793_452_800)

    private func loaded() async -> (AppStore, MockRatchetClient) {
        let client = MockRatchetClient(now: now)
        let store = AppStore(client: client, scenario: client)
        await store.refresh()
        return (store, client)
    }

    // MARK: Roots and children

    @Test func eachStoreOwnsItsPartOfWhatARefreshLoads() async {
        let (store, _) = await loaded()
        #expect(store.optimizationStore.optimizations.count == 5)
        #expect(store.connectorStore.connectors.count == 8)
        #expect(store.connectorStore.connectors(of: .aws).count == 3)
        #expect(store.sandboxStore.sandbox.isOn && store.sandboxStore.receipt != nil)
        #expect(store.sectionErrors.isEmpty)
        #expect(store.scenario != nil)
    }

    @Test func aStoreActionShowsItsFailureOnTheSharedErrorLine() async {
        let (store, _) = await loaded()
        await store.connectorStore.removeConnector("missing")
        #expect(store.lastError != nil)
        await store.sandboxStore.endSandbox()
        #expect(store.lastError == nil)
    }

    // MARK: Events through the reducer

    @Test func eventsFromTheSampleClientDriveEveryStore() async {
        let (store, client) = await loaded()
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        for _ in 0..<2 {
            for event in await client.advance() { store.apply(event) }
        }
        #expect(store.optimizationStore.optimization(id: "opt-checkout")?.winsCount == 3)
        #expect(store.optimizationStore.details["opt-checkout"]?.moves.count == 3)
        #expect(store.overview?.winsCount == 10)
    }

    @Test func aStreamStateEventIsKept() async {
        let (store, _) = await loaded()
        #expect(store.streamState == .connecting)
        store.apply(.streamStateChanged(.live))
        #expect(store.streamState == .live)
        store.apply(.streamStateChanged(.reconnecting(attempt: 3, reason: "gone")))
        #expect(store.streamState == .reconnecting(attempt: 3, reason: "gone"))
    }

    @Test func notificationsFollowTheReducerAndOnlyOnceLoaded() async throws {
        let client = MockRatchetClient(now: now)
        let store = AppStore(client: client)
        var posted: [NotificationRequest] = []
        store.onNotification = { posted.append($0) }
        // Before the first load nothing is announced.
        store.apply(.environmentDraftReady(draftID: "d0", repoID: "acme/shop"))
        #expect(posted.isEmpty)
        await store.refresh()
        store.apply(.environmentDraftReady(draftID: "d1", repoID: "acme/shop"))
        #expect(posted.map(\.id) == ["draft-d1"])
    }

    // MARK: Resync

    @Test func manyResyncsAtOnceAreOneReloadAndOneMore() async {
        let (store, client) = await loaded()
        _ = try? await client.addConnector(NewConnector(kind: .github, name: "octo"))
        for _ in 0..<5 { store.apply(.resync) }
        for _ in 0..<300 where store.connectorStore.connectors.count == 8 { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(store.connectorStore.connectors.count == 9)
    }

    // MARK: Details are kept for the recent ones only

    @Test func theOldestFinishedDetailsAreDroppedPastTheLimit() {
        let order = (1...30).map { "o\($0)" }
        let victims = Eviction.victims(order: order, keeping: ["o1", "o30"], limit: 24)
        #expect(victims == ["o2", "o3", "o4", "o5", "o6", "o7"])
        #expect(Eviction.victims(order: order, keeping: [], limit: 30).isEmpty)
        #expect(Eviction.victims(order: ["a", "b"], keeping: ["a"], limit: 0) == ["b"])
    }

    // MARK: The log

    @Test func readingTheSameLogTwiceAtOnceIsOneReadThenOneMore() async {
        let (store, _) = await loaded()
        async let first: Void = store.insight.loadAgentLog("agent-claude-team")
        async let second: Void = store.insight.loadAgentLog("agent-claude-team")
        _ = await (first, second)
        let log = store.insight.log("agent-claude-team")
        #expect(log?.isLoaded == true)
        #expect(log?.lines.map(\.id) == log?.lines.map(\.id).sorted())
        #expect(Set(log?.lines.map(\.id) ?? []).count == log?.lines.count)
    }

    @Test func aLogLineChangesTheLogModelAndNotTheDetail() async {
        let (store, client) = await loaded()
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        let model = store.insight.logModel("agent-claude-team")
        await store.insight.loadAgentLog("agent-claude-team")
        let detailBefore = store.optimizationStore.details["opt-checkout"]
        let linesBefore = model.log.lines.count
        _ = await client.advance()
        _ = await client.advance()
        // The announcement of new lines reads them into the log's own model; the detail is not touched.
        store.apply(.agentLogAppended(optimizationID: "opt-checkout", agentID: "agent-claude-team"))
        for _ in 0..<300 where model.log.lines.count == linesBefore { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(model.log.lines.count > linesBefore)
        #expect(store.optimizationStore.details["opt-checkout"] == detailBefore)
    }

    @Test func theModelsOfLongAgoAreDropped() async {
        let (store, _) = await loaded()
        let first = store.insight.logModel("agent-0")
        for number in 1...InsightsStore.logLimit { _ = store.insight.logModel("agent-\(number)") }
        #expect(!store.insight.hasLog("agent-0"))
        #expect(store.insight.hasLog("agent-\(InsightsStore.logLimit)"))
        #expect(first.log.lines.isEmpty)
    }

    // MARK: Cache

    @Test func theCacheWritesTheLastStateOnlyAndLogsNothingWhenItWorks() async throws {
        let url = FileManager.default.temporaryDirectory.appending(path: "ratchet-cache-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        let cache = SnapshotCache(url: url, delay: .seconds(60))
        func state(_ repo: String) -> CachedState {
            CachedState(
                overview: nil, optimizations: [], connectors: [], templates: [], benchmarks: [],
                repos: [Repo(id: repo, account: "acme")], sandbox: .off, receipt: nil)
        }
        await cache.save(state("acme/one"))
        await cache.save(state("acme/two"))
        #expect(!FileManager.default.fileExists(atPath: url.path))
        await cache.flush()
        let loadedState = await cache.load()
        #expect(loadedState?.repos.map(\.id) == ["acme/two"])
    }

    @Test func aCacheThatCannotBeWrittenDoesNotCrashOrLeaveAFile() async {
        let url = URL(fileURLWithPath: "/nonexistent-root-\(UUID().uuidString)/cache.json")
        let cache = SnapshotCache(url: url, delay: .milliseconds(1))
        await cache.save(CachedState(
            overview: nil, optimizations: [], connectors: [], templates: [], benchmarks: [], repos: [], sandbox: .off, receipt: nil))
        await cache.flush()
        #expect(await cache.load() == nil)
    }

    @Test func switchingTheCacheFileForgetsAWriteThatWaited() async throws {
        let first = FileManager.default.temporaryDirectory.appending(path: "ratchet-cache-\(UUID().uuidString).json")
        let second = FileManager.default.temporaryDirectory.appending(path: "ratchet-cache-\(UUID().uuidString).json")
        defer {
            try? FileManager.default.removeItem(at: first)
            try? FileManager.default.removeItem(at: second)
        }
        let cache = SnapshotCache(url: first, delay: .seconds(60))
        await cache.save(CachedState(
            overview: nil, optimizations: [], connectors: [], templates: [], benchmarks: [], repos: [], sandbox: .off, receipt: nil))
        await cache.use(second)
        await cache.flush()
        #expect(!FileManager.default.fileExists(atPath: first.path))
        #expect(!FileManager.default.fileExists(atPath: second.path))
    }
}

/// The chat store over the mock chat client.
@MainActor
@Suite("Chat store")
struct ChatStoreTests {
    private func make() -> (ChatStore, MockChatClient) {
        let client = MockChatClient(now: Date(timeIntervalSince1970: 1_793_452_800))
        return (ChatStore(client: client, pause: { _ in }), client)
    }

    private func settle(_ store: ChatStore, _ id: String, until done: @MainActor (ChatConversation) -> Bool) async {
        for _ in 0..<400 {
            if let conversation = store.conversation(id), done(conversation) { return }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    @Test func aChatStartsAndFollowsItsOwnStream() async throws {
        let (store, _) = make()
        let id = try #require(await store.start(NewChat(repo: "acme/shop")))
        let follower = Task { await store.follow(id) }
        await settle(store, id) { $0.chat.lastEventID >= 1 }
        await store.send(id, text: "hello")
        await settle(store, id) { $0.lastOutcome == .completed }
        follower.cancel()
        await follower.value
        let conversation = try #require(store.conversation(id))
        #expect(conversation.chat.messages.map(\.role) == [.user, .assistant])
        #expect(conversation.lastOutcome == .completed && !conversation.isReplying)
        #expect(store.order == [id])
    }

    @Test func aFailedTurnIsKeptOnTheConversationAndRetryAsksAgain() async throws {
        let (store, _) = make()
        let id = try #require(await store.start())
        let follower = Task { await store.follow(id) }
        await store.send(id, text: "[simulate:rate-limit]")
        await settle(store, id) { $0.error != nil }
        #expect(store.conversation(id)?.error?.isRateLimited == true)
        await store.retry(id)
        await settle(store, id) { $0.chat.messages.filter { $0.role == .user }.count == 2 }
        follower.cancel()
        await follower.value
        #expect(store.conversation(id)?.chat.messages.filter { $0.role == .user }.count == 2)
    }

    @Test func aRefusalIsKeptOnTheConversationNotThrown() async throws {
        let (store, _) = make()
        let id = try #require(await store.start())
        await store.send(id, text: "[simulate:slow]")
        await store.send(id, text: "again")
        #expect(store.conversation(id)?.actionError != nil)
        await store.interrupt(id)
    }

    @Test func startingWithNoHarnessReadySaysWhy() async {
        let store = ChatStore(client: MockChatClient(readyHarnesses: []), pause: { _ in })
        #expect(await store.start() == nil)
        #expect(store.startError?.contains("No agent is signed in") == true)
    }

    @Test func endingAChatForgetsIt() async throws {
        let (store, _) = make()
        let id = try #require(await store.start())
        await store.end(id)
        #expect(store.conversation(id) == nil && store.order.isEmpty)
    }
}
