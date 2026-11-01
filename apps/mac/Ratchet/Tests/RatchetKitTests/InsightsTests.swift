import Foundation
import RatchetAPI
import Testing
@testable import RatchetKit

// The assertions of the Electron renderer's tests for the live log (`LiveLog streaming`), the Result screen (spend breakdown,
// where the time went, before/after rows, the winning change), the Benchmarks screen (metric switch, candidate filters) and
// the chart helpers (`histogramBins`, bar heights), as model, store and mapping tests.

private func line(_ id: Int64, _ text: String, kind: String = "message") -> AgentLogLine {
    AgentLogLine(id: id, at: Date(timeIntervalSince1970: Double(id)), kind: kind, text: text)
}

@Suite("Agent log")
struct AgentLogTests {
    @Test func theNewestPageComesFirstThenOnlyWhatFollowsTheCursor() {
        var log = AgentLog()
        #expect(!log.isLoaded)
        log.apply(AgentLogPage(lines: [line(1, "online", kind: "status"), line(2, "thinking")], cursor: 2))
        #expect(log.isLoaded && log.cursor == 2)
        #expect(log.lines.map(\.text) == ["online", "thinking"])
        log.apply(AgentLogPage(lines: [line(3, "verified: ok", kind: "verdict")], cursor: 3))
        #expect(log.lines.map(\.text) == ["online", "thinking", "verified: ok"])
        #expect(log.cursor == 3)
        // An empty page keeps the cursor.
        log.apply(AgentLogPage(lines: [], cursor: 3))
        #expect(log.cursor == 3 && log.lines.count == 3)
    }

    @Test func aPageReadTwiceAddsNothing() {
        var log = AgentLog()
        let page = AgentLogPage(lines: [line(1, "a"), line(2, "b")], cursor: 2)
        log.apply(page)
        log.apply(page)
        #expect(log.lines.map(\.id) == [1, 2])
    }

    @Test func onlyTheNewestThreeHundredLinesAreKept() {
        var log = AgentLog()
        log.apply(AgentLogPage(lines: (1...250).map { line(Int64($0), "line \($0)") }, cursor: 250))
        log.apply(AgentLogPage(lines: (251...400).map { line(Int64($0), "line \($0)") }, cursor: 400))
        #expect(log.lines.count == AgentLog.capacity)
        #expect(log.lines.first?.id == 101 && log.lines.last?.id == 400)
    }

    @Test func anErrorIsShownAndTheNextPageClearsIt() {
        var log = AgentLog()
        log.fail("gone")
        #expect(log.error == "gone" && log.isLoaded && log.lines.isEmpty)
        log.apply(AgentLogPage(lines: [line(1, "back")], cursor: 1))
        #expect(log.error == nil)
    }

    @Test func theDaemonsPageMapsToLines() throws {
        let json = #"{"events":[{"id":7,"at":1793000000000,"type":"status","text":"online"},{"id":8,"at":1793000060000,"type":"message","text":"thinking"}],"cursor":8}"#
        let page = try DaemonMapping.agentLogPage(Data(json.utf8))
        #expect(page.cursor == 8)
        #expect(page.lines.map(\.kind) == ["status", "message"])
        #expect(page.lines[0].at == Date(timeIntervalSince1970: 1_793_000_000))
    }

    @MainActor
    @Test func theStoreLoadsTheLogAndAppendsWhatTheEventStreamAnnounces() async throws {
        let mock = MockRatchetClient(now: testNow)
        let store = AppStore(client: mock)
        await store.refresh()
        await store.loadDetail(id: "opt-checkout")
        // An event for a log nobody opened changes nothing.
        store.apply(.agentLogAppended(optimizationID: "opt-checkout", agentID: "agent-claude-team"))
        #expect(store.insight.logs["agent-claude-team"] == nil)

        await store.loadAgentLog("agent-claude-team")
        let first = try #require(store.insight.logs["agent-claude-team"])
        #expect(first.isLoaded && first.error == nil && first.lines.count > 2)
        await store.loadAgentLog("agent-claude-team")
        #expect(store.insight.logs["agent-claude-team"]?.lines.count == first.lines.count, "nothing new, nothing added")

        _ = await mock.advance()
        _ = await mock.advance()
        await store.loadAgentLog("agent-claude-team")
        let later = try #require(store.insight.logs["agent-claude-team"])
        #expect(later.lines.count > first.lines.count)
        #expect(later.lines.map(\.id) == later.lines.map(\.id).sorted())
    }

