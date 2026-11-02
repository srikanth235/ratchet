import Foundation
import Testing
@testable import RatchetKit

@Suite("Event reducer")
struct EventReducerTests {
    private let now = Date(timeIntervalSince1970: 1_793_452_800)

    private func state() -> LiveState {
        let world = MockWorld.standard(now: now)
        return LiveState(
            overview: world.overview(.days90), optimizations: world.optimizations, details: world.details,
            connectors: world.connectors, sandbox: world.sandbox, receipt: world.receipt)
    }

    private func checkout(_ state: LiveState) throws -> Optimization {
        try #require(state.optimizations.first { $0.id == "opt-checkout" })
    }

    // MARK: Optimizations

    @Test func anUpdatedOptimizationMovesTheTotals() throws {
        let before = state()
        var changed = try checkout(before)
        changed.spentUSD += 2
        changed.valuePerYearUSD = (changed.valuePerYearUSD ?? 0) + 600
        changed.winsCount += 1
        let points = before.overview?.series.count ?? 0
        let result = EventReducer.reduce(before, .optimizationUpdated(changed))
        #expect(result.state.overview?.spentUSD == 186)
        #expect(result.state.overview?.valueLockedPerYearUSD == 16_000)
        #expect(result.state.overview?.winsCount == 10)
        #expect(result.state.overview?.series.count == points + 1)
        #expect(result.state.overview?.series.last?.valuePerYearUSD == 16_000)
        #expect(result.state.overview?.series.last?.win == nil)
        #expect(try checkout(result.state).winsCount == 3)
        #expect(result.touched == [.optimizations, .overview, .details])
    }

    @Test func aWinIsNamedAfterTheMoveThatWaitedForIt() throws {
        let before = state()
        let move = RatchetMove(id: "m9", at: now, value: 250, title: "Warm the cache")
        let waiting = EventReducer.reduce(before, .moveAdded(optimizationID: "opt-checkout", move: move)).state
        #expect(waiting.pendingWins["opt-checkout"]?.title == "Warm the cache")
        var changed = try checkout(waiting)
        changed.valuePerYearUSD = (changed.valuePerYearUSD ?? 0) + 450
        changed.winsCount += 1
        let result = EventReducer.reduce(waiting, .optimizationUpdated(changed))
        #expect(result.state.overview?.series.contains { $0.win == "Warm the cache" } == true)
        #expect(result.state.pendingWins["opt-checkout"] == nil)
    }

    @Test func aReplayedMoveDoesNotNameTheNextWin() throws {
        let before = state()
        let old = try #require(before.details["opt-checkout"]?.moves.first)
        let replayed = EventReducer.reduce(before, .moveAdded(optimizationID: "opt-checkout", move: old))
        #expect(replayed.state.pendingWins["opt-checkout"] == nil)
        #expect(replayed.touched.isEmpty)
        // Of two new moves the newer one names it, in whatever order they arrive.
        let early = RatchetMove(id: "m-early", at: now, value: 250, title: "Early")
        let late = RatchetMove(id: "m-late", at: now.addingTimeInterval(60), value: 240, title: "Late")
        var next = EventReducer.reduce(before, .moveAdded(optimizationID: "opt-checkout", move: late)).state
        next = EventReducer.reduce(next, .moveAdded(optimizationID: "opt-checkout", move: early)).state
        #expect(next.pendingWins["opt-checkout"]?.title == "Late")
    }

    @Test func aWinWithoutAMoveIsAVerifiedWin() throws {
        let before = state()
        var changed = try checkout(before)
        changed.valuePerYearUSD = (changed.valuePerYearUSD ?? 0) + 450
        let result = EventReducer.reduce(before, .optimizationUpdated(changed))
        #expect(result.state.overview?.series.contains { $0.win == "Verified win" } == true)
    }

    @Test func aNewOptimizationGoesFirstAndWithoutAnOverviewNothingElseMoves() {
        var empty = LiveState()
        let created = Optimization(
            id: "opt-new", title: "New", repo: "acme/x", branch: "main", goal: "Go", status: .running,
            metric: MetricSpec(name: "p95", unit: "ms"), baseline: 100)
        empty = EventReducer.reduce(empty, .optimizationUpdated(created)).state
        #expect(empty.optimizations.map(\.id) == ["opt-new"])
        #expect(empty.overview == nil)
        let result = EventReducer.reduce(state(), .optimizationUpdated(created))
        #expect(result.state.optimizations.first?.id == "opt-new")
    }

    @Test func registeringAnOptimizationAnnouncesNothing() throws {
        var finished = try checkout(state())
        finished.status = .done
        let result = EventReducer.register(finished, in: state())
        #expect(try checkout(result.state).status == .done)
        #expect(result.notifications.isEmpty)
    }

    // MARK: Details

