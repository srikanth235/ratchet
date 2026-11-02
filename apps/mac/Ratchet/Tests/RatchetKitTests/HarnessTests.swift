import Foundation
import Testing
@testable import RatchetKit

private let epoch = Date(timeIntervalSince1970: 1_793_452_800)

// MARK: Editors

@Suite("Agent, limit and schedule editors")
struct EditorTests {
    @Test func theAgentOrderMovesAndReadsTheSameEverywhere() {
        var editor = AgentOrderEditor(order: [.claudeCode, .codex, .grok])
        #expect(editor.summary == "Claude Code → Codex → Grok · on this Mac")
        #expect(!editor.canMoveUp(at: 0) && editor.canMoveDown(at: 0) && editor.canMoveUp(at: 2) && !editor.canMoveDown(at: 2))
        editor.moveDown(at: 0)
        editor.moveUp(at: 0)  // the first cannot go up: nothing happens
        #expect(editor.order == [.codex, .claudeCode, .grok])
        editor.moveDown(at: 2)  // nor the last down
        #expect(editor.order == [.codex, .claudeCode, .grok])
        editor.placement = .cloud
        #expect(editor.summary == "Codex → Claude Code → Grok · in the cloud")
        #expect(editor.instruction == "Use Codex, then Claude Code, then Grok. Run them in the cloud.")
        #expect(editor.chipTitle == "Codex + 2 more")
    }

    @Test func theListKeepsAtLeastOneAgentAndOffersOnlyWhatIsNotInIt() {
        var editor = AgentOrderEditor(order: [.claudeCode, .codex], available: [.claudeCode, .codex, .grok])
        #expect(editor.addable == [.grok])
        editor.add(.grok)
        editor.add(.grok)
        #expect(editor.order == [.claudeCode, .codex, .grok] && editor.addable.isEmpty)
        editor.remove(.codex)
        editor.remove(.grok)
        editor.remove(.claudeCode)
        #expect(editor.order == [.claudeCode] && !editor.canRemove)
        #expect(editor.instruction == "Use Claude Code. Run them on this Mac." && editor.chipTitle == "Claude Code")
    }

    @Test func anEditorReadsACardRow() {
        let row = ChatCardRow(
            key: "agents", label: "Agents", value: "Codex → Grok · in the cloud", editable: true,
            options: [ChatCardOption(label: "Codex", value: "codex"), ChatCardOption(label: "Grok", value: "grok")],
            raw: .array([.string("codex"), .string("grok")]))
        let editor = AgentOrderEditor(row: row)
        #expect(editor.order == [.codex, .grok] && editor.placement == .cloud && editor.available == [.codex, .grok])
        let limits = LimitsEditor(row: ChatCardRow(key: "limits", label: "Each run", value: "up to $20 and 4 h", raw: .object(["budgetUsd": .number(20)])))
        #expect(limits == LimitsEditor(budgetUSD: 20, hours: 4) && limits.summary == "up to $20 and 4 h")
        #expect(LimitsEditor(row: ChatCardRow(key: "limits", label: "Each run", value: "up to $50")) == LimitsEditor(budgetUSD: 50, hours: 2))
    }

    @Test func scheduleWordsRoundTrip() {
        #expect(ScheduleKind.allCases.map(\.title) == ["Once", "Every night", "Weekdays at night", "Once a week", "On every push"])
        #expect(ScheduleKind(shown: "every night at 02:00") == .nightly && ScheduleKind(shown: "weekdays at 02:00") == .weekdays)
        #expect(ScheduleKind(shown: "weekly at 02:00") == .weekly && ScheduleKind(shown: "run once, now") == .once)
        #expect(ScheduleKind(shown: "on every push") == .onPush && ScheduleKind(shown: "never") == nil)
        #expect(ScheduleKind(rawValue: "on-push") == .onPush)
        #expect(ScheduleKind.once.instruction.contains("do not add a schedule"))
    }

    @Test func restoringARowSaysItsOldValue() {
        let agents = ChatCardRow(key: "agents", label: "Agents", value: "Claude Code · on this Mac", raw: .array([.string("claude-code")]))
        #expect(DraftRowEdit.restoring(agents).instruction == "Use Claude Code. Run them on this Mac.")
        let schedule = ChatCardRow(key: "schedule", label: "Schedule", value: "every night at 02:00", raw: .string("every night at 02:00"))
        #expect(DraftRowEdit.restoring(schedule).instruction == ScheduleKind.nightly.instruction)
        let goal = ChatCardRow(key: "goal", label: "Goal", value: "Make it faster")
        #expect(DraftRowEdit.restoring(goal).instruction == "Change the goal: put it back to Make it faster")
    }
}