    @MainActor
    @Test func anUnknownAgentShowsItsErrorInTheLog() async {
        let store = AppStore(client: MockRatchetClient(now: testNow))
        await store.loadAgentLog("nobody")
        #expect(store.insight.logs["nobody"]?.error != nil)
    }
}

@Suite("Result details")
struct ResultDetailTests {
    @Test func theBiggestDropIsTheSpanThatLostTheMostTime() {
        let spans = [
            SpanTiming(name: "parse", baselineMS: 38, bestMS: 31),
            SpanTiming(name: "search", baselineMS: 214, bestMS: 118),
            SpanTiming(name: "wait", baselineMS: 64, bestMS: 66),
        ]
        #expect(SpanTiming.biggestDrop(in: spans)?.name == "search")
        #expect(SpanTiming.longest(in: spans) == 214)
        #expect(SpanTiming.biggestDrop(in: [SpanTiming(name: "wait", baselineMS: 10, bestMS: 12)]) == nil)
    }

    @Test func compareRowsReadLikeTheOldOnes() {
        let p95 = CompareLine(key: "p95", label: "p95", unit: "ms", baseline: 182, best: 131, deltaPercent: -28.02, lowerIsBetter: true)
        #expect(p95.text(p95.baseline) + "→" + p95.text(p95.best) + p95.deltaText() == "182 ms→131 ms−28%")
        #expect(!p95.isWorse)
        let memory = CompareLine(key: "peak_rss_mb", label: "peak rss", unit: "MB", baseline: 512, best: 538, deltaPercent: 5.08, lowerIsBetter: true)
        #expect(memory.deltaText() == "+5.1%" && memory.isWorse)
        // A guardrail is a metric outside the objective's family.
        #expect(memory.isGuardrail(family: ["p50", "p95", "p99"]) && !p95.isGuardrail(family: ["p50", "p95", "p99"]))
        // Less than a whole percent is not "worse".
        let flat = CompareLine(key: "error_rate", label: "error rate", unit: "%", baseline: 0.1, best: 0.1, deltaPercent: 0.3, lowerIsBetter: true)
        #expect(!flat.isWorse)
        // A higher-is-better metric that fell is worse.
        let score = CompareLine(key: "score", label: "score", unit: "", baseline: 0.6, best: 0.5, deltaPercent: -16.7, lowerIsBetter: false)
        #expect(score.isWorse)
        #expect(CompareLine(key: "k", label: "k", unit: "", baseline: nil, best: nil, deltaPercent: nil, lowerIsBetter: true).deltaText() == "—")
        #expect(p95.deltaText(decimals: 0) == "−28%")
    }

    @Test func theWinningChangeNamesItsCommitsAndShowsTheExcerpt() {
        let change = WinningChange(
            commits: 5, sha: "a91f3c2", baseRef: "main",
            files: [DiffFile(path: "src/search.ts", additions: 40, deletions: 2, binary: false), DiffFile(path: "logo.png", additions: 0, deletions: 0, binary: true)],
            additions: 40, deletions: 2, excerptPath: "src/search.ts",
            excerpt: [DiffLine(kind: .hunk, text: "@@ -1,3 +1,4 @@ function search()"), DiffLine(kind: .del, text: "old"), DiffLine(kind: .add, text: "new"), DiffLine(kind: .ctx, text: "tail")])
        #expect(change.headline == "5 verified commits, squashed · a91f3c2 vs main")
        #expect(change.statLine == "2 files · +40 −2")
        #expect(change.excerpt.map { $0.rendered(path: "src/search.ts") } == ["@@ -1,3 +1,4 @@ src/search.ts", "- old", "+ new", "  tail"])
        #expect(WinningChange(commits: 0, sha: nil, baseRef: "main", unavailable: "No verified win yet.").headline == "nothing to squash yet")
        #expect(WinningChange(commits: 1, sha: nil, baseRef: "main").headline == "1 verified commit, squashed")
    }

