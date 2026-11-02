import Foundation
import Testing
@testable import RatchetKit

@MainActor
@Suite("App store")
struct AppStoreTests {
    private func loadedStore() async -> (AppStore, MockRatchetClient) {
        let client = MockRatchetClient(now: testNow)
        let store = AppStore(client: client)
        await store.refresh()
        return (store, client)
    }

    @Test func refreshFillsEveryList() async {
        let (store, _) = await loadedStore()
        #expect(store.isLoaded)
        #expect(store.optimizationStore.optimizations.count == 5)
        #expect(store.connectorStore.connectors.count == 8)
        #expect(store.templates.count == 10)
        #expect(store.benchmarks.count == 5)
        #expect(store.repos.count == 6)
        #expect(store.sandboxStore.sandbox.isOn)
        #expect(store.sandboxStore.receipt != nil)
        #expect(store.overview?.spentUSD == 184)
        #expect(store.needsYouCount == 2)
        #expect(store.optimizationStore.runningOptimizations.map(\.id) == ["opt-checkout"])
    }

    @Test func filtersSplitRunningAndDone() async {
        let (store, _) = await loadedStore()
        #expect(store.optimizationStore.visibleOptimizations(.all).count == 5)
        #expect(store.optimizationStore.visibleOptimizations(.running).map(\.id) == ["opt-checkout", "opt-search"])
        #expect(store.optimizationStore.visibleOptimizations(.done).count == 3)
    }

    @Test func changingTheRangeReloadsTheOverview() async {
        let (store, _) = await loadedStore()
        await store.setRange(.days30)
        #expect(store.range == .days30)
        #expect(store.overview?.range == .days30)
        #expect(store.overview?.winsCount == 3)
    }

    @Test func anUpdatedOptimizationMovesTheTotals() async {
        let (store, _) = await loadedStore()
        guard var checkout = store.optimizationStore.optimization(id: "opt-checkout") else {
            Issue.record("missing opt-checkout")
            return
        }
        checkout.spentUSD += 2
        checkout.valuePerYearUSD = (checkout.valuePerYearUSD ?? 0) + 600
        checkout.winsCount += 1
        let points = store.overview?.series.count ?? 0
        store.apply(.optimizationUpdated(checkout))
        #expect(store.overview?.spentUSD == 186)
        #expect(store.overview?.valueLockedPerYearUSD == 16_000)
        #expect(store.overview?.winsCount == 10)
        #expect(store.overview?.series.count == points + 1)
        #expect(store.overview?.series.last?.valuePerYearUSD == 16_000)
        #expect(store.overview?.series.last?.win == nil)
        #expect(store.optimizationStore.optimization(id: "opt-checkout")?.winsCount == 3)
    }

    @Test func detailEventsUpdateTheLoadedDetail() async {
        let (store, _) = await loadedStore()
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        let candidates = store.optimizationStore.details["opt-checkout"]?.candidates.count ?? 0
        let fresh = Candidate(id: "ffff000", title: "New idea", agent: "Claude Code · your login", verdict: .evaluating, at: testNow)
        store.apply(.candidateUpdated(optimizationID: "opt-checkout", candidate: fresh))
        #expect(store.optimizationStore.details["opt-checkout"]?.candidates.count == candidates + 1)
        var done = fresh
        done.verdict = .noise
        store.apply(.candidateUpdated(optimizationID: "opt-checkout", candidate: done))
        #expect(store.optimizationStore.details["opt-checkout"]?.candidates.count == candidates + 1)
        #expect(store.optimizationStore.details["opt-checkout"]?.candidates.first?.verdict == .noise)

        let move = RatchetMove(id: "m9", at: testNow, value: 250, title: "New idea")
        store.apply(.moveAdded(optimizationID: "opt-checkout", move: move))
        store.apply(.moveAdded(optimizationID: "opt-checkout", move: move))
        #expect(store.optimizationStore.details["opt-checkout"]?.moves.count == 3)

        let agent = Agent(id: "agent-claude-login", name: "Claude Code · your login", placement: .local, state: .paused)
        store.apply(.agentUpdated(optimizationID: "opt-checkout", agent: agent))
        #expect(store.optimizationStore.details["opt-checkout"]?.agents.first { $0.id == agent.id }?.state == .paused)
    }

