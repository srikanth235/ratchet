import Foundation
import Testing
@testable import RatchetKit

let testNow = Date(timeIntervalSince1970: 1_793_000_000)

func isCandidateUpdate(_ event: RatchetEvent) -> Bool {
    if case .candidateUpdated = event { return true }
    return false
}

func isMoveAdded(_ event: RatchetEvent) -> Bool {
    if case .moveAdded = event { return true }
    return false
}

@Suite("Mock client")
struct MockClientTests {
    @Test func overviewMatchesTheMock() async throws {
        let client = MockRatchetClient(now: testNow)
        let overview = try await client.overview(range: .days90)
        #expect(overview.spentUSD == 184)
        #expect(overview.valueLockedPerYearUSD == 15_400)
        #expect(overview.winsCount == 9)
        #expect(Figure.multiple(overview.returnMultiple ?? 0) == "84×")
        #expect(Elapsed.days(overview.paybackDays ?? 0) == "4 days")
        #expect(overview.series.first?.win == nil)
        #expect(overview.series.last?.win == nil)
        #expect(overview.series.last?.valuePerYearUSD == 15_400)
        #expect(overview.needsYou.map(\.kind) == [.approveEnvironment, .mergeWins])
    }

    @Test func rangesNarrowTheOverview() async throws {
        let client = MockRatchetClient(now: testNow)
        let month = try await client.overview(range: .days30)
        #expect(month.winsCount == 3)
        #expect(month.spentUSD == 79)
        #expect(month.series.first?.valuePerYearUSD == 10_600)
        let all = try await client.overview(range: .all)
        #expect(all.winsCount == 9)
    }

    @Test func optimizationsCoverEveryStatus() async throws {
        let client = MockRatchetClient(now: testNow)
        let all = try await client.optimizations()
        #expect(Set(all.map(\.status)) == Set(OptimizationStatus.allCases))
        let detail = try await client.optimization(id: "opt-checkout")
        #expect(detail.agents.count == 2)
        #expect(detail.moves.count == 2)
        let candidates = try await client.candidates(optimizationID: "opt-checkout")
        #expect(candidates.contains { $0.verdict == .evaluating })
        await #expect(throws: RatchetClientError.self) { try await client.optimization(id: "nope") }
    }

    @Test func spendAddsUp() async throws {
        let client = MockRatchetClient(now: testNow)
        let spend = try await client.spend(range: .days90)
        #expect(spend.lines.reduce(0) { $0 + $1.usd } == spend.totalUSD)
    }

    @Test func agentsPauseResumeAndListen() async throws {
        let client = MockRatchetClient(now: testNow)
        let paused = try await client.pauseAgent(id: "agent-claude-login")
        #expect(paused.state == .paused)
        let resumed = try await client.resumeAgent(id: "agent-claude-login")
        #expect(resumed.state == .working)
        try await client.messageAgent(id: "agent-claude-login", text: "Try the cache first")
        let agents = try await client.agents(optimizationID: "opt-checkout")
        let login = agents.first { $0.id == "agent-claude-login" }
        #expect(login?.activity.contains("Try the cache first") == true)
        await #expect(throws: RatchetClientError.self) { try await client.messageAgent(id: "agent-claude-login", text: "  ") }
        await #expect(throws: RatchetClientError.self) { try await client.pauseAgent(id: "nope") }
    }

    @Test func connectorsAddDefaultRemove() async throws {
        let client = MockRatchetClient(now: testNow)
        let before = try await client.connectors()
        #expect(before.filter { $0.kind == .aws }.count == 3)
        #expect(before.first { $0.id == "aws-acme-staging" }?.state == .expired)
        let added = try await client.addConnector(NewConnector(kind: .aws, name: "sandbox-eu", detail: "SSO · eu-central-1"))
        #expect(added.state == .connected)
        let list = try await client.setDefaultConnector(id: added.id)
        #expect(list.filter { $0.kind == .aws && $0.isDefault }.map(\.id) == [added.id])
        try await client.removeConnector(id: added.id)
        let after = try await client.connectors()
        #expect(after.filter { $0.kind == .aws && $0.isDefault }.count == 1)
        await #expect(throws: RatchetClientError.self) { try await client.addConnector(NewConnector(kind: .github, name: " ")) }
    }