    @Test func spendIsSplitByCategoryAndAgentWithTheCostOfAWin() {
        let spend = SpendBreakdown(
            totalUSD: 12, estimatedUSD: 2.5, budgetUSD: 50,
            categories: [SpendLine(id: "tokens", label: "Model tokens", usd: 8), SpendLine(id: "bench", label: "Bench runner", usd: 4)],
            agents: [
                AgentSpend(id: "a", agentID: "a", name: "Claude Code", usd: 6, wins: 2),
                AgentSpend(id: "b", agentID: "b", name: "Codex", usd: 3, wins: 0),
                AgentSpend(id: "shared", agentID: nil, name: "Shared infrastructure", usd: 3, wins: 0),
            ], wins: 3)
        #expect(spend.perWinUSD == 4)
        #expect(spend.categoryFractions.map { ($0 * 100).rounded() } == [67, 33])
        #expect(spend.agents[0].perWinUSD == 3 && spend.agents[1].perWinUSD == nil)
        #expect(spend.agents[2].isShared)
        #expect(spend.fraction(of: spend.agents[0]) == 0.5)
        #expect(spend.reconciliationNote == "Includes $2.50 estimated")
        var billed = spend
        billed.estimatedUSD = 0
        #expect(billed.reconciliationNote == "Reconciled with billing")
        var none = spend
        none.wins = 0
        #expect(none.perWinUSD == nil)
    }

    @Test func theDaemonsSpendMapsToCategoriesAndAgents() throws {
        let json = """
        {"total":12.5,"estimated":2.5,"actual":10,"budgetUsd":50,
         "byCategory":[{"id":"tokens","label":"Model tokens","usd":8},{"id":"bench_runner","label":"Bench runner","usd":4.5}],
         "byAgent":[{"agentId":"a1","kind":"claude-code","branch":"ratchet/a1","usd":6},
                    {"agentId":"a2","kind":"claude-code","branch":"ratchet/a2","usd":3.5},
                    {"agentId":null,"kind":null,"branch":null,"usd":3}],
         "series":[]}
        """
        let spend = try DaemonMapping.spendBreakdown(Data(json.utf8), wins: ["a1": 2, "a2": 1], totalWins: 3)
        #expect(spend.categories.map(\.label) == ["Model tokens", "Bench runner"])
        // Two agents of one kind are told apart by their branch.
        #expect(spend.agents.map(\.name) == ["Claude Code · ratchet/a1", "Claude Code · ratchet/a2", "Shared infrastructure"])
        #expect(spend.agents.map(\.wins) == [2, 1, 0])
        #expect(spend.agents[2].isShared)
        #expect(spend.perWinUSD == 12.5 / 3)
    }

    @Test func theSampleResultHasEverythingTheScreenShows() async throws {
        let client = MockRatchetClient(now: testNow)
        let insights = try await client.insights(optimizationID: "opt-cli")
        #expect(insights.spans?.map(\.name) == ["parse", "db.query", "serialize", "wait"])
        #expect(insights.compare.first?.key == "p95 latency")
        #expect(insights.change?.files.count == 2 && insights.change?.unavailable == nil)
        let spend = try #require(insights.spend)
        #expect(abs(spend.agents.reduce(0) { $0 + $1.usd } - spend.totalUSD) < 0.05)
        #expect(abs(spend.categories.reduce(0) { $0 + $1.usd } - spend.totalUSD) < 0.05)
        // No win, no spans and nothing to diff.
        let none = try await client.insights(optimizationID: "opt-bundle")
        #expect(none.spans == nil && none.compare.isEmpty)
        #expect(none.change?.unavailable == "No verified win yet.")
    }

    @MainActor
    @Test func theStoreKeepsTheInsightsOfWhatWasOpened() async {
        let store = AppStore(client: MockRatchetClient(now: testNow))
        #expect(store.insight.insights["opt-forecast"] == nil)
        await store.loadInsights("opt-forecast")
        #expect(store.insight.insights["opt-forecast"]?.spend?.wins == 3)
        await store.loadInsights("nobody")
        #expect(store.lastError != nil)
    }
}

@Suite("Benchmark run")
struct BenchmarkRunTests {
    @Test func theMetricSwitchListsTheObjectivesFamilyAndReplotsTheRatchet() async throws {
        let client = MockRatchetClient(now: testNow)
        let p95 = try await client.benchmarkSeries(optimizationID: "opt-checkout", metric: nil)
        #expect(p95.hasMetricSwitch)
        #expect(p95.options.map(\.label) == ["p50", "p95", "p99"])
        #expect(p95.metric == "p95" && p95.selected?.isObjective == true)
        #expect(p95.noiseFloor != nil)
        #expect(p95.best == 288 && p95.locks == 2)
        #expect(p95.improvement == 412 - 288)

        let p99 = try await client.benchmarkSeries(optimizationID: "opt-checkout", metric: "p99")
        #expect(p99.metric == "p99" && p99.selected?.isObjective == false)
        #expect(p99.noiseFloor == nil, "the noise band belongs to the objective")
        #expect((p99.best ?? 0) > (p95.best ?? 0))
        await #expect(throws: RatchetClientError.self) { try await client.benchmarkSeries(optimizationID: "opt-checkout", metric: "nope") }
    }

