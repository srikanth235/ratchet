import Foundation

/// The JSON of the P6 and P7 routes (`/v1/optimizations/{id}/agents|value|schedule`, `/v1/home`, the story part of
/// `/v1/overview`), written by hand until the generated client holds them; thin on purpose (decoding and mapping only).
enum HarnessWire {
    // MARK: Agents policy

    struct Policy: Decodable {
        var harnesses: [String]
        var waitLimitS: Int?
        var crashStrikes: Int?
        var recheckS: Int?
        var source: String?
        var summary: String?
    }

    static func policy(_ wire: Policy) -> AgentsPolicy {
        AgentsPolicy(
            harnesses: wire.harnesses.compactMap { Harness(rawValue: $0) },
            waitLimitSeconds: wire.waitLimitS ?? AgentsPolicy.defaultWaitLimitSeconds,
            crashStrikes: wire.crashStrikes ?? AgentsPolicy.defaultCrashStrikes,
            recheckSeconds: wire.recheckS ?? AgentsPolicy.defaultRecheckSeconds,
            source: AgentsPolicySource(rawValue: wire.source ?? "") ?? .settings, summary: wire.summary)
    }

    static func policyBody(_ input: AgentsPolicyInput) throws -> Data {
        var fields: [String: Any] = ["harnesses": input.harnesses.map(\.rawValue)]
        if let value = input.waitLimitSeconds { fields["waitLimitS"] = value }
        if let value = input.crashStrikes { fields["crashStrikes"] = value }
        if let value = input.recheckSeconds { fields["recheckS"] = value }
        return try DaemonTransport.jsonBody(fields)
    }

    // MARK: Value model

    struct Value: Decodable {
        var set: Bool
        var metric: String?
        var direction: String?
        var usdPerUnit: Double?
        var period: String?
        var unit: String?
        var basis: String?
        var note: String?
        var summary: String?
    }

    static func value(_ wire: Value) -> ValueModel {
        ValueModel(
            isSet: wire.set, metric: wire.metric ?? "", lowerIsBetter: wire.direction != "maximize", usdPerUnit: wire.usdPerUnit,
            period: wire.period.flatMap { ValuePeriod(rawValue: $0) }, unit: wire.unit,
            basis: wire.basis.flatMap { ValueBasisKind(rawValue: $0) }, note: wire.note, summary: wire.summary ?? (wire.set ? "" : "Not set"))
    }

    static func valueBody(_ input: ValueModelInput) throws -> Data {
        var fields: [String: Any] = ["usdPerUnit": input.usdPerUnit, "period": input.period.rawValue, "basis": input.basis.rawValue]
        if let unit = input.unit, !unit.isEmpty { fields["unit"] = unit }
        if let note = input.note, !note.isEmpty { fields["note"] = note }
        return try DaemonTransport.jsonBody(fields)
    }

    // MARK: Schedule

    struct Schedule: Decodable {
        var schedule: String?
        var summary: String?
        var runsNext30Days: Int?
    }

    static func schedule(_ wire: Schedule) -> ScheduleSetting {
        ScheduleSetting(kind: wire.schedule.flatMap { ScheduleKind(rawValue: $0) }, summary: wire.summary, runsNext30Days: wire.runsNext30Days)
    }

    static func scheduleBody(_ kind: ScheduleKind?) throws -> Data {
        try DaemonTransport.jsonBody(["schedule": kind?.rawValue ?? NSNull()])
    }

    // MARK: The value story

    struct Amount: Decodable {
        var usdPerYear: Double?
        var wins: Int?
        var pullRequests: Int?
    }

    struct Story: Decodable {
        var basis: String?
        var delivered: Amount?
        var pending: Amount?
        var readyToShip: Amount?
        var spentUsd: Double?
        var `return`: Double?
        var paybackDays: Double?
        var line: String?
    }

    struct Spent: Decodable {
        var usd: Double?
        var budgetUsd: Double?
        var awsUsd: Double?
        var agentsApiUsd: Double?
        var includedTokens: Int?
        var detail: String?
    }

    struct Forecast: Decodable {
        var days: Int?
        var scheduledRuns: Int?
        var unforecast: Int?
        var expectedUsd: Double?
        var expectedAwsUsd: Double?
        var expectedAgentsApiUsd: Double?
        var expectedIncludedTokens: Int?
        var hardCapUsd: Double?
        var line: String?
    }

