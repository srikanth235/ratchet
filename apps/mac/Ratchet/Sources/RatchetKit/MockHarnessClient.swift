import Foundation

/// What the sample value story looks like.
public enum ValueScenario: Sendable {
    /// Delivered and pending value, billed partly to a key.
    case wins
    /// A subscription-only setup: agents are included in the plan, spend is AWS only.
    case subscription
    /// Optimizations exist, none has a win yet.
    case noWins
    /// No optimization at all.
    case empty
}

/// A `HarnessClient` with sample answers, for tests, snapshots and demo mode. Settings are kept in memory and validated like
/// the daemon does; Home and the Overview are built from one story, so their numbers are the same by construction.
public actor MockHarnessClient: HarnessClient {
    private var policies: [String: AgentsPolicy] = [:]
    private var values: [String: ValueModel] = [:]
    private var schedules: [String: ScheduleSetting] = [:]
    private let scenario: ValueScenario
    private let configLocked: Set<String>
    private let now: Date

    public init(scenario: ValueScenario = .wins, configLocked: Set<String> = [], now: Date = Date(timeIntervalSince1970: 1_793_452_800)) {
        self.scenario = scenario
        self.configLocked = configLocked
        self.now = now
    }

    // MARK: Agents

    public func agentsPolicy(optimizationID: String) async throws -> AgentsPolicy {
        policies[optimizationID] ?? AgentsPolicy(harnesses: [.claudeCode, .codex], source: configLocked.contains(optimizationID) ? .config : .settings)
    }

    public func setAgentsPolicy(optimizationID: String, _ input: AgentsPolicyInput) async throws -> AgentsPolicy {
        if let problem = input.problem { throw RatchetClientError.invalid(problem) }
        if configLocked.contains(optimizationID) {
            throw RatchetClientError.invalid("ratchet.yaml has an agents section: change the list there.")
        }
        let policy = AgentsPolicy(
            harnesses: input.harnesses, waitLimitSeconds: input.waitLimitSeconds ?? AgentsPolicy.defaultWaitLimitSeconds,
            crashStrikes: input.crashStrikes ?? AgentsPolicy.defaultCrashStrikes,
            recheckSeconds: input.recheckSeconds ?? AgentsPolicy.defaultRecheckSeconds)
        policies[optimizationID] = policy
        return policy
    }

    // MARK: Value

    public func valueModel(optimizationID: String) async throws -> ValueModel {
        values[optimizationID] ?? ValueModel(
            isSet: true, metric: "http_req_duration.p95", lowerIsBetter: true, usdPerUnit: 1200, period: .year, unit: "ms",
            basis: .estimated, note: "ms saved × checkouts a day", summary: "$1.2k per ms a year · estimated · ms saved × checkouts a day")
    }

    public func setValueModel(optimizationID: String, _ input: ValueModelInput) async throws -> ValueModel {
        if let problem = input.problem { throw RatchetClientError.invalid(problem) }
        let summary = Money.short(input.usdPerUnit) + " per " + (input.unit ?? "unit") + " " + input.period.title + " · "
            + input.basis.rawValue + (input.note.map { " · " + $0 } ?? "")
        let model = ValueModel(
            isSet: true, metric: "http_req_duration.p95", lowerIsBetter: true, usdPerUnit: input.usdPerUnit, period: input.period,
            unit: input.unit, basis: input.basis, note: input.note, summary: summary)
        values[optimizationID] = model
        return model
    }

    public func clearValueModel(optimizationID: String) async throws -> ValueModel {
        values[optimizationID] = .unset
        return .unset
    }

    // MARK: Schedule

    public func schedule(optimizationID: String) async throws -> ScheduleSetting {
        schedules[optimizationID] ?? ScheduleSetting(kind: .nightly, runsNext30Days: 30)
    }

    public func setSchedule(optimizationID: String, _ kind: ScheduleKind?) async throws -> ScheduleSetting {
        let runs: Int?
        switch kind {
        case .none: runs = 0
        case .once?: runs = 1
        case .nightly?: runs = 30
        case .weekdays?: runs = 22
        case .weekly?: runs = 4
        case .onPush?: runs = nil
        }
        let setting = ScheduleSetting(kind: kind, runsNext30Days: runs)
        schedules[optimizationID] = setting
        return setting
    }

    // MARK: The value story

    private var story: ValueStory {
        switch scenario {
        case .wins:
            return ValueStory(
                delivered: ValueAmount(usdPerYear: 12_500, wins: 3), pending: ValueAmount(usdPerYear: 2_900, wins: 1, pullRequests: 1),
                readyToShip: ValueAmount(usdPerYear: 0, wins: 0), spentUSD: 184, returnMultiple: 68, paybackDays: 5.4,
                line: "$12.5k delivered + $2.9k pending · 68×")
        case .subscription:
            return ValueStory(
                delivered: ValueAmount(usdPerYear: 9_000, wins: 2), pending: ValueAmount(usdPerYear: 0, wins: 0, pullRequests: 0),
                spentUSD: 121, returnMultiple: 74, paybackDays: 4.9, line: "$9.0k delivered · 74×")
        case .noWins, .empty:
            return ValueStory()
        }
    }

    private var spent: SpendDetail {
        switch scenario {
        case .wins:
            return SpendDetail(usd: 184, budgetUSD: 500, awsUSD: 121, agentsAPIUSD: 63, includedTokens: 0, detail: "AWS $121 · agents $63")
        case .subscription:
            return SpendDetail(
                usd: 121, budgetUSD: 500, awsUSD: 121, agentsAPIUSD: 0, includedTokens: 2_400_000,
                detail: "AWS $121 · agents included in your plan (2.4M tokens)")
        case .noWins:
            return SpendDetail(usd: 38, budgetUSD: 500, awsUSD: 38, detail: "AWS $38 · agents included in your plan")
        case .empty:
            return SpendDetail()
        }
    }

    private var forecast: ValueForecast {
        scenario == .empty
            ? ValueForecast()
            : ValueForecast(
                scheduledRuns: 30, expectedUSD: 1140, expectedAWSUSD: 1140, hardCapUSD: 1500, line: "30 runs · ~$1.1k · cap $1.5k")
    }

    private var phase: OverviewPhase {
        switch scenario {
        case .wins, .subscription: .wins
        case .noWins: .noWins
        case .empty: .empty
        }
    }

    public func home(range: TimeRange) async throws -> HomeSummary {
        HomeSummary(
            range: range, isEmpty: scenario == .empty, story: story, optimizations: scenario == .empty ? 0 : 3,
            openPullRequests: story.pending.pullRequests, mergedPullRequests: story.delivered.wins > 0 ? 2 : 0,
            failed: scenario == .wins ? HomeFailure(optimizationID: "opt-cold-start", title: "CLI cold start", detail: "AWS session expired") : nil)
    }

    public func valueOverview(range: TimeRange) async throws -> ValueOverview {
        let state: EmptyStateText?
        switch scenario {
        case .empty: state = EmptyStateText(title: "No wins yet", body: "Start your first optimization.")
        case .noWins: state = EmptyStateText(title: "No wins yet", body: "Runs are in progress; a verified win appears here.")
        default: state = nil
        }
        var points: [ValueChartPoint] = []
        if phase == .wins {
            let delivered = story.delivered.usdPerYear
            let pending = story.pending.usdPerYear
            for step in 0...4 {
                let share = Double(step) / 4
                points.append(ValueChartPoint(
                    at: now.addingTimeInterval(Double(step - 4) * 86_400 * 7), spentUSD: story.spentUSD * share,
                    deliveredUSDPerYear: step >= 2 ? delivered * share : 0, pendingUSDPerYear: step == 4 ? pending : 0))
            }
        }
        return ValueOverview(range: range, phase: phase, emptyState: state, story: story, spent: spent, forecast: forecast, chart: points)
    }
}