    @Test func aSingleMetricHidesTheSwitch() async throws {
        let series = try await MockRatchetClient(now: testNow).benchmarkSeries(optimizationID: "opt-cli", metric: nil)
        #expect(!series.hasMetricSwitch)
    }

    @Test func rejectedPointsCanBeHidden() async throws {
        let series = try await MockRatchetClient(now: testNow).benchmarkSeries(optimizationID: "opt-checkout", metric: nil)
        #expect(series.visiblePoints(showRejected: true).count > series.visiblePoints(showRejected: false).count)
        #expect(series.visiblePoints(showRejected: false).allSatisfy { $0.verified })
        #expect(series.agents.count == 2)
    }

    @Test func theDaemonsSeriesMaps() throws {
        let vm = try JSONDecoder().decode(VM.RatchetSeries.self, from: Data(Fixtures.ratchetIngest.utf8))
        let series = DaemonMapping.benchmarkSeries(vm)
        #expect(!series.lowerIsBetter && series.unit == "k/s")
        #expect(series.steps.map(\.value) == [41.2, 49.8, 58.9])
        #expect(series.best == 58.9 && series.locks == 2)
        #expect(abs((series.improvement ?? 0) - 17.7) < 1e-9)
        #expect(series.points.map(\.verified) == [true, false, true])
        #expect(series.points[0].agent == "Codex")
        #expect(series.noiseFloor == 0.9)
    }

    @Test func candidateFiltersNarrowTheTable() async throws {
        let rows = try await MockRatchetClient(now: testNow).candidateResults(optimizationID: "opt-checkout")
        #expect(CandidateFilter.all.apply(to: rows).count == rows.count)
        #expect(CandidateFilter.verified.apply(to: rows).count == 2)
        let blocked = CandidateFilter.blocked.apply(to: rows)
        #expect(blocked.count == 1)
        #expect(blocked[0].guardrailText.hasPrefix("protected path changed"))
        #expect(blocked[0].note == nil)
        #expect(CandidateFilter.pending.apply(to: rows).count == 1)
        #expect(CandidateFilter.failedConfirm.title == "failed confirm")
        let verified = CandidateFilter.verified.apply(to: rows)[0]
        #expect(verified.guardrailText == "passed" && verified.movedRatchet)
    }

    @Test func theDaemonsCandidatesKeepTheirBenchResults() throws {
        let list = try JSONDecoder().decode([VM.CandidateVm].self, from: Data(Fixtures.candidatesIngest.utf8))
        let rows = list.map(DaemonMapping.candidateResult)
        #expect(rows.map(\.result) == [.verified, .rejected, .verified])
        #expect(rows.map(\.sha) == ["67b05fc", "67b05fd", "67b05fe"])
        #expect(rows[0].confirmValue == 49.1 && rows[1].confirmValue == nil)
        #expect(rows[0].samples.count == 5 && rows[0].movedRatchet && !rows[1].movedRatchet)
        #expect(rows[1].note == "improvement 0.6 does not exceed required gain 1.49")
        #expect(CandidateFilter.rejected.apply(to: rows).map(\.sha) == ["67b05fd"])
    }
}

@Suite("Chart helpers")
struct ChartHelperTests {
    @Test func histogramBinsCountEverySampleAndClampTheMaxIntoTheLastBin() {
        let bins = Histogram.bins(of: [0, 1, 1, 9, 10], lo: 0, hi: 10, count: 5)
        #expect(bins == [3, 0, 0, 0, 2])
        #expect(bins.reduce(0, +) == 5)
        // Non-finite samples are skipped; equal values land in one bin.
        #expect(Histogram.bins(of: [3, .nan, 3], lo: 3, hi: 3, count: 4) == [2, 0, 0, 0])
    }

    @Test func theBinCountGrowsWithTheSamplesWithinBounds() {
        #expect(Histogram.binCount(forSamples: 1) == 8)
        #expect(Histogram.binCount(forSamples: 16) == 16)
        #expect(Histogram.binCount(forSamples: 100_000) == 28)
    }

    @Test func twoSeriesShareOneRange() {
        #expect(Histogram.range(of: [[2, 4], [1, 3]]) == 1...4)
        #expect(Histogram.range(of: [[], []]) == nil)
    }

