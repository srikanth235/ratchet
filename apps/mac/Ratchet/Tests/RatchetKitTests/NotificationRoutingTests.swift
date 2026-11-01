import Foundation
import Testing
@testable import RatchetKit

@Suite("Notification routing")
struct NotificationRoutingTests {
    private func optimization(
        _ id: String, status: OptimizationStatus, spent: Double = 10, budget: Double? = 50, wins: Int = 0
    ) -> Optimization {
        Optimization(
            id: id, title: "Checkout p95", repo: "acme/shop", status: status, metric: MetricSpec(name: "p95", unit: "ms"),
            baseline: 300, spentUSD: spent, budgetUSD: budget, winsCount: wins)
    }

    private func state(
        _ optimizations: [Optimization] = [], needs: [NeedsYouItem] = [], connectors: [Connector] = []
    ) -> ObservedState {
        ObservedState(optimizations: optimizations, needsYou: needs, connectors: connectors)
    }

    private func need(_ id: String, _ kind: NeedsYouKind, optimization: String = "o1") -> NeedsYouItem {
        NeedsYouItem(id: id, kind: kind, title: "Title \(id)", detail: "Detail \(id)", actionTitle: "Open", optimizationID: optimization)
    }

    @Test func aNewWinToMergeNotifiesOnceAndOpensTheOptimization() throws {
        let old = state()
        let new = state(needs: [need("n1", .mergeWins)])
        let found = NotificationRouting.changes(from: old, to: new)
        #expect(found.count == 1)
        let first = try #require(found.first)
        #expect(first.category == .winReady)
        #expect(first.destination == .optimization("o1"))
        #expect(first.id == "needs-n1")
        // The same item again is not new.
        #expect(NotificationRouting.changes(from: new, to: new).isEmpty)
    }

    @Test func eachNeedsYouKindHasItsCategoryAndPlace() {
        let kinds: [(NeedsYouKind, NotificationCategory, NotificationDestination)] = [
            (.mergeWins, .winReady, .optimization("o1")),
            (.approveEnvironment, .environmentDrafted, .newOptimization),
            (.budgetLimit, .budgetLimit, .optimization("o1")),
            (.runFailed, .runFinished, .optimization("o1")),
            (.signIn, .connectorExpired, .connectors),
        ]
        for (kind, category, destination) in kinds {
            let found = NotificationRouting.changes(from: state(), to: state(needs: [need("x", kind)]))
            #expect(found.first?.category == category, "\(kind)")
            #expect(found.first?.destination == destination, "\(kind)")
        }
    }

    @Test func aRunThatFinishesNotifiesWithItsWins() throws {
        let old = state([optimization("o1", status: .running)])
        let new = state([optimization("o1", status: .done, wins: 3)])
        let found = NotificationRouting.changes(from: old, to: new)
        let note = try #require(found.first)
        #expect(found.count == 1)
        #expect(note.category == .runFinished)
        #expect(note.body == "3 verified wins.")
        #expect(note.destination == .optimization("o1"))
        let none = NotificationRouting.changes(from: old, to: state([optimization("o1", status: .noWin)]))
        #expect(none.first?.body == "No win this time.")
    }

    @Test func aRunThatSpentItsBudgetIsABudgetNotificationNotAFinishedOne() {
        let old = state([optimization("o1", status: .running, spent: 49, budget: 50)])
        let new = state([optimization("o1", status: .done, spent: 50, budget: 50)])
        let found = NotificationRouting.changes(from: old, to: new)
        #expect(found.map(\.category) == [.budgetLimit])
    }

    @Test func aFailedRunIsNotAlsoReportedAsFinished() {
        let old = state([optimization("o1", status: .running)])
        let new = state([optimization("o1", status: .done)], needs: [need("f", .runFailed)])
        let found = NotificationRouting.changes(from: old, to: new)
        #expect(found.map(\.category) == [.runFinished])
        #expect(found.first?.id == "needs-f")
    }

    @Test func anOptimizationThatWasAlreadyFinishedIsQuiet() {
        let done = optimization("o1", status: .done)
        #expect(NotificationRouting.changes(from: state([done]), to: state([done])).isEmpty)
        // One that appears already finished (first load) is not news either.
        #expect(NotificationRouting.changes(from: state(), to: state([done])).isEmpty)
    }

    @Test func aConnectorThatExpiresNotifiesOnTheTransitionOnly() throws {
        let good = Connector(id: "aws1", kind: .aws, name: "acme-prod", detail: "", state: .valid)
        var expired = good
        expired.state = .expired
        let found = NotificationRouting.changes(from: state(connectors: [good]), to: state(connectors: [expired]))
        let note = try #require(found.first)
        #expect(note.category == .connectorExpired)
        #expect(note.destination == .connectors)
        #expect(NotificationRouting.changes(from: state(connectors: [expired]), to: state(connectors: [expired])).isEmpty)
        // A connector that was already expired at the first load is not news.
        #expect(NotificationRouting.changes(from: state(), to: state(connectors: [expired])).isEmpty)
    }

    @Test func switchedOffCategoriesAreDropped() {
        let new = state(needs: [need("n1", .mergeWins), need("n2", .signIn)])
        let found = NotificationRouting.changes(from: state(), to: new, enabled: [.connectorExpired])
        #expect(found.map(\.category) == [.connectorExpired])
        #expect(NotificationRouting.changes(from: state(), to: new, enabled: []).isEmpty)
    }

    @Test func aDraftedEnvironmentNotifiesWhenEnabled() {
        let note = NotificationRouting.environmentDrafted(draftID: "d1", repoID: "acme/shop")
        #expect(note?.category == .environmentDrafted)
        #expect(note?.destination == .newOptimization)
        #expect(note?.id == "draft-d1")
        #expect(NotificationRouting.environmentDrafted(draftID: "d1", repoID: "acme/shop", enabled: [.winReady]) == nil)
    }

    @Test func destinationsSurviveTheNotificationsUserInfo() {
        let all: [NotificationDestination] = [.optimization("opt:with:colons"), .connectors, .newOptimization, .overview]
        for destination in all { #expect(NotificationDestination(token: destination.token) == destination) }
        #expect(NotificationDestination(token: "nonsense") == nil)
    }

    @Test func everyCategoryHasWordsAndItsOwnSwitch() {
        #expect(Set(NotificationCategory.allCases.map(\.defaultsKey)).count == NotificationCategory.allCases.count)
        for category in NotificationCategory.allCases {
            #expect(!category.title.isEmpty && !category.explanation.isEmpty)
        }
    }
}
