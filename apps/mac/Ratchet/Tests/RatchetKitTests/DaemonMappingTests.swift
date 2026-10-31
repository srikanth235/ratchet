import Foundation
import RatchetAPI
import Testing
@testable import RatchetKit

/// The daemon's JSON (Fixtures.swift) through the generated types and the mapping to the app's models.
@Suite("Daemon mapping")
struct DaemonMappingTests {
    private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try JSONDecoder().decode(type, from: Data(json.utf8))
    }

    private func rows() throws -> [VM.OptimizationVm] {
        try decode(VM.OptimizationsSnapshot.self, Fixtures.optimizations).optimizations
    }

    @Test func optimizationRowsKeepTheirMeaning() throws {
        let list = try rows()
        let search = try #require(list.first { $0.id == "opt_demo_search" })
        let mapped = DaemonMapping.optimization(search)
        #expect(mapped.title == "search-api p95 latency")
        #expect(mapped.status == .running)
        #expect(mapped.metric.name == "p95")
        #expect(mapped.metric.unit == "ms")
        #expect(mapped.metric.lowerIsBetter)
        #expect(mapped.winsCount == 5)
        #expect(mapped.agentsCount == 2)
        #expect(mapped.baseline == 182)
        #expect(mapped.isImprovement)
        #expect(mapped.valuePerYearUSD != nil)

        let ingest = try #require(list.first { $0.id == "opt_demo_ingest" })
        let finished = DaemonMapping.optimization(ingest)
        #expect(finished.status == .done)
        #expect(finished.metric.name == "events per s")
        #expect(!finished.metric.lowerIsBetter)
        #expect(finished.metric.decimals == 1)
        #expect(finished.valuePerYearUSD == nil)
        #expect(finished.isImprovement)

        let draft = try #require(list.first { $0.id == "opt_demo_web" })
        let waiting = DaemonMapping.optimization(draft)
        #expect(waiting.status == .waiting)
        #expect(waiting.best == nil && waiting.baseline == 0)
    }

    @Test func aDetailAddsTheClockTheNoiseAndThePullRequest() throws {
        let detail = try decode(VM.OptimizationDetail.self, Fixtures.detailSearch)
        let mapped = DaemonMapping.optimization(detail.summary, detail: detail)
        #expect(mapped.timeLimitHours == 8)
        let left = try #require(mapped.secondsLeft)
        #expect(abs(left - (28_800 - detail.elapsedS)) < 0.001)
        #expect(mapped.startedAt == DaemonMapping.date(ms: try #require(detail.startedAt)))
        let noise = try #require(mapped.noise)
        #expect(abs(noise - 3.1 / 182) < 1e-9)
        #expect(mapped.prNumber == nil)
        #expect(DaemonMapping.pullRequestNumber("https://github.com/acme/ingest/pull/214") == 214)
        #expect(DaemonMapping.pullRequestNumber(nil) == nil)

        let finished = try decode(VM.OptimizationDetail.self, Fixtures.detailIngest)
        #expect(DaemonMapping.optimization(finished.summary, detail: finished).secondsLeft == nil)
    }

    @Test func agentsMapToTheirState() throws {
        let detail = try decode(VM.OptimizationDetail.self, Fixtures.detailSearch)
        let agents = detail.agents.map(DaemonMapping.agent)
        #expect(agents.count == 2)
        #expect(agents[0].name == "Claude Code")
        #expect(agents[0].placement == .cloud)
        #expect(agents[0].state == .working)
        #expect(agents[0].evals == Int(detail.agents[0].stats.candidates))
        #expect(agents[0].wins == Int(detail.agents[0].stats.verified))
        #expect(!agents[0].activity.isEmpty)
    }

    @Test func candidatesShowTheirShaAndVerdict() throws {
        let list = try decode([VM.CandidateVm].self, Fixtures.candidatesIngest)
        let mapped = list.map(DaemonMapping.candidate)
        #expect(mapped.map(\.id) == ["67b05fc", "67b05fd", "67b05fe"])
        #expect(mapped.map(\.verdict) == [.verified, .noise, .verified])
        #expect(mapped[0].title == "Batch writes into 4 KiB segments")
        #expect(mapped[0].agent == "Codex")
        #expect(mapped[0].value == 49.8)
    }

    @Test func theRatchetBecomesMoves() throws {
        let series = try decode(VM.RatchetSeries.self, Fixtures.ratchetIngest)
        let start = Date(timeIntervalSince1970: 1_000_000)
        let moves = DaemonMapping.moves(series, optimizationID: "opt_demo_ingest", start: start)
        #expect(moves.map(\.value) == [49.8, 58.9])
        #expect(moves.map(\.title) == ["Batch writes into 4 KiB segments", "Vectorise the parser inner loop"])
        #expect(moves[0].at == start.addingTimeInterval(94 * 60))
        #expect(moves[0].id == "opt_demo_ingest-move-1")
    }

    @Test func theOverviewKeepsItsNumbersAndItsStaircase() throws {
        let vm = try decode(VM.OverviewVm.self, Fixtures.overview)
        let overview = DaemonMapping.overview(vm, budgetUSD: 123)
        #expect(overview.range == .days90)
        #expect(overview.spentUSD == vm.spent.usd)
        #expect(overview.budgetUSD == 500)  // the cap of the daemon, not the fallback
        #expect(overview.valueLockedPerYearUSD == vm.valueLockedIn.usdPerYear)
        #expect(overview.winsCount == Int(vm.valueLockedIn.wins))
        // start + one point per win + end; the ends have no win
        #expect(overview.series.count == vm.staircase.points.count + 2)
        #expect(overview.series.first?.win == nil && overview.series.last?.win == nil)
        #expect(overview.series.dropFirst().dropLast().allSatisfy { $0.win != nil })
        #expect(overview.series == overview.series.sorted { $0.date < $1.date })
        #expect(overview.series.last?.valuePerYearUSD == vm.staircase.points.last?.usdPerYear)
        #expect(overview.needsYou.map(\.kind) == [.mergeWins, .mergeWins, .signIn])
        #expect(overview.needsYou[0].optimizationID == "opt_demo_ingest")
        #expect(overview.needsYou[2].optimizationID == "")
        #expect(overview.needsYou[0].actionTitle == "Open PR")
        #expect(overview.returnMultiple != nil)
    }

    @Test func withoutACapTheBudgetsAddUp() throws {
        var vm = try decode(VM.OverviewVm.self, Fixtures.overview)
        vm.spent.capUsd = nil
        #expect(DaemonMapping.overview(vm, budgetUSD: 123).budgetUSD == 123)
    }

    @Test func connectorsKeepTheirStatesAndCaps() throws {
        let list = try decode([VM.ConnectorVm].self, Fixtures.connectors).map(DaemonMapping.connector)
        func connector(_ id: String) throws -> Connector { try #require(list.first { $0.id == id }) }
        #expect(try connector("conn_gh_personal").state == .connected)
        #expect(try connector("conn_gh_personal").isDefault)
        let prod = try connector("conn_aws_prod")
        #expect(prod.state == .valid)
        #expect(prod.monthlyCapUSD == 500)
        #expect(prod.usedThisMonthUSD == 88)
        #expect(prod.leftThisMonthUSD == 412)
        #expect(try connector("conn_aws_staging").state == .expired)
        #expect(try connector("conn_agent_cc_login").state == .signedIn)
        #expect(try connector("conn_agent_cc_login").canRunInCloud == false)
        #expect(try connector("conn_agent_cc_key").canRunInCloud)
        #expect(try connector("conn_agent_codex_login").state == .signedOut)
    }

    @Test func aSelectionGoesBothWays() throws {
        let vm = try decode(VM.ConnectorSelectionVm.self, Fixtures.selection)
        let selection = DaemonMapping.selection(vm)
        #expect(selection.repoID == "acme/shop-api")
        #expect(selection.awsID == "conn_aws_prod")
        #expect(selection.agents == ["conn_agent_cc_login": .local])
        let input = DaemonMapping.selectionInput(selection)
        #expect(input.awsConnectorId == "conn_aws_prod")
        #expect(input.agents.map(\.connectorId) == ["conn_agent_cc_login"])
        #expect(input.agents.first?.runsIn == .local)
    }

    @Test func anEnvironmentDraftReadsLikeTheMock() throws {
        let draft = DaemonMapping.draft(try decode(VM.EnvironmentDraftVm.self, Fixtures.draft))
        #expect(draft.id == "envd_1")
        #expect(draft.repoID == "acme/shop-api")
        #expect(draft.scope == .full)
        #expect(draft.state == .draft)
        #expect(draft.emphasis == "full stack")
        #expect(draft.headline.contains(draft.emphasis))
        #expect(draft.draftedBy == "Claude Code")
        #expect(draft.draftSeconds == 120)
        #expect(draft.readyMinutes == 6)
        #expect(draft.resources.count == 4)
        #expect(draft.resources[0].machine == "c7i.2xlarge")
        #expect(draft.resources[0].detail == "shop-api in the repo's container")
        #expect(draft.files == ["main.tf", "Dockerfile", "seed.sh"])
        #expect(draft.checks == "isolated, results sealed, same hardware every time, full teardown")
        #expect(abs(draft.usdPerHour - 0.59) < 1e-9)
    }

    @Test func theSandboxAndItsReceipt() throws {
        let vm = try decode(VM.EnvironmentsVm.self, Fixtures.environments)
        let sandbox = DaemonMapping.sandbox(vm)
        #expect(sandbox.isOn)
        #expect(sandbox.account == "us-east-1")
        #expect(sandbox.offAt == nil)
        #expect(DaemonMapping.endableSandboxID(vm) == "sbx-sim-1")

        let receipt = DaemonMapping.receipt(try #require(vm.lastReceipt), startedAt: Date(timeIntervalSince1970: 1_793_430_000))
        #expect(receipt.id == "sbx-demo-0")
        #expect(receipt.account == "us-east-1")
        #expect(receipt.costUSD == 61.2)
        #expect(receipt.destroyed.prefix(3) == ["3 instances", "1 agent identity", "1 secret"])
        #expect(receipt.destroyed.contains("vpc"))
        #expect(receipt.allGone)
        #expect(receipt.duration > 0)

        var off = vm
        off.environments = []
        #expect(DaemonMapping.sandbox(off) == .off)
        #expect(DaemonMapping.endableSandboxID(off) == nil)
    }

    @Test func templatesAndRepos() throws {
        let templates = try decode([VM.TemplateInfo].self, Fixtures.templates).map(DaemonMapping.template)
        #expect(!templates.isEmpty)
        #expect(templates.contains { $0.kind == .demo && $0.id.hasPrefix("demo-") })
        #expect(templates.allSatisfy { !$0.title.isEmpty && !$0.machine.isEmpty })

        let repos = try decode([VM.RepoVm].self, Fixtures.repos).map(DaemonMapping.repo)
        #expect(repos.first?.id == "acme-org/shop-api")
        #expect(repos.first?.account == "acme-org")
        #expect(repos.first?.name == "shop-api")
    }

    @Test func anInspectedRepoBecomesAHarnessDetection() throws {
        let inspected = try decode(VM.RepoInspectVm.self, Fixtures.inspect)
        let found = DaemonMapping.harness(repoID: "acme/search-api", inspected)
        #expect(found.found)
        #expect(found.summary.hasPrefix("Found ratchet.yaml: p95"))

        var bare = inspected
        bare.config = nil
        let missing = DaemonMapping.harness(repoID: "acme/search-api", bare)
        #expect(!missing.found)
        #expect(missing.summary.contains("template"))
    }

    @Test func idsAndRangesRoundTrip() {
        for range in TimeRange.allCases {
            #expect(DaemonMapping.timeRange(DaemonMapping.range(range)) == range)
        }
        #expect(DaemonMapping.decimals([412, 288]) == 0)
        #expect(DaemonMapping.decimals([41.2, 58.9]) == 1)
        #expect(DaemonMapping.decimals([2.41, 1.62]) == 2)
        #expect(DaemonMapping.decimals([0.612, 0.648]) == 3)
        #expect(DaemonMapping.metricName("http_req_duration.p95") == "p95")
        #expect(DaemonMapping.metricName("render.p99") == "p99")
    }
}