    @Test func sandboxConnectorAndNeedsYouEvents() async {
        let (store, _) = await loadedStore()
        store.apply(.sandboxChanged(.off))
        #expect(!store.sandboxStore.sandbox.isOn)
        store.apply(.needsYouChanged([]))
        #expect(store.needsYouCount == 0)
        store.apply(.connectorsChanged([]))
        #expect(store.connectorStore.connectors.isEmpty)
    }

    @Test func mockEventsDriveTheStore() async {
        let (store, client) = await loadedStore()
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        let before = store.optimizationStore.optimization(id: "opt-checkout")?.spentUSD ?? 0
        for _ in 0..<2 {
            for event in await client.advance() { store.apply(event) }
        }
        let checkout = store.optimizationStore.optimization(id: "opt-checkout")
        #expect((checkout?.spentUSD ?? 0) > before)
        #expect(checkout?.winsCount == 3)
        #expect(store.optimizationStore.details["opt-checkout"]?.candidates.first?.verdict == .verified)
        #expect(store.optimizationStore.details["opt-checkout"]?.moves.count == 3)
        #expect(store.overview?.winsCount == 10)
        #expect(store.overview?.series.contains { $0.win == "Prefetch the shipping rates" } == true)
    }

    @Test func aFinishedRunTearsTheSandboxDown() async {
        let (store, client) = await loadedStore()
        for _ in 0..<100 {
            for event in await client.advance() { store.apply(event) }
        }
        #expect(store.optimizationStore.optimization(id: "opt-checkout")?.status == .done)
        #expect(!store.sandboxStore.sandbox.isOn)
        #expect(store.sandboxStore.receipt?.id == "receipt-opt-checkout")
        #expect(store.needsYou.contains { $0.optimizationID == "opt-checkout" })
    }

    @Test func connectorActionsReload() async {
        let (store, _) = await loadedStore()
        await store.connectorStore.addConnector(NewConnector(kind: .github, name: "acme-labs", detail: "GitHub App · 4 repos"))
        #expect(store.connectorStore.connectors(of: .github).count == 3)
        await store.connectorStore.setDefaultConnector("gh-acme-org")
        #expect(store.connectorStore.connectors(of: .github).filter(\.isDefault).map(\.id) == ["gh-acme-org"])
        await store.connectorStore.removeConnector("gh-acme-org")
        #expect(store.connectorStore.connectors(of: .github).count == 2)
        await store.connectorStore.removeConnector("missing")
        #expect(store.lastError != nil)
    }

    @Test func signingInRefreshesTheConnector() async {
        let (store, _) = await loadedStore()
        await store.connectorStore.signInConnector("aws-acme-staging")
        #expect(store.connectorStore.connectors.first { $0.id == "aws-acme-staging" }?.state == .connected)
    }

    @Test func endingTheSandboxStoresTheReceipt() async {
        let (store, _) = await loadedStore()
        await store.sandboxStore.endSandbox()
        #expect(!store.sandboxStore.sandbox.isOn)
        #expect(store.sandboxStore.receipt?.id.hasPrefix("receipt-end") == true)
    }

    @Test func agentActionsUpdateTheDetail() async {
        let (store, _) = await loadedStore()
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        await store.optimizationStore.pauseAgent("agent-claude-team")
        #expect(store.optimizationStore.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-team" }?.state == .paused)
        await store.optimizationStore.resumeAgent("agent-claude-team")
        #expect(store.optimizationStore.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-team" }?.state == .working)
    }