    @Test func evalBarsKeepTheSmallestBarVisibleAndTheLargestFull() {
        #expect(EvalBars.heights(of: [1, 5], height: 100) == [6, 94])
        #expect(EvalBars.heights(of: [3, 3], height: 100) == [60, 60])
        #expect(EvalBars.heights(of: [], height: 100).isEmpty)
        #expect(EvalBars.guide(for: 3, values: [1, 5], height: 100) == 50)
        #expect(EvalBars.guide(for: 3, values: [3, 3], height: 100) == nil)
    }

    @Test func measuredValuesKeepAboutThreeDigits() {
        #expect(MetricNumber.text(131.4, unit: "ms") == "131 ms")
        #expect(MetricNumber.text(15.93, unit: "%") == "15.9%")
        #expect(MetricNumber.text(0.6481) == "0.648")
        #expect(MetricNumber.text(4.567) == "4.57")
    }
}

@Suite("Insights through the daemon client")
struct InsightsDaemonClientTests {
    private func daemon() -> FakeDaemon {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/optimizations/opt_demo_search", 200, Fixtures.detailSearch)
        daemon.route("GET /v1/optimizations/opt_demo_search/spend", 200, #"{"total":42.6,"estimated":1,"actual":41.6,"budgetUsd":120,"byCategory":[{"id":"tokens","label":"Model tokens","usd":30},{"id":"bench_runner","label":"Bench runner","usd":12.6}],"byAgent":[{"agentId":null,"kind":null,"branch":null,"usd":42.6}],"series":[]}"#)
        daemon.route("GET /v1/optimizations/opt_demo_search/winning-change", 200, #"{"commits":2,"sha":"67b05fe","baseRef":"main","files":[{"path":"src/ingest/writer.ts","additions":9,"deletions":1,"binary":false}],"additions":9,"deletions":1,"excerpt":{"path":"src/ingest/writer.ts","lines":[{"kind":"hunk","text":"@@ -22,10 +22,14 @@ export class SegmentWriter {"},{"kind":"add","text":"flush();"}]},"unavailable":null}"#)
        daemon.route("GET /v1/agents/agt_1/events", 200, #"{"events":[{"id":1,"at":1793000000000,"type":"status","text":"online"}],"cursor":1}"#)
        daemon.route("GET /v1/optimizations/opt_demo_ingest/candidates", 200, Fixtures.candidatesIngest)
        daemon.route("GET /v1/optimizations/opt_demo_ingest/ratchet", 200, Fixtures.ratchetIngest)
        return daemon
    }

    @Test func theResultDetailsComeFromThreeRoutes() async throws {
        let client = DaemonRatchetClient(transport: daemon())
        let insights = try await client.insights(optimizationID: "opt_demo_search")
        #expect(insights.spend?.categories.count == 2 && insights.spend?.agents.first?.isShared == true)
        #expect(insights.change?.files.map(\.path) == ["src/ingest/writer.ts"])
        #expect(insights.change?.excerpt.map(\.kind) == [.hunk, .add])
    }

    @Test func aMissingSpendOrDiffLeavesTheRestOfTheResult() async throws {
        let fake = daemon()
        fake.route("GET /v1/optimizations/opt_demo_search/spend", 500, #"{"code":"internal","message":"boom"}"#)
        fake.route("GET /v1/optimizations/opt_demo_search/winning-change", 500, #"{"code":"internal","message":"boom"}"#)
        let insights = try await DaemonRatchetClient(transport: fake).insights(optimizationID: "opt_demo_search")
        #expect(insights.spend == nil && insights.change == nil)
    }

    @Test func theLogIsReadThroughTheEventsRoute() async throws {
        let fake = daemon()
        let page = try await DaemonRatchetClient(transport: fake).agentLog(agentID: "agt_1", after: nil, limit: 200)
        #expect(page.lines.map(\.text) == ["online"] && page.cursor == 1)
        #expect(fake.count("GET /v1/agents/agt_1/events") == 1)
        await #expect(throws: RatchetClientError.self) {
            try await DaemonRatchetClient(transport: fake).agentLog(agentID: "agt_missing", after: 3, limit: 10)
        }
    }

    @Test func theSeriesAndTheCandidatesAreMapped() async throws {
        let client = DaemonRatchetClient(transport: daemon())
        let series = try await client.benchmarkSeries(optimizationID: "opt_demo_ingest", metric: nil)
        #expect(series.steps.count == 3)
        let rows = try await client.candidateResults(optimizationID: "opt_demo_ingest")
        #expect(rows.count == 3)
    }
}