    struct ChartPoint: Decodable {
        var at: Int64
        var spentUsd: Double?
        var deliveredUsdPerYear: Double?
        var pendingUsdPerYear: Double?
        var readyUsdPerYear: Double?
    }

    struct EmptyState: Decodable {
        var title: String
        var body: String
    }

    struct Need: Decodable {
        var optimizationId: String
        var title: String
        var sub: String?
    }

    struct PullRequests: Decodable {
        var open: Int?
        var merged: Int?
    }

    /// The parts of `GET /v1/overview` this layer reads.
    struct Overview: Decodable {
        var phase: String?
        var emptyState: EmptyState?
        var story: Story?
        var spent: Spent?
        var forecast: Forecast?
        var chart: [ChartPoint]?
    }

    struct Home: Decodable {
        var empty: Bool?
        var story: Story?
        var optimizations: Int?
        var pullRequests: PullRequests?
        var failed: Need?
    }

    static func date(ms: Int64) -> Date { Date(timeIntervalSince1970: Double(ms) / 1000) }

    static func amount(_ wire: Amount?) -> ValueAmount {
        ValueAmount(usdPerYear: wire?.usdPerYear ?? 0, wins: wire?.wins ?? 0, pullRequests: wire?.pullRequests ?? 0)
    }

    static func story(_ wire: Story?) -> ValueStory {
        ValueStory(
            basis: wire?.basis.flatMap { ValueBasisKind(rawValue: $0) } ?? .estimated, delivered: amount(wire?.delivered),
            pending: amount(wire?.pending), readyToShip: amount(wire?.readyToShip), spentUSD: wire?.spentUsd ?? 0,
            returnMultiple: wire?.return, paybackDays: wire?.paybackDays, line: wire?.line ?? "")
    }

    static func overview(_ wire: Overview, range: TimeRange) -> ValueOverview {
        let spent = wire.spent
        let forecast = wire.forecast
        return ValueOverview(
            range: range, phase: wire.phase.flatMap { OverviewPhase(rawValue: $0) } ?? .wins,
            emptyState: wire.emptyState.map { EmptyStateText(title: $0.title, body: $0.body) }, story: story(wire.story),
            spent: SpendDetail(
                usd: spent?.usd ?? 0, budgetUSD: spent?.budgetUsd ?? 0, awsUSD: spent?.awsUsd ?? 0, agentsAPIUSD: spent?.agentsApiUsd ?? 0,
                includedTokens: spent?.includedTokens ?? 0, detail: spent?.detail ?? ""),
            forecast: ValueForecast(
                days: forecast?.days ?? 30, scheduledRuns: forecast?.scheduledRuns ?? 0, unforecast: forecast?.unforecast ?? 0,
                expectedUSD: forecast?.expectedUsd ?? 0, expectedAWSUSD: forecast?.expectedAwsUsd ?? 0,
                expectedAgentsAPIUSD: forecast?.expectedAgentsApiUsd ?? 0, expectedIncludedTokens: forecast?.expectedIncludedTokens ?? 0,
                hardCapUSD: forecast?.hardCapUsd, line: forecast?.line ?? ""),
            chart: (wire.chart ?? []).map {
                ValueChartPoint(
                    at: date(ms: $0.at), spentUSD: $0.spentUsd ?? 0, deliveredUSDPerYear: $0.deliveredUsdPerYear ?? 0,
                    pendingUSDPerYear: $0.pendingUsdPerYear ?? 0, readyUSDPerYear: $0.readyUsdPerYear ?? 0)
            })
    }

    static func home(_ wire: Home, range: TimeRange) -> HomeSummary {
        HomeSummary(
            range: range, isEmpty: wire.empty ?? false, story: story(wire.story), optimizations: wire.optimizations ?? 0,
            openPullRequests: wire.pullRequests?.open ?? 0, mergedPullRequests: wire.pullRequests?.merged ?? 0,
            failed: wire.failed.map { HomeFailure(optimizationID: $0.optimizationId, title: $0.title, detail: $0.sub ?? "") })
    }

    /// The daemon's name of a range: `30d`, `90d`, `all`.
    static func rangeParameter(_ range: TimeRange) -> String {
        switch range {
        case .days30: "30d"
        case .days90: "90d"
        case .all: "all"
        }
    }
}