// MARK: The harness client, on the mock and on the wire

@Suite("Harness client")
struct HarnessClientTests {
    private func daemon() -> (DaemonHarnessClient, FakeDaemon) {
        let fake = FakeDaemon()
        return (DaemonHarnessClient(transport: fake), fake)
    }

    @Test func theAgentsPolicyReadsAndWrites() async throws {
        let (client, fake) = daemon()
        fake.route("GET /v1/optimizations/o1/agents", 200, #"{"harnesses":["claude-code","codex","mock"],"waitLimitS":1800,"crashStrikes":3,"recheckS":300,"source":"config","summary":"Claude Code → Codex"}"#)
        let policy = try await client.agentsPolicy(optimizationID: "o1")
        #expect(policy.harnesses == [.claudeCode, .codex] && policy.isLockedByConfig && policy.summary == "Claude Code → Codex")
        fake.route("PUT /v1/optimizations/o1/agents", 200, #"{"harnesses":["codex"],"waitLimitS":600,"crashStrikes":3,"recheckS":300,"source":"settings","summary":"Codex"}"#)
        let saved = try await client.setAgentsPolicy(optimizationID: "o1", AgentsPolicyInput(harnesses: [.codex], waitLimitSeconds: 600))
        #expect(saved.harnesses == [.codex] && saved.waitLimitSeconds == 600 && !saved.isLockedByConfig)
        let body = try #require(fake.requests.last { $0.key == "PUT /v1/optimizations/o1/agents" }?.body)
        let sent = try #require(try JSONSerialization.jsonObject(with: Data(body.utf8)) as? [String: Any])
        #expect(sent["harnesses"] as? [String] == ["codex"] && sent["waitLimitS"] as? Int == 600 && sent["crashStrikes"] == nil)
    }

    @Test func theDaemonRefusesWhileTheConfigHoldsTheList() async {
        let (client, fake) = daemon()
        fake.route("PUT /v1/optimizations/o1/agents", 409, #"{"code":"conflict","message":"ratchet.yaml has an agents section."}"#)
        await #expect(throws: RatchetClientError.invalid("ratchet.yaml has an agents section.")) {
            try await client.setAgentsPolicy(optimizationID: "o1", AgentsPolicyInput(harnesses: [.codex]))
        }
        await #expect(throws: RatchetClientError.invalid("Pick at least one agent.")) {
            try await client.setAgentsPolicy(optimizationID: "o1", AgentsPolicyInput(harnesses: []))
        }
    }

    @Test func theValueModelReadsWritesAndClears() async throws {
        let (client, fake) = daemon()
        fake.route("GET /v1/optimizations/o1/value", 200, #"{"set":true,"metric":"p95","direction":"minimize","usdPerUnit":1200.0,"period":"year","unit":"ms","basis":"estimated","note":"ms saved","summary":"$1,200 per ms a year · estimated · ms saved"}"#)
        let model = try await client.valueModel(optimizationID: "o1")
        #expect(model.isSet && model.usdPerUnit == 1200 && model.period == .year && model.lowerIsBetter && model.basis == .estimated)
        fake.route("PUT /v1/optimizations/o1/value", 200, #"{"set":true,"metric":"p95","direction":"minimize","usdPerUnit":2.5,"period":"month","unit":"k/s","basis":"measured","note":null,"summary":"$2.50 per k/s a month"}"#)
        let saved = try await client.setValueModel(optimizationID: "o1", ValueModelInput(usdPerUnit: 2.5, period: .month, basis: .measured, unit: "k/s"))
        #expect(saved.period == .month && saved.basis == .measured && saved.note == nil)
        let body = try #require(fake.requests.last { $0.key == "PUT /v1/optimizations/o1/value" }?.body)
        let sent = try #require(try JSONSerialization.jsonObject(with: Data(body.utf8)) as? [String: Any])
        #expect(sent["usdPerUnit"] as? Double == 2.5 && sent["period"] as? String == "month" && sent["note"] == nil)
        fake.route("DELETE /v1/optimizations/o1/value", 204)
        fake.route("GET /v1/optimizations/o1/value", 200, #"{"set":false,"metric":"p95","direction":"minimize","usdPerUnit":null,"period":null,"unit":null,"basis":null,"note":null,"summary":"Not set"}"#)
        let cleared = try await client.clearValueModel(optimizationID: "o1")
        #expect(!cleared.isSet && cleared.summary == "Not set")
        await #expect(throws: RatchetClientError.self) { try await client.setValueModel(optimizationID: "o1", ValueModelInput(usdPerUnit: 0)) }
    }

    @Test func theScheduleReadsWritesAndClears() async throws {
        let (client, fake) = daemon()
        fake.route("GET /v1/optimizations/o1/schedule", 200, #"{"schedule":"weekdays","summary":"Weekdays at night","runsNext30Days":22}"#)
        let schedule = try await client.schedule(optimizationID: "o1")
        #expect(schedule.kind == .weekdays && schedule.runsNext30Days == 22)
        fake.route("PUT /v1/optimizations/o1/schedule", 200, #"{"schedule":null,"summary":"Not scheduled","runsNext30Days":null}"#)
        let cleared = try await client.setSchedule(optimizationID: "o1", nil)
        #expect(cleared.kind == nil && cleared.summary == "Not scheduled")
        let body = try #require(fake.requests.last { $0.key == "PUT /v1/optimizations/o1/schedule" }?.body)
        #expect(body == #"{"schedule":null}"#)
    }

    private let overviewJSON = #"""
    {"phase":"wins","emptyState":null,
     "story":{"basis":"estimated","delivered":{"usdPerYear":12500.0,"wins":3},"pending":{"usdPerYear":2900.0,"wins":1,"pullRequests":1},"readyToShip":{"usdPerYear":0.0,"wins":0},
              "spentUsd":184.0,"return":68.0,"paybackDays":5.4,"line":"$12.5k delivered + $2.9k pending · 68×"},
     "spent":{"usd":184.0,"budgetUsd":500.0,"optimizations":3,"awsUsd":121.0,"agentsApiUsd":63.0,"includedTokens":0,"detail":"AWS $121 · agents $63"},
     "forecast":{"days":30,"scheduledRuns":30,"unforecast":0,"expectedUsd":1140.0,"expectedAwsUsd":1140.0,"expectedAgentsApiUsd":0.0,"expectedIncludedTokens":0,"hardCapUsd":1500.0,"line":"30 runs · ~$1.1k · cap $1.5k"},
     "chart":[{"at":1793000000000,"spentUsd":0.0,"deliveredUsdPerYear":0.0,"pendingUsdPerYear":0.0,"readyUsdPerYear":0.0},
              {"at":1793452800000,"spentUsd":184.0,"deliveredUsdPerYear":12500.0,"pendingUsdPerYear":2900.0,"readyUsdPerYear":0.0}],
     "rows":[],"wins":[]}
    """#

    @Test func homeAndTheOverviewTellOneStoryOnTheWire() async throws {
        let (client, fake) = daemon()
        fake.route("GET /v1/overview", 200, overviewJSON)
        fake.route("GET /v1/home", 200, #"{"range":"90d","empty":false,"story":{"basis":"estimated","delivered":{"usdPerYear":12500.0,"wins":3},"pending":{"usdPerYear":2900.0,"wins":1,"pullRequests":1},"readyToShip":{"usdPerYear":0.0,"wins":0},"spentUsd":184.0,"return":68.0,"paybackDays":5.4,"line":"$12.5k delivered + $2.9k pending · 68×"},"optimizations":3,"pullRequests":{"open":1,"merged":2},"failed":{"id":"n1","kind":"run-failed","optimizationId":"o9","title":"CLI cold start","sub":"AWS session expired","action":"Open"}}"#)
        let home = try await client.home(range: .days90)
        let overview = try await client.valueOverview(range: .days90)
        #expect(home.story == overview.story)
        #expect(home.story.headline == "$12.5k delivered + $2.9k pending · 68×" && home.story.returnText == "68×")
        #expect(home.optimizations == 3 && home.openPullRequests == 1 && home.mergedPullRequests == 2 && !home.isEmpty)
        #expect(home.failed == HomeFailure(optimizationID: "o9", title: "CLI cold start", detail: "AWS session expired"))
        #expect(home.countsLine == "3 optimizations · 3 PRs" && home.spentCaption == "Spent · 90 days")
        #expect(overview.spent.detail == "AWS $121 · agents $63" && overview.spent.awsUSD == 121 && overview.spent.agentsAPIUSD == 63)
        #expect(overview.forecast.expectedUSD == 1140 && overview.forecast.hardCapUSD == 1500 && overview.forecast.typicalRunUSD == 38)
        #expect(overview.chart.count == 2 && overview.chart[1].pendingUSDPerYear == 2900 && overview.phase == .wins && overview.hasChart)
        #expect(fake.requests.contains { $0.key == "GET /v1/home" } && fake.requests.contains { $0.key == "GET /v1/overview" })
    }

    @Test func theEmptyStatesCarryTheirWords() async throws {
        let (client, fake) = daemon()
        fake.route("GET /v1/overview", 200, #"{"phase":"empty","emptyState":{"title":"No wins yet","body":"Start your first optimization."},"story":{"basis":"estimated","delivered":{"usdPerYear":0,"wins":0},"pending":{"usdPerYear":0,"wins":0,"pullRequests":0},"readyToShip":{"usdPerYear":0,"wins":0},"spentUsd":0,"return":null,"paybackDays":null,"line":""},"spent":{"usd":0,"budgetUsd":0,"awsUsd":0,"agentsApiUsd":0,"includedTokens":0,"detail":""},"forecast":{"days":30,"scheduledRuns":0,"unforecast":0,"expectedUsd":0,"expectedAwsUsd":0,"expectedAgentsApiUsd":0,"expectedIncludedTokens":0,"hardCapUsd":null,"line":""},"chart":[]}"#)
        let overview = try await client.valueOverview(range: .all)
        #expect(overview.phase == .empty && overview.emptyState == EmptyStateText(title: "No wins yet", body: "Start your first optimization."))
        #expect(!overview.hasChart && overview.story.headline == "" && overview.story.returnText == nil && overview.forecast.typicalRunUSD == nil)
        let query = fake.requests.first { $0.key == "GET /v1/overview" }
        #expect(query != nil)
    }

    @Test func theMockEnforcesTheSameRules() async throws {
        let mock = MockHarnessClient(configLocked: ["locked"])
        await #expect(throws: RatchetClientError.self) { try await mock.setAgentsPolicy(optimizationID: "o1", AgentsPolicyInput(harnesses: [])) }
        await #expect(throws: RatchetClientError.self) { try await mock.setAgentsPolicy(optimizationID: "locked", AgentsPolicyInput(harnesses: [.codex])) }
        let lockedPolicy = try await mock.agentsPolicy(optimizationID: "locked")
        #expect(lockedPolicy.isLockedByConfig)
        let saved = try await mock.setAgentsPolicy(optimizationID: "o1", AgentsPolicyInput(harnesses: [.grok, .codex]))
        let again = try await mock.agentsPolicy(optimizationID: "o1")
        #expect(saved.summary == "Grok → Codex" && again == saved)
        await #expect(throws: RatchetClientError.self) { try await mock.setValueModel(optimizationID: "o1", ValueModelInput(usdPerUnit: -1)) }
        await #expect(throws: RatchetClientError.self) { try await mock.setValueModel(optimizationID: "o1", ValueModelInput(usdPerUnit: 1, unit: String(repeating: "x", count: 25))) }
        await #expect(throws: RatchetClientError.self) { try await mock.setValueModel(optimizationID: "o1", ValueModelInput(usdPerUnit: 1, note: String(repeating: "x", count: 201))) }
        let value = try await mock.setValueModel(optimizationID: "o1", ValueModelInput(usdPerUnit: 3, unit: "ms"))
        let read = try await mock.valueModel(optimizationID: "o1")
        let cleared = try await mock.clearValueModel(optimizationID: "o1")
        #expect(value.isSet && read == value && !cleared.isSet)
        let weekly = try await mock.setSchedule(optimizationID: "o1", .weekly)
        let none = try await mock.setSchedule(optimizationID: "o1", nil)
        #expect(weekly.runsNext30Days == 4 && none.kind == nil)
    }

    @Test func homeAndTheOverviewAgreeInEveryScenario() async throws {
        for scenario in [ValueScenario.wins, .subscription, .noWins, .empty] {
            let mock = MockHarnessClient(scenario: scenario)
            let home = try await mock.home(range: .days90)
            let overview = try await mock.valueOverview(range: .days90)
            #expect(home.story == overview.story, "\(scenario)")
            #expect(home.isEmpty == (overview.phase == .empty), "\(scenario)")
        }
        let wins = try await MockHarnessClient(scenario: .wins).home(range: .days90)
        #expect(wins.story.headline == "$12.5k delivered + $2.9k pending · 68×" && !wins.story.headline.contains("locked in"))
        let plan = try await MockHarnessClient(scenario: .subscription).valueOverview(range: .days90)
        #expect(plan.spent.detail.contains("included in your plan") && plan.spent.agentsAPIUSD == 0 && plan.spent.includedTokens > 0)
        #expect(plan.story.spentUSD == plan.spent.usd)  // the return counts real dollars only
    }

    @Test func pendingValueNeverEntersTheReturnAndTheHeadlineFallsBackToTheNumbers() {
        let story = ValueStory(delivered: ValueAmount(usdPerYear: 12_500, wins: 3), pending: ValueAmount(usdPerYear: 2_900, wins: 1, pullRequests: 1), spentUSD: 184, returnMultiple: 68)
        #expect(story.headline == "$12.5k delivered + $2.9k pending · 68×" && story.pendingText == "$2.9k pending" && story.deliveredText == "$12.5k")
        let onlyPending = ValueStory(pending: ValueAmount(usdPerYear: 2_900, wins: 1, pullRequests: 1), spentUSD: 184)
        #expect(onlyPending.returnText == nil && onlyPending.headline == "$0.00 delivered + $2.9k pending")
    }
}

// MARK: Models over the clients

@MainActor
@Suite("Value story model")
struct ValueStoryModelTests {
    @Test func loadsBothScreensTogetherAndTheyAgree() async {
        let model = ValueStoryModel(client: MockHarnessClient(scenario: .wins))
        #expect(model.content == .loading && model.story == nil)
        await model.load()
        #expect(model.content == .wins && model.screensAgree && model.story?.returnText == "68×")
        #expect(model.typicalRunCostUSD == 38 && model.spendDetail == "AWS $121 · agents $63")
        #expect(ValueStoryModel.footnote == "Estimated from each optimization's Value row." && ValueStoryModel.footnoteLink == "Edit value")
    }

    @Test func noWinsAndFirstLaunchShowTheCalmEmptyState() async {
        let model = ValueStoryModel(client: MockHarnessClient(scenario: .empty))
        await model.load()
        #expect(model.content == .empty(EmptyStateText(title: "No wins yet", body: "Start your first optimization.")))
        #expect(model.home?.isEmpty == true && model.story?.headline == "")
    }

    @Test func aFailedLoadIsKeptAsAnError() async {
        struct Broken: ValueStoryClient {
            func home(range: TimeRange) async throws -> HomeSummary { throw RatchetClientError.unavailable("ratchetd is not running") }
            func valueOverview(range: TimeRange) async throws -> ValueOverview { throw RatchetClientError.unavailable("ratchetd is not running") }
        }
        let model = ValueStoryModel(client: Broken())
        await model.load()
        #expect(model.error == "ratchetd is not running" && model.content == .loading)
    }
}

@MainActor
@Suite("Optimization settings model")
struct SettingsModelTests {
    @Test func theRowsReadTheirTagsAndSaveWithASavedMark() async {
        let model = OptimizationSettingsModel(client: MockHarnessClient(), optimizationID: "o1", defaultOrder: [.claudeCode, .codex])
        await model.load()
        var rows = Dictionary(uniqueKeysWithValues: model.rows.map { ($0.key, $0) })
        #expect(rows[.agents]?.value == "Claude Code → Codex" && rows[.agents]?.tag == .usingDefault && rows[.agents]?.isEditable == true)
        #expect(rows[.value]?.tag == .custom && rows[.schedule]?.value == "Every night" && rows[.schedule]?.tag == .custom)
        #expect(rows.values.allSatisfy { !$0.isSaved } && rows[.agents]?.editTitle == "Edit")

        var editor = model.agentsEditor ?? AgentOrderEditor(order: [])
        editor.moveDown(at: 0)
        #expect(await model.saveAgents(editor))
        rows = Dictionary(uniqueKeysWithValues: model.rows.map { ($0.key, $0) })
        #expect(rows[.agents]?.value == "Codex → Claude Code" && rows[.agents]?.tag == .custom && rows[.agents]?.isSaved == true)
        #expect(rows[.value]?.isSaved == false)
    }

    @Test func aListInTheConfigCannotBeEditedAndSaysWhere() async {
        let model = OptimizationSettingsModel(client: MockHarnessClient(configLocked: ["o1"]), optimizationID: "o1")
        await model.load()
        let row = model.rows.first { $0.key == .agents }
        #expect(row?.isEditable == false && row?.lockedNote == "Set in ratchet.yaml: change it in the repo." && row?.tag == .custom)
        let refused = await model.saveAgents(AgentOrderEditor(order: [.codex]))
        #expect(!refused && model.errors[.agents]?.contains("ratchet.yaml") == true && model.savedRow == nil)
    }

    @Test func theValueEditorValidatesAndSaves() async {
        let model = OptimizationSettingsModel(client: MockHarnessClient(), optimizationID: "o1")
        await model.load()
        var editor = model.valueEditor
        #expect(editor.usdPerUnitText == "1200" && editor.unit == "ms" && editor.problem == nil)
        editor.usdPerUnitText = "$2,500.50"
        editor.period = .month
        #expect(editor.input?.usdPerUnit == 2500.5 && editor.problem == nil)
        #expect(await model.saveValue(editor) && model.savedRow == .value)
        editor.usdPerUnitText = "lots"
        #expect(editor.problem != nil)
        #expect(!(await model.saveValue(editor)) && model.errors[.value] != nil)
        #expect(await model.clearValue())
        #expect(model.rows.first { $0.key == .value }?.value == "Not set" && model.rows.first { $0.key == .value }?.tag == .usingDefault)
    }

    @Test func archiveStopsTheScheduleAndAsksFirst() async {
        let prompt = OptimizationSettingsModel.archivePrompt(title: "Checkout p95")
        #expect(prompt.title == "Archive “Checkout p95”?" && prompt.message == "Stops the schedule and keeps its history and wins." && prompt.confirm == "Archive")
        let model = OptimizationSettingsModel(client: MockHarnessClient(), optimizationID: "o1")
        await model.load()
        #expect(await model.archive())
        #expect(model.isArchived && model.schedule?.kind == nil && model.rows.first { $0.key == .schedule }?.value == "Not scheduled")
    }

    @Test func aRowThatFailsToLoadSaysSoAndTheOthersStillLoad() async {
        struct Partial: OptimizationSettingsClient {
            let inner = MockHarnessClient()
            func agentsPolicy(optimizationID: String) async throws -> AgentsPolicy { throw RatchetClientError.notFound("o9") }
            func setAgentsPolicy(optimizationID: String, _ input: AgentsPolicyInput) async throws -> AgentsPolicy { try await inner.setAgentsPolicy(optimizationID: optimizationID, input) }
            func valueModel(optimizationID: String) async throws -> ValueModel { try await inner.valueModel(optimizationID: optimizationID) }
            func setValueModel(optimizationID: String, _ input: ValueModelInput) async throws -> ValueModel { try await inner.setValueModel(optimizationID: optimizationID, input) }
            func clearValueModel(optimizationID: String) async throws -> ValueModel { try await inner.clearValueModel(optimizationID: optimizationID) }
            func schedule(optimizationID: String) async throws -> ScheduleSetting { try await inner.schedule(optimizationID: optimizationID) }
            func setSchedule(optimizationID: String, _ kind: ScheduleKind?) async throws -> ScheduleSetting { try await inner.setSchedule(optimizationID: optimizationID, kind) }
        }
        let model = OptimizationSettingsModel(client: Partial(), optimizationID: "o9")
        await model.load()
        #expect(model.errors[.agents] != nil && model.errors[.value] == nil && model.rows.first { $0.key == .schedule }?.value == "Every night")
    }
}

// MARK: The Agents page

@MainActor
@Suite("Agents page")
struct AgentsPageTests {
    private func page(_ preferences: MemoryHarnessPreferences = MemoryHarnessPreferences()) -> AgentsPageModel {
        AgentsPageModel(client: MockRatchetClient(now: epoch), preferences: preferences)
    }

    @Test func everyStatusThatIsNotOkHasOneFixButton() async {
        let model = page()
        await model.load()
        let logins = Dictionary(uniqueKeysWithValues: model.loginRows.map { ($0.harness, $0) })
        #expect(logins[.claudeCode]?.status == .signedIn && logins[.claudeCode]?.fix == nil)
        #expect(logins[.codex]?.status == .notInstalled && logins[.codex]?.fix == .redetect)
        #expect(logins[.grok]?.status == .notInstalled && logins[.grok]?.fix == .redetect)
        #expect(logins[.claudeCode]?.title == "Claude Code · your login" && logins[.codex]?.status.word == "Not installed")
        await model.redetect()  // Codex was installed meanwhile, and is signed out
        let after = model.loginRows.first { $0.harness == .codex }
        #expect(after?.status == .signedOut && after?.fix == .signIn(.codex) && after?.status.word == "Signed out")
        #expect(AgentFix.signIn(.codex).title == "Sign In" && AgentFix.addKey(.grok).title == "Add Key…" && AgentFix.redetect.title == "Re-detect")
        for row in model.rowsNeedingAFix { #expect(row.fix != nil && !row.status.isOK) }
        let noKey = model.keyRows.first { $0.status == .noKey }
        #expect(noKey?.fix == .addKey(noKey?.harness ?? .claudeCode) && noKey?.detail.hasSuffix("key · none yet") == true)
    }

    @Test func redetectFindsTheHarnessThatWasInstalledMeanwhile() async {
        let model = page()
        await model.load()
        #expect(model.catalog.detection(of: .codex)?.installed == false)
        await model.perform(.redetect)
        #expect(model.catalog.detection(of: .codex)?.installed == true && !model.isDetecting)
    }

    @Test func signInStartsTheFlowAndHandsTheCommandToTheApp() async {
        let model = page()
        await model.load()
        var asked: (Harness, String)?
        model.onSignIn = { asked = ($0, $1) }
        await model.perform(.signIn(.codex))
        #expect(asked?.0 == .codex && asked?.1 == "codex login")
        var key: Harness?
        model.onAddKey = { key = $0 }
        await model.perform(.addKey(.grok))
        #expect(key == .grok)
    }

    @Test func theChatDefaultMustBeReadyAndTheOrderKeepsTheChoiceAndAddsTheRest() async {
        let preferences = MemoryHarnessPreferences()
        let model = page(preferences)
        await model.load()
        #expect(model.chatDefault == .claudeCode)
        model.setChatDefault(.grok)  // not installed: refused
        #expect(model.chatDefault == .claudeCode && preferences.chatDefault == nil)
        model.setChatDefault(.claudeCode)
        #expect(preferences.chatDefault == .claudeCode && model.loginRows.first { $0.harness == .claudeCode }?.isChatDefault == true)
        #expect(model.orderSummary == "Claude Code · on this Mac")
        await model.redetect()
        preferences.optimizationOrder = [.codex, .claudeCode]
        #expect(model.optimizationOrder == [.claudeCode] || model.optimizationOrder == [.codex, .claudeCode])
        var editor = model.orderEditor
        editor.moveDown(at: 0)
        model.saveOrder(editor)
        #expect(preferences.optimizationOrder == editor.order)
    }

    @Test func theLoginRowsSayWhatEachOneCostsAndHowManyOptimizationsUseIt() async {
        let model = page()
        await model.load()
        model.usage = [.claudeCode: 6]
        let claude = model.loginRows.first { $0.harness == .claudeCode }
        #expect(claude?.detail.contains("6 optimizations use it") == true)
        model.usage = [.claudeCode: 1]
        #expect(model.loginRows.first { $0.harness == .claudeCode }?.detail.contains("1 optimization uses it") == true)
    }

    @Test func preferencesSurviveInTheUserDefaults() {
        let defaults = UserDefaults(suiteName: "ratchet-tests-\(UUID().uuidString)") ?? .standard
        let preferences = UserDefaultsHarnessPreferences(defaults: defaults)
        #expect(preferences.chatDefault == nil && preferences.optimizationOrder.isEmpty)
        preferences.chatDefault = .grok
        preferences.optimizationOrder = [.codex, .claudeCode]
        let again = UserDefaultsHarnessPreferences(defaults: defaults)
        #expect(again.chatDefault == .grok && again.optimizationOrder == [.codex, .claudeCode])
    }
}

// MARK: Run log harness events and keeping awake

@Suite("Run log harness events and power")
struct HarnessLogTests {
    private func line(_ id: Int64, _ text: String, kind: String = "status") -> AgentLogLine {
        AgentLogLine(id: id, at: epoch.addingTimeInterval(Double(id)), kind: kind, text: text)
    }

    @Test func theRunnersSentencesAreRecognised() {
        #expect(HarnessLogEvent(text: "Claude Code is rate limited; waiting 5 min for it to lift") == .wait(harness: .claudeCode, detail: "5 min"))
        #expect(HarnessLogEvent(text: "switching from Claude Code to Codex: Claude Code sign-in expired")
            == .switched(from: .claudeCode, to: .codex, cause: .authExpired, reason: "Claude Code sign-in expired"))
        #expect(HarnessLogEvent(text: "switching from Codex to Grok: Codex is rate limited")
            == .switched(from: .codex, to: .grok, cause: .rateLimited, reason: "Codex is rate limited"))
        #expect(HarnessLogEvent(text: "paused: no agent is available. Claude Code is signed out; checking again in 5 min")
            == .paused(reason: "No agent is available. Claude Code is signed out"))
        #expect(HarnessLogEvent(text: "resumed with Codex") == .resumed(harness: .codex))
        #expect(HarnessLogEvent(text: "steering note read") == nil && HarnessLogEvent(text: "resumed with Nobody") == nil)
    }

    @Test func theTitlesAndDetailsSayWhatHappened() {
        #expect(HarnessLogEvent.wait(harness: .claudeCode, detail: "5 min").title == "Waiting for Claude Code")
        #expect(HarnessLogEvent.wait(harness: .claudeCode, detail: "5 min").detail == "Claude Code is rate limited. Waiting 5 min for it to lift.")
        let switched = HarnessLogEvent.switched(from: .claudeCode, to: .codex, cause: .authExpired, reason: "Claude Code sign-in expired")
        #expect(switched.title == "Switched to Codex" && switched.needsAttention)
        #expect(!HarnessLogEvent.switched(from: .codex, to: .grok, cause: .rateLimited, reason: "x").needsAttention)
        let paused = HarnessLogEvent.paused(reason: "No agent is available.")
        #expect(paused.title == "Paused" && paused.detail.hasSuffix("It resumes by itself when an agent is back.") && paused.needsAttention)
        #expect(HarnessLogEvent.resumed(harness: .grok).title == "Resumed with Grok")
    }

    @Test func aLogYieldsItsHarnessEntriesTheActiveHarnessAndAnyPause() {
        var log = AgentLog()
        log.apply(AgentLogPage(lines: [
            line(1, "Claude Code is rate limited; waiting 5 min for it to lift"),
            line(2, "turn 4 started", kind: "message"),
            line(3, "switching from Claude Code to Codex: Claude Code is rate limited"),
            line(4, "resumed with Codex", kind: "tool"),  // not a status line: ignored
        ], cursor: 4))
        #expect(log.harnessEntries.map(\.id) == [1, 3] && log.activeHarness == .codex && log.pausedReason == nil)
        log.apply(AgentLogPage(lines: [line(5, "paused: no agent is available.; checking again in 5 min")], cursor: 5))
        #expect(log.pausedReason == "No agent is available." && log.activeHarness == .codex)
        log.apply(AgentLogPage(lines: [line(6, "resumed with Claude Code")], cursor: 6))
        #expect(log.pausedReason == nil && log.activeHarness == .claudeCode)
    }

    @Test func theMenuBarSaysItIsKeepingTheMacAwakeOnlyWhileTheDaemonHoldsTheAssertion() {
        #expect(PowerState().menuLine == nil && PowerState().menuDetail == nil)
        let awake = PowerState(keepingAwake: true, localOptimizationIDs: ["a", "b"])
        #expect(awake.menuLine == "Keeping your Mac awake" && awake.menuDetail == "While 2 optimizations run on this Mac.")
        #expect(PowerState(keepingAwake: true, localOptimizationIDs: ["a"]).menuDetail == "While 1 optimization runs on this Mac.")
        #expect(PowerState(keepingAwake: false, localOptimizationIDs: ["a"]).menuLine == nil)
    }

    @Test func runActiveCarriesKeepingAwakeThroughTheStreamAndTheReducer() throws {
        let message = SSEMessage(event: "run-active", data: #"{"type":"run-active","active":true,"optimizationIds":["o1"],"keepingAwake":true}"#)
        #expect(DaemonEvent.decode(message) == .runActive(active: true, optimizationIDs: ["o1"], keepingAwake: true))
        let old = SSEMessage(event: "run-active", data: #"{"type":"run-active","active":true,"optimizationIds":["o1"]}"#)
        #expect(DaemonEvent.decode(old) == .runActive(active: true, optimizationIDs: ["o1"], keepingAwake: false))
        let state = PowerState(keepingAwake: true, localOptimizationIDs: ["o1"])
        let reduction = EventReducer.reduce(LiveState(), .powerChanged(state))
        #expect(reduction.power == state && reduction.touched.isEmpty && !reduction.resync)
    }
}

@MainActor
@Suite("Keeping the Mac awake in the store")
struct PowerStoreTests {
    @Test func theStoreKeepsThePowerStateTheStreamReports() {
        let store = AppStore(client: MockRatchetClient(now: epoch))
        #expect(store.power.menuLine == nil)
        store.apply(.powerChanged(PowerState(keepingAwake: true, localOptimizationIDs: ["o1"])))
        #expect(store.power.menuLine == "Keeping your Mac awake" && store.power.localOptimizationIDs == ["o1"])
        store.apply(.powerChanged(PowerState()))
        #expect(store.power.menuLine == nil)
    }
}
