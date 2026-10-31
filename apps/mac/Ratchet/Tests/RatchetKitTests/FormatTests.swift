import Foundation
import Testing
@testable import RatchetKit

@Suite("Formatting")
struct FormatTests {
    @Test("Money, short", arguments: [
        (0.59, "$0.59"), (9.5, "$9.50"), (38.0, "$38"), (184.0, "$184"),
        (4800.0, "$4.8k"), (15_400.0, "$15.4k"), (120_000.0, "$120k"), (-12.0, "−$12"),
    ])
    func moneyShort(value: Double, expected: String) {
        #expect(Money.short(value) == expected)
    }

    @Test func moneyExactAndRate() {
        #expect(Money.exact(1.18) == "$1.18")
        #expect(Money.perHour(0.36) == "$0.36/h")
    }

    @Test("Chart axis", arguments: [(0.0, "$0"), (500.0, "$500"), (5000.0, "$5k"), (12_500.0, "$12.5k")])
    func axis(value: Double, expected: String) {
        #expect(Money.axis(value) == expected)
    }

    @Test("Signed percent", arguments: [
        (-0.301, "−30%"), (0.0588, "+5.9%"), (-0.126, "−13%"), (-0.393, "−39%"), (0.0, "0%"), (-0.0001, "0%"),
    ])
    func signedPercent(fraction: Double, expected: String) {
        #expect(Percent.signed(fraction) == expected)
    }

    @Test func plainAndNoise() {
        #expect(Percent.plain(0.37) == "37%")
        #expect(Percent.plain(0.372, decimals: 1) == "37.2%")
        #expect(Percent.noise(0.021) == "±2.1%")
        #expect(Percent.noise(0.02) == "±2%")
    }

    @Test("Durations", arguments: [(4320.0, "1 h 12 m"), (7200.0, "2 h"), (2700.0, "45 m"), (30.0, "30 s"), (-5.0, "0 s")])
    func durations(seconds: Double, expected: String) {
        #expect(Elapsed.hm(seconds) == expected)
    }

    @Test func daysAndMinutes() {
        #expect(Elapsed.days(4.36) == "4 days")
        #expect(Elapsed.days(1) == "1 day")
        #expect(Elapsed.days(0.2) == "under a day")
        #expect(Elapsed.minutes(5.6) == "6 min")
    }

    @Test func dates() {
        let utc = TimeZone(identifier: "UTC")!
        let date = Date(timeIntervalSince1970: 15 * 3600 + 40 * 60)
        #expect(Elapsed.timeOfDay(date, timeZone: utc) == "15:40")
        #expect(Elapsed.shortDate(date, timeZone: utc) == "Jan 1")
    }

    @Test func figures() {
        #expect(Figure.multiple(83.7) == "84×")
        #expect(Figure.multiple(4.3) == "4.3×")
        #expect(Figure.wins(1) == "1 verified win")
        #expect(Figure.wins(9) == "9 verified wins")
        #expect(Figure.spentOf(38, budget: 50).budget == "50")
        #expect(Figure.spentOf(64, budget: nil).budget == nil)
    }

    @Test func metricLines() {
        let ms = MetricSpec(name: "p95 latency", unit: "ms")
        let ndcg = MetricSpec(name: "relevance", label: "nDCG", decimals: 3, lowerIsBetter: false)
        let wape = MetricSpec(name: "WAPE", label: "WAPE", unit: "%", decimals: 1)
        #expect(ms.pair(from: 412, to: 288) == "412 → 288 ms")
        #expect(ndcg.pair(from: 0.612, to: 0.648) == "nDCG 0.612 → 0.648")
        #expect(wape.pair(from: 18.2, to: 15.9) == "WAPE 18.2 → 15.9%")
        #expect(ms.text(288) == "288 ms")
    }

    @Test func optimizationChange() {
        let ms = MetricSpec(name: "p95 latency", unit: "ms")
        let better = Optimization(id: "a", title: "A", repo: "o/r", status: .done, metric: ms, baseline: 412, best: 288)
        let worse = Optimization(id: "b", title: "B", repo: "o/r", status: .done, metric: ms, baseline: 100, best: 120)
        let none = Optimization(id: "c", title: "C", repo: "o/r", status: .noWin, metric: ms, baseline: 100)
        #expect(better.isImprovement)
        #expect(!worse.isImprovement)
        #expect(none.change == nil)
        #expect(none.resultText == nil)
        #expect(better.resultText == "412 → 288 ms")
        #expect(Percent.signed(better.change ?? 0) == "−30%")
    }
}