    @Test func detailEventsUpdateTheLoadedDetailOnly() {
        let loaded = state()
        let count = loaded.details["opt-checkout"]?.candidates.count ?? 0
        let fresh = Candidate(id: "ffff000", title: "New idea", agent: "Claude Code · your login", verdict: .evaluating, at: now)
        var result = EventReducer.reduce(loaded, .candidateUpdated(optimizationID: "opt-checkout", candidate: fresh))
        #expect(result.state.details["opt-checkout"]?.candidates.count == count + 1)
        #expect(result.state.details["opt-checkout"]?.candidates.first?.id == "ffff000")
        #expect(result.touched == [.details])
        var done = fresh
        done.verdict = .noise
        result = EventReducer.reduce(result.state, .candidateUpdated(optimizationID: "opt-checkout", candidate: done))
        #expect(result.state.details["opt-checkout"]?.candidates.count == count + 1)
        #expect(result.state.details["opt-checkout"]?.candidates.first?.verdict == .noise)

        let unloaded = EventReducer.reduce(loaded, .candidateUpdated(optimizationID: "not-loaded", candidate: fresh))
        #expect(unloaded.touched.isEmpty)
    }

    @Test func aMoveIsAddedOnceAndAnAgentIsReplacedOrAdded() {
        let loaded = state()
        let moves = loaded.details["opt-checkout"]?.moves.count ?? 0
        let move = RatchetMove(id: "m9", at: now, value: 250, title: "New idea")
        var result = EventReducer.reduce(loaded, .moveAdded(optimizationID: "opt-checkout", move: move))
        result = EventReducer.reduce(result.state, .moveAdded(optimizationID: "opt-checkout", move: move))
        #expect(result.state.details["opt-checkout"]?.moves.count == moves + 1)

        let paused = Agent(id: "agent-claude-login", name: "Claude Code · your login", placement: .local, state: .paused)
        result = EventReducer.reduce(result.state, .agentUpdated(optimizationID: "opt-checkout", agent: paused))
        #expect(result.state.details["opt-checkout"]?.agents.first { $0.id == paused.id }?.state == .paused)
        let agents = result.state.details["opt-checkout"]?.agents.count ?? 0
        let added = Agent(id: "agent-new", name: "Codex", placement: .local, state: .working)
        result = EventReducer.reduce(result.state, .agentUpdated(optimizationID: "opt-checkout", agent: added))
        #expect(result.state.details["opt-checkout"]?.agents.count == agents + 1)
    }

    // MARK: Everything else

    @Test func sandboxConnectorsFlowAndNeedsYouEachTouchOnlyTheirPart() {
        let loaded = state()
        #expect(EventReducer.reduce(loaded, .sandboxChanged(.off)).touched == [.sandbox])
        #expect(!EventReducer.reduce(loaded, .sandboxChanged(.off)).state.sandbox.isOn)
        #expect(EventReducer.reduce(loaded, .connectorsChanged([])).touched == [.connectors])
        #expect(EventReducer.reduce(loaded, .connectorsChanged([])).state.connectors.isEmpty)
        #expect(EventReducer.reduce(loaded, .githubFlowChanged(nil)).touched == [.githubFlow])
        #expect(EventReducer.reduce(loaded, .needsYouChanged([])).touched == [.overview])
        #expect(EventReducer.reduce(loaded, .needsYouChanged([])).state.overview?.needsYou.isEmpty == true)
        #expect(EventReducer.reduce(LiveState(), .needsYouChanged([])).touched.isEmpty)
        let receipt = TeardownReceipt(id: "r", account: "a", startedAt: now, endedAt: now, costUSD: 1, destroyed: [])
        #expect(EventReducer.reduce(loaded, .receiptIssued(receipt)).state.receipt?.id == "r")
    }

    @Test func aLogLineChangesNothingInTheState() {
        let result = EventReducer.reduce(state(), .agentLogAppended(optimizationID: "opt-checkout", agentID: "agent-claude-login"))
        #expect(result.touched.isEmpty)
        #expect(result.logAppended == "agent-claude-login")
    }

    @Test func resyncAndStreamStateAreEffects() {
        #expect(EventReducer.reduce(state(), .resync).resync)
        let result = EventReducer.reduce(state(), .streamStateChanged(.reconnecting(attempt: 2, reason: "gone")))
        #expect(result.streamState == .reconnecting(attempt: 2, reason: "gone"))
        #expect(result.touched.isEmpty && !result.resync)
    }

    // MARK: Notifications

    @Test func aFinishedRunIsAnnouncedOnceLoaded() throws {
        var finished = try checkout(state())
        finished.status = .done
        let announced = EventReducer.reduce(state(), .optimizationUpdated(finished))
        #expect(announced.notifications.contains { $0.id == "finished-opt-checkout" })
        let first = EventReducer.reduce(state(), .optimizationUpdated(finished), isLoaded: false)
        #expect(first.notifications.isEmpty)
        let switchedOff = EventReducer.reduce(state(), .optimizationUpdated(finished), enabled: [])
        #expect(switchedOff.notifications.isEmpty)
    }

    @Test func aDraftReadyEventIsAnnouncedWhenItsCategoryIsOn() {
        let event = RatchetEvent.environmentDraftReady(draftID: "d1", repoID: "acme/shop")
        let on = EventReducer.reduce(state(), event)
        #expect(on.notifications.map(\.id) == ["draft-d1"])
        #expect(on.touched.isEmpty)
        let off = EventReducer.reduce(state(), event, enabled: [.winReady])
        #expect(off.notifications.isEmpty)
    }
}
