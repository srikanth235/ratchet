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
        #expect(store.optimizations.count == 5)
        #expect(store.connectors.count == 8)
        #expect(store.templates.count == 10)
        #expect(store.benchmarks.count == 5)
        #expect(store.repos.count == 6)
        #expect(store.sandbox.isOn)
        #expect(store.receipt != nil)
        #expect(store.overview?.spentUSD == 184)
        #expect(store.needsYouCount == 2)
        #expect(store.runningOptimizations.map(\.id) == ["opt-checkout"])
    }

    @Test func filtersSplitRunningAndDone() async {
        let (store, _) = await loadedStore()
        #expect(store.visibleOptimizations(.all).count == 5)
        #expect(store.visibleOptimizations(.running).map(\.id) == ["opt-checkout", "opt-search"])
        #expect(store.visibleOptimizations(.done).count == 3)
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
        guard var checkout = store.optimization(id: "opt-checkout") else {
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
        #expect(store.optimization(id: "opt-checkout")?.winsCount == 3)
    }

    @Test func detailEventsUpdateTheLoadedDetail() async {
        let (store, _) = await loadedStore()
        await store.loadDetail(id: "opt-checkout")
        let candidates = store.details["opt-checkout"]?.candidates.count ?? 0
        let fresh = Candidate(id: "ffff000", title: "New idea", agent: "Claude Code · your login", verdict: .evaluating, at: testNow)
        store.apply(.candidateUpdated(optimizationID: "opt-checkout", candidate: fresh))
        #expect(store.details["opt-checkout"]?.candidates.count == candidates + 1)
        var done = fresh
        done.verdict = .noise
        store.apply(.candidateUpdated(optimizationID: "opt-checkout", candidate: done))
        #expect(store.details["opt-checkout"]?.candidates.count == candidates + 1)
        #expect(store.details["opt-checkout"]?.candidates.first?.verdict == .noise)

        let move = RatchetMove(id: "m9", at: testNow, value: 250, title: "New idea")
        store.apply(.moveAdded(optimizationID: "opt-checkout", move: move))
        store.apply(.moveAdded(optimizationID: "opt-checkout", move: move))
        #expect(store.details["opt-checkout"]?.moves.count == 3)

        let agent = Agent(id: "agent-claude-login", name: "Claude Code · your login", placement: .local, state: .paused)
        store.apply(.agentUpdated(optimizationID: "opt-checkout", agent: agent))
        #expect(store.details["opt-checkout"]?.agents.first { $0.id == agent.id }?.state == .paused)
    }

    @Test func sandboxConnectorAndNeedsYouEvents() async {
        let (store, _) = await loadedStore()
        store.apply(.sandboxChanged(.off))
        #expect(!store.sandbox.isOn)
        store.apply(.needsYouChanged([]))
        #expect(store.needsYouCount == 0)
        store.apply(.connectorsChanged([]))
        #expect(store.connectors.isEmpty)
    }

    @Test func mockEventsDriveTheStore() async {
        let (store, client) = await loadedStore()
        await store.loadDetail(id: "opt-checkout")
        let before = store.optimization(id: "opt-checkout")?.spentUSD ?? 0
        for _ in 0..<2 {
            for event in await client.advance() { store.apply(event) }
        }
        let checkout = store.optimization(id: "opt-checkout")
        #expect((checkout?.spentUSD ?? 0) > before)
        #expect(checkout?.winsCount == 3)
        #expect(store.details["opt-checkout"]?.candidates.first?.verdict == .verified)
        #expect(store.details["opt-checkout"]?.moves.count == 3)
        #expect(store.overview?.winsCount == 10)
        #expect(store.overview?.series.contains { $0.win == "Prefetch the shipping rates" } == true)
    }

    @Test func aFinishedRunTearsTheSandboxDown() async {
        let (store, client) = await loadedStore()
        for _ in 0..<100 {
            for event in await client.advance() { store.apply(event) }
        }
        #expect(store.optimization(id: "opt-checkout")?.status == .done)
        #expect(!store.sandbox.isOn)
        #expect(store.receipt?.id == "receipt-opt-checkout")
        #expect(store.needsYou.contains { $0.optimizationID == "opt-checkout" })
    }

    @Test func connectorActionsReload() async {
        let (store, _) = await loadedStore()
        await store.addConnector(NewConnector(kind: .github, name: "acme-labs", detail: "GitHub App · 4 repos"))
        #expect(store.connectors(of: .github).count == 3)
        await store.setDefaultConnector("gh-acme-org")
        #expect(store.connectors(of: .github).filter(\.isDefault).map(\.id) == ["gh-acme-org"])
        await store.removeConnector("gh-acme-org")
        #expect(store.connectors(of: .github).count == 2)
        await store.removeConnector("missing")
        #expect(store.lastError != nil)
    }

    @Test func signingInRefreshesTheConnector() async {
        let (store, _) = await loadedStore()
        await store.signInConnector("aws-acme-staging")
        #expect(store.connectors.first { $0.id == "aws-acme-staging" }?.state == .connected)
    }

    @Test func endingTheSandboxStoresTheReceipt() async {
        let (store, _) = await loadedStore()
        await store.endSandbox()
        #expect(!store.sandbox.isOn)
        #expect(store.receipt?.id.hasPrefix("receipt-end") == true)
    }

    @Test func agentActionsUpdateTheDetail() async {
        let (store, _) = await loadedStore()
        await store.loadDetail(id: "opt-checkout")
        await store.pauseAgent("agent-claude-team")
        #expect(store.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-team" }?.state == .paused)
        await store.resumeAgent("agent-claude-team")
        #expect(store.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-team" }?.state == .working)
    }

    @Test func theLastSnapshotRendersBeforeTheClientAnswers() async {
        let url = FileManager.default.temporaryDirectory.appending(path: "ratchet-cache-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        let first = AppStore(client: MockRatchetClient(now: testNow), cacheURL: url)
        #expect(first.optimizations.isEmpty)
        await first.refresh()
        let second = AppStore(client: MockRatchetClient(now: testNow), cacheURL: url)
        #expect(!second.isLoaded)
        #expect(second.optimizations.count == 5)
        #expect(second.overview?.spentUSD == 184)
        #expect(second.connectors.count == 8)
    }
}