    @Test func selectionsAreRememberedPerRepo() async throws {
        let client = MockRatchetClient(now: testNow)
        let known = try await client.connectionSelection(repoID: "acme-org/shop-api")
        #expect(known?.awsID == "aws-acme-prod")
        let unknown = try await client.connectionSelection(repoID: "acme-org/web-app")
        #expect(unknown == nil)
        try await client.saveConnectionSelection(ConnectionSelection(repoID: "acme-org/web-app", awsID: "aws-personal-dev"))
        let saved = try await client.connectionSelection(repoID: "acme-org/web-app")
        #expect(saved?.awsID == "aws-personal-dev")
    }

    @Test func environmentDraftsReviseAndApprove() async throws {
        let client = MockRatchetClient(now: testNow)
        let draft = try await client.createDraft(DraftRequest(repoID: "acme-org/shop-api", goal: "Make checkout p95 faster"))
        #expect(draft.scope == .full)
        #expect(draft.resources.count == 4)
        #expect(abs(draft.usdPerHour - 0.59) < 0.0001)
        #expect(abs(draft.cost(forHours: 2) - 1.18) < 0.0001)
        let rds = try await client.reviseDraft(id: draft.id, instruction: "use RDS instead of a Postgres container")
        #expect(rds.resources.contains { $0.machine == "db.r6g.large" })
        #expect(rds.revision == 2)
        let small = try await client.reviseDraft(id: draft.id, instruction: "Use a smaller setup")
        #expect(small.scope == .single)
        #expect(small.resources.count == 1)
        let approved = try await client.approveDraft(id: draft.id)
        #expect(approved.state == .approved)
        let cli = try await client.createDraft(DraftRequest(repoID: "srikanth235/acme-cli", goal: "Faster start"))
        #expect(cli.scope == .single)
    }

    @Test func sandboxEndsOnceAndLeavesAReceipt() async throws {
        let client = MockRatchetClient(now: testNow)
        let before = try await client.sandboxStatus()
        #expect(before.isOn)
        let receipt = try await client.endSandbox()
        #expect(receipt.allGone)
        #expect(receipt.costUSD > 0)
        let after = try await client.sandboxStatus()
        #expect(!after.isOn)
        let last = try await client.sandboxReceipt()
        #expect(last?.id == receipt.id)
        await #expect(throws: RatchetClientError.self) { try await client.endSandbox() }
    }

    @Test func startingAnOptimizationAddsARunningOne() async throws {
        let client = MockRatchetClient(now: testNow)
        let selection = ConnectionSelection(repoID: "acme-org/web-app", awsID: "aws-acme-prod", agents: ["agent-claude-login": .local])
        let started = try await client.startOptimization(StartRequest(
            repoID: "acme-org/web-app", branch: "main", goal: "Shrink the bundle", selection: selection, budgetUSD: 50, timeLimitHours: 2))
        #expect(started.status == .running)
        #expect(started.budgetUSD == 50)
        #expect(started.secondsLeft == 7200)
        let all = try await client.optimizations()
        #expect(all.first?.id == started.id)
        await #expect(throws: RatchetClientError.self) {
            try await client.startOptimization(StartRequest(
                repoID: "acme-org/web-app", branch: "main", goal: " ", selection: selection, budgetUSD: 50, timeLimitHours: 2))
        }
    }

    @Test func advancingMakesProgress() async throws {
        let client = MockRatchetClient(now: testNow)
        let first = await client.advance()
        #expect(first.contains(where: isCandidateUpdate))
        let second = await client.advance()
        #expect(second.contains(where: isMoveAdded))
        let detail = try await client.optimization(id: "opt-checkout")
        #expect(detail.optimization.winsCount == 3)
        #expect((detail.optimization.best ?? 999) < 288)
        #expect(detail.optimization.spentUSD > 38)
    }

    @Test func aRunEndsWithATeardown() async throws {
        let client = MockRatchetClient(now: testNow)
        for _ in 0..<100 { await client.advance() }
        let detail = try await client.optimization(id: "opt-checkout")
        #expect(detail.optimization.status == .done)
        #expect(detail.optimization.prNumber == 215)
        #expect(detail.receipt != nil)
        let sandbox = try await client.sandboxStatus()
        #expect(!sandbox.isOn)
        let overview = try await client.overview(range: .days90)
        #expect(overview.needsYou.contains { $0.optimizationID == "opt-checkout" })
    }

    @Test func eventsReachSubscribers() async throws {
        let client = MockRatchetClient(now: testNow)
        let stream = await client.events()
        await client.advance()
        var iterator = stream.makeAsyncIterator()
        let first = await iterator.next()
        #expect(first != nil)
    }
}
