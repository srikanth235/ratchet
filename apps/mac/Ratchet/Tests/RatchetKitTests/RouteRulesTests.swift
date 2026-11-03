import Foundation
import Testing
@testable import RatchetKit

@Suite("Route rules")
struct RouteRulesTests {
    private let start = RouteState(page: "optimizations", optimizationID: "o1", wizardOpen: false)

    @Test func choosingThePageYouAreOnGoesBackToItsList() {
        let next = RouteRules.show("optimizations", from: start)
        #expect(next.page == "optimizations" && next.optimizationID == nil)
    }

    @Test func anotherPageClosesTheOptimizationAndTheFlow() {
        var state = start
        state.wizardOpen = true
        let next = RouteRules.show("connectors", from: state)
        #expect(next == RouteState(page: "connectors", optimizationID: nil, wizardOpen: false))
    }

    @Test func openingAnOptimizationShowsItInPlace() {
        let next = RouteRules.open(optimization: "o2", from: RouteState(page: "overview", wizardOpen: true))
        #expect(next == RouteState(page: "optimizations", optimizationID: "o2", wizardOpen: false))
    }

    @Test func theFlowOpensOnOptimizations() {
        let next = RouteRules.startWizard(from: start)
        #expect(next == RouteState(page: "optimizations", optimizationID: nil, wizardOpen: true))
    }

    @Test func aClickBeforeTheWindowIsReadyWaitsOnce() {
        var pending = PendingDestination()
        #expect(pending.take() == nil && pending.isEmpty)
        pending.hold(.connectors)
        pending.hold(.optimization("o9"))
        #expect(!pending.isEmpty)
        #expect(pending.take() == .optimization("o9"))
        #expect(pending.take() == nil)
    }
}
