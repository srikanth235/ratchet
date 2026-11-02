import Foundation
import Observation
import Testing
@testable import RatchetKit

private final class Raised: @unchecked Sendable {  // test-only: set once from an observation handler, read after
    private let lock = NSLock()
    private var set = false
    var value: Bool { lock.withLock { set } }
    func raise() { lock.withLock { set = true } }
}

private let epoch = Date(timeIntervalSince1970: 1_793_452_800)

@MainActor
@Suite("P5b: what the screens need from the models")
struct P5bTests {
    @Test func pausingTheScheduleClearsItAndResumingPutsItBack() async {
        let model = OptimizationSettingsModel(client: MockHarnessClient(), optimizationID: "o1")
        await model.load()
        #expect(model.canPauseSchedule && !model.isSchedulePaused && model.pauseTitle == "Pause Schedule")
        #expect(await model.pauseSchedule())
        #expect(model.isSchedulePaused && model.schedule?.kind == nil && model.pauseTitle == "Resume Schedule" && model.pausedSchedule == .nightly)
        #expect(await model.togglePause())
        #expect(!model.isSchedulePaused && model.schedule?.kind == .nightly && model.pausedSchedule == nil)
    }

    @Test func aScheduleThatWasNeverSetCannotBePaused() async {
        let model = OptimizationSettingsModel(client: MockHarnessClient(), optimizationID: "o2")
        await model.load()
        _ = await model.saveSchedule(nil)
        #expect(!model.canPauseSchedule)
        #expect(!(await model.pauseSchedule()))
    }

    @Test func aConversationViewIsRedrawnWhenAMessageArrivesButNotForADelta() async throws {
        let mock = MockChatClient(now: epoch)
        let store = ChatStore(client: mock, coalescing: .zero, pause: { _ in })
        let id = try #require(await store.start(NewChat(harness: .claudeCode)))
        let model = ConversationModel(store: store, chatID: id)
        let session = try #require(store.session(id))

        let flag = Raised()
        withObservationTracking { _ = model.items } onChange: { flag.raise() }
        session.apply([ChatFrame(id: 1, event: .message(ChatMessage(seq: 1, role: .user, text: "hello", at: epoch)))])
        #expect(flag.value)

        let delta = Raised()
        withObservationTracking { _ = model.items } onChange: { delta.raise() }
        session.apply([ChatFrame(id: 2, event: .delta("Noted"))])
        #expect(!delta.value)
    }

    @Test func theChartInWordsNamesTheStepsAndWhereItEnded() {
        let story = ValueStory(
            delivered: ValueAmount(usdPerYear: 12_500, wins: 3), pending: ValueAmount(usdPerYear: 2_900, wins: 1, pullRequests: 1),
            spentUSD: 184, returnMultiple: 68, line: "$12.5k delivered + $2.9k pending · 68×")
        let day = 86_400.0
        let overview = ValueOverview(
            range: .days90, story: story, spent: SpendDetail(usd: 184), forecast: ValueForecast(),
            chart: [
                ValueChartPoint(at: epoch, spentUSD: 0, deliveredUSDPerYear: 0),
                ValueChartPoint(at: epoch.addingTimeInterval(day * 30), spentUSD: 60, deliveredUSDPerYear: 2_400),
                ValueChartPoint(at: epoch.addingTimeInterval(day * 60), spentUSD: 120, deliveredUSDPerYear: 12_500, pendingUSDPerYear: 0),
                ValueChartPoint(at: epoch.addingTimeInterval(day * 90), spentUSD: 184, deliveredUSDPerYear: 12_500, pendingUSDPerYear: 2_900),
            ])
        let text = overview.chartSummary
        #expect(text.hasPrefix("2 steps: +$2.4k on "))
        #expect(text.contains("+$10.1k on ") && text.hasSuffix("Now $12.5k delivered, $2.9k pending, $184 spent."))
        let empty = ValueOverview(range: .days90, phase: .empty, story: ValueStory(), spent: SpendDetail(), forecast: ValueForecast(), chart: [])
        #expect(empty.chartSummary == "No chart yet.")
    }

    @Test func nothingWaitsOnAPersonAnyMore() {
        #expect(OptimizationStatus.waiting.title == "Not started")
        #expect(OptimizationStatus.allCases.allSatisfy { !$0.title.lowercased().contains("waiting on you") })
    }

    @Test func theCreationConversationTakesHomesPlaceInTheRoute() {
        let from = RouteState(page: "optimizations", optimizationID: "o1")
        let next = RouteRules.startConversation(from: from)
        #expect(next.page == "home" && next.optimizationID == nil && next.wizardOpen)
        #expect(!RouteRules.show("home", from: next).wizardOpen)
    }
}
