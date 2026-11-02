import Foundation
import Testing
@testable import RatchetKit

@Suite("Menu bar rules")
struct MenuBarRulesTests {
    private func optimization(left: Double?, limitHours: Double = 2, spent: Double = 12) -> Optimization {
        Optimization(
            id: "o1", title: "Checkout p95", repo: "acme/shop", status: .running, metric: MetricSpec(name: "p95", unit: "ms"),
            baseline: 300, spentUSD: spent, secondsLeft: left, timeLimitHours: limitHours)
    }

    @Test func theBarFillsAsTimeRunsDown() {
        #expect(MenuBarRules.timeFraction(optimization(left: 3600)) == 0.5)
        #expect(MenuBarRules.timeFraction(optimization(left: 7200)) == 0)
        #expect(MenuBarRules.timeFraction(optimization(left: 0)) == 1)
    }

    @Test func theBarStaysInsideItsTrack() {
        #expect(MenuBarRules.timeFraction(optimization(left: 99_999)) == 0)
        #expect(MenuBarRules.timeFraction(optimization(left: -5)) == 1)
        #expect(MenuBarRules.timeFraction(optimization(left: nil)) == 0)
        #expect(MenuBarRules.timeFraction(optimization(left: 100, limitHours: 0)) == 0)
    }

    @Test func theNoteSaysTimeLeftAndMoneySpent() {
        #expect(MenuBarRules.runNote(optimization(left: 4800)) == Elapsed.hm(4800) + " left · " + Money.short(12) + " spent")
        #expect(MenuBarRules.runNote(optimization(left: nil)) == Money.short(12) + " spent")
    }

    @Test func theSandboxLineNamesTheAccountAndTheCost() {
        #expect(MenuBarRules.sandboxLine(.off) == "Sandbox off")
        let line = MenuBarRules.sandboxLine(SandboxStatus(isOn: true, account: "acme-prod", costSoFarUSD: 1.5))
        #expect(line == "Sandbox on · acme-prod · " + Money.exact(1.5) + " so far")
        #expect(MenuBarRules.sandboxLine(SandboxStatus(isOn: true)).hasPrefix("Sandbox on · AWS · "))
        let timed = MenuBarRules.sandboxLine(SandboxStatus(isOn: true, account: "a", offAt: Date(timeIntervalSince1970: 0)))
        #expect(timed.contains(" · off at "))
    }
}