    @Test func stoppingAndOpeningAPullRequestRefreshTheOptimization() async {
        let (store, _) = await loadedStore()
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        await store.optimizationStore.stopOptimization("opt-checkout")
        #expect(store.optimizationStore.optimization(id: "opt-checkout")?.status == .done)
        #expect(store.optimizationStore.details["opt-checkout"]?.optimization.status == .done)
        let url = await store.optimizationStore.openPullRequest("opt-checkout")
        #expect(url != nil)
        #expect(store.optimizationStore.optimization(id: "opt-checkout")?.prURL == url?.absoluteString)
        #expect(await store.optimizationStore.openPullRequest("opt-bundle") == nil)  // nothing to merge: the error is shown, not thrown
        #expect(store.lastError != nil)
        #expect(await store.optimizationStore.report("opt-cli")?.fileName.hasSuffix(".md") == true)
    }

    @Test func aFailedStopIsShownAndTheNextActionClearsIt() async {
        let (store, _) = await loadedStore()
        await store.optimizationStore.stopOptimization("missing")
        #expect(store.lastError?.hasPrefix("Couldn't stop the optimization.") == true)
        // a background refresh does not wipe what the person has not read yet
        await store.refresh()
        #expect(store.lastError != nil)
        // the banner's close button
        store.dismissError()
        #expect(store.lastError == nil)
        // starting the next action clears it, even when that action goes well
        await store.optimizationStore.stopOptimization("missing")
        #expect(store.lastError != nil)
        await store.optimizationStore.pauseAgent("agent-claude-team")
        #expect(store.lastError == nil)
    }

    @Test func aFailedEndSandboxIsShown() async {
        let (store, _) = await loadedStore()
        await store.sandboxStore.endSandbox()
        #expect(store.lastError == nil)
        await store.sandboxStore.endSandbox()  // already off
        #expect(store.lastError?.hasPrefix("Couldn't end the sandbox.") == true)
        #expect(store.lastError?.contains("already off") == true)
    }

    @Test func aFailedLoadClearsWhenTheNextLoadWorks() async {
        let (store, _) = await loadedStore()
        await store.optimizationStore.loadDetail(id: "missing")
        #expect(store.lastError != nil)
        await store.refresh()
        #expect(store.lastError == nil)
    }

    @Test func theLastSnapshotRendersBeforeTheClientAnswers() async {
        let url = FileManager.default.temporaryDirectory.appending(path: "ratchet-cache-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        let first = AppStore(client: MockRatchetClient(now: testNow), cacheURL: url)
        #expect(first.optimizationStore.optimizations.isEmpty)
        await first.refresh()
        await first.flushCache()
        let second = AppStore(client: MockRatchetClient(now: testNow), cacheURL: url)
        #expect(second.optimizationStore.optimizations.isEmpty)
        await second.restoreCache()
        #expect(!second.isLoaded)
        #expect(second.optimizationStore.optimizations.count == 5)
        #expect(second.overview?.spentUSD == 184)
        #expect(second.connectorStore.connectors.count == 8)
    }

    @Test func flushingTheCacheWritesTheLastRefresh() async throws {
        let url = FileManager.default.temporaryDirectory.appending(path: "ratchet-cache-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        let store = AppStore(client: MockRatchetClient(now: testNow), cacheURL: url)
        for _ in 0..<3 { await store.refresh() }
        await store.flushCache()
        let data = try Data(contentsOf: url)
        let cached = try JSONDecoder().decode(CachedState.self, from: data)
        #expect(cached.optimizations.count == 5)
    }

    @Test func aResyncEventReloadsWhatIsShown() async {
        let (store, client) = await loadedStore()
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        _ = try? await client.addConnector(NewConnector(kind: .github, name: "octo"))
        #expect(store.connectorStore.connectors.count == 8)
        store.apply(.resync)
        for _ in 0..<100 where store.connectorStore.connectors.count == 8 { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(store.connectorStore.connectors.count == 9)
        #expect(store.optimizationStore.details["opt-checkout"] != nil)
    }

    @Test func demoDataIsAFlagOfTheStore() {
        #expect(!AppStore(client: MockRatchetClient(now: testNow)).isDemoData)
        #expect(AppStore(client: MockRatchetClient(now: testNow), isDemoData: true).isDemoData)
    }
}
