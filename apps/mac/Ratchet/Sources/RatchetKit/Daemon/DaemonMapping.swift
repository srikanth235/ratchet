import Foundation
import RatchetAPI

// The view models of `ratchetd` (generated into RatchetAPI) to the models of the app (Models.swift). Pure functions,
// so the tests run them on JSON from the daemon without a connection.

typealias VM = Components.Schemas

enum DaemonMapping {
    // MARK: Small helpers

    /// The daemon sends epoch milliseconds.
    static func date(ms: Int64) -> Date {
        Date(timeIntervalSince1970: Double(ms) / 1000)
    }

    static func title(of kind: VM.AgentKind) -> String {
        switch kind {
        case .claudeCode: "Claude Code"
        case .codex: "Codex"
        case .mock: "Sample agent"
        }
    }

    /// The pull request number at the end of a GitHub URL: "…/pull/214" is 214.
    static func pullRequestNumber(_ url: String?) -> Int? {
        guard let url, let last = url.split(separator: "/").last else { return nil }
        return Int(last)
    }

    // MARK: Optimizations

    /// How the daemon's lifecycle reads in the app: what runs, what waits for you, what ended with or without a win.
    static func status(_ vm: VM.OptimizationVm) -> OptimizationStatus {
        switch vm.status {
        case .draft: .waiting
        case .baselining, .running, .stopping: .running
        case .completed, .stopped, .failed, .prOpen: vm.verifiedWins > 0 ? .done : .noWin
        }
    }

    /// "http_req_duration.p95" is shown as "p95", "events_per_s" as "events per s".
    static func metricName(_ path: String) -> String {
        (path.split(separator: ".").last.map(String.init) ?? path).replacingOccurrences(of: "_", with: " ")
    }

    /// Decimals that keep about three significant digits of the numbers the metric takes.
    static func decimals(_ values: [Double]) -> Int {
        let magnitude = values.map { abs($0) }.max() ?? 0
        if magnitude >= 100 || magnitude == 0 { return 0 }
        if magnitude >= 10 { return 1 }
        if magnitude >= 1 { return 2 }
        return 3
    }

    static func metric(_ vm: VM.OptimizationVm) -> MetricSpec {
        let values = [vm.baseline, vm.best].compactMap { $0 }
        return MetricSpec(
            name: metricName(vm.metric), unit: vm.unit, decimals: decimals(values), lowerIsBetter: vm.direction == .lower)
    }

    /// A row of the optimizations table. `detail`, when fetched, adds the clock, the pull request and the noise.
    static func optimization(_ vm: VM.OptimizationVm, detail: VM.OptimizationDetail? = nil) -> Optimization {
        let baseline = vm.baseline ?? 0
        var secondsLeft: Double?
        var timeLimitHours = 2.0
        var startedAt: Date?
        var noise: Double?
        var prNumber: Int?
        if let detail {
            timeLimitHours = Double(detail.timeLimitS) / 3600
            startedAt = detail.startedAt.map(date(ms:))
            if status(vm) == .running { secondsLeft = max(Double(detail.timeLimitS) - detail.elapsedS, 0) }
            if let floor = detail.noiseFloor, baseline != 0 { noise = abs(floor / baseline) }
            prNumber = pullRequestNumber(detail.prUrl)
        }
        return Optimization(
            id: vm.id, title: vm.name, repo: vm.repo, branch: detail?.baseRef ?? "main", goal: "",
            status: status(vm), metric: metric(vm), baseline: baseline, best: vm.best, noise: noise,
            spentUSD: vm.spendUsd, budgetUSD: vm.budgetUsd > 0 ? vm.budgetUsd : nil,
            valuePerYearUSD: vm.value?.usdPerYear, secondsLeft: secondsLeft, timeLimitHours: timeLimitHours,
            startedAt: startedAt ?? (vm.createdAt > 0 ? date(ms: vm.createdAt) : nil), prNumber: prNumber,
            winsCount: Int(vm.verifiedWins), evalsCount: Int(vm.candidateCount), agentsCount: vm.agents.count)
    }

    static func agent(_ vm: VM.AgentVm) -> Agent {
        let state: AgentState
        switch vm.status {
        case .pending, .launching, .running: state = .working
        case .paused: state = .paused
        case .stopped, .done, .failed: state = .idle
        }
        let activity: String
        if let hypothesis = vm.hypothesis, !hypothesis.isEmpty {
            activity = hypothesis
        } else {
            activity = state == .idle ? "Finished" : "Reading the repo"
        }
        return Agent(
            id: vm.id, name: title(of: vm.kind), placement: vm.runnerInstanceId == nil ? .local : .cloud, state: state,
            activity: activity, evals: Int(vm.stats.candidates), wins: Int(vm.stats.verified), spentUSD: vm.stats.spendUsd)
    }

    static func verdict(_ vm: VM.CandidateVerdict) -> Verdict {
        switch vm {
        case .pending: .evaluating
        case .verified: .verified
        case .rejected, .failedConfirm: .noise
        case .blocked: .blocked
        }
    }

    static func candidate(_ vm: VM.CandidateVm) -> Candidate {
        let firstLine = vm.message.split(whereSeparator: \.isNewline).first.map(String.init) ?? vm.message
        return Candidate(
            id: String(vm.sha.prefix(7)), title: firstLine, agent: title(of: vm.agent), verdict: verdict(vm.verdict),
            value: vm.benchValue ?? vm.confirmValue, at: date(ms: vm.submittedAt))
    }

    /// One move per step after the baseline; `start` is when the run began (the series counts minutes from there).
    /// The title is the label of the verified candidate that reached the step's value ("67b05fc · Batch writes" is
    /// shown as "Batch writes").
    static func moves(_ series: VM.RatchetSeries, optimizationID: String, start: Date) -> [RatchetMove] {
        series.steps.dropFirst().enumerated().map { index, step in
            let label = series.points.first { $0.verified && abs($0.value - step.value) < 1e-9 }?.label
            var title = label ?? ""
            if let separator = title.range(of: " · ") { title = String(title[separator.upperBound...]) }
            return RatchetMove(
                id: "\(optimizationID)-move-\(index + 1)", at: start.addingTimeInterval(step.t * 60), value: step.value,
                title: title.isEmpty ? "Verified win" : title)
        }
    }

    // MARK: Overview

    static func range(_ range: TimeRange) -> VM.OverviewRange {
        switch range {
        case .days30: ._30d
        case .days90: ._90d
        case .all: .all
        }
    }

    static func timeRange(_ range: VM.OverviewRange) -> TimeRange {
        switch range {
        case ._30d: .days30
        case ._90d: .days90
        case .all: .all
        }
    }

    static func needsYou(_ item: VM.NeedsYouItem) -> NeedsYouItem {
        let kind: NeedsYouKind
        switch item.kind {
        case .approveEnvironment: kind = .approveEnvironment
        case .mergeWins: kind = .mergeWins
        case .signIn: kind = .signIn
        case .budgetLimit: kind = .budgetLimit
        }
        return NeedsYouItem(
            id: item.id, kind: kind, title: item.title, detail: item.subtitle, actionTitle: item.actionLabel,
            optimizationID: item.optimizationId ?? "")
    }

    /// `budgetUSD` is the cap of the overview, or the sum of the optimization budgets when the daemon has no cap.
    static func overview(_ vm: VM.OverviewVm, budgetUSD: Double) -> Overview {
        let from = date(ms: vm.staircase.from)
        let to = date(ms: vm.staircase.to)
        let wins = vm.staircase.points.sorted { $0.at < $1.at }
        let timeRange = timeRange(vm.range)
        var series: [ValuePoint] = []
        // Before the first win of the window the value is not known; "all time" starts at nothing.
        series.append(ValuePoint(id: "start", date: from, valuePerYearUSD: timeRange == .all ? 0 : (wins.first?.usdPerYear ?? 0)))
        for (index, point) in wins.enumerated() {
            series.append(ValuePoint(
                id: "win-\(index)-\(point.at)", date: date(ms: point.at), valuePerYearUSD: point.usdPerYear,
                win: "Verified win", optimizationTitle: point.optimizationName))
        }
        series.append(ValuePoint(id: "end", date: to, valuePerYearUSD: wins.last?.usdPerYear ?? 0))
        return Overview(
            range: timeRange, spentUSD: vm.spent.usd, budgetUSD: vm.spent.capUsd ?? budgetUSD,
            valueLockedPerYearUSD: vm.valueLockedIn.usdPerYear, winsCount: Int(vm.valueLockedIn.wins), series: series,
            needsYou: vm.needsYou.map(needsYou))
    }

    // MARK: Templates and repos

    static func template(_ vm: VM.TemplateInfo) -> Template {
        Template(
            id: vm.id, kind: vm.kind == .demo ? .demo : .template, category: vm.category.rawValue, title: vm.title,
            summary: vm.description, measures: vm.measures, machine: vm.machine)
    }

    static func repo(_ vm: VM.RepoVm) -> Repo {
        let account = vm.fullName.split(separator: "/").first.map(String.init) ?? vm.fullName
        return Repo(id: vm.fullName, defaultBranch: vm.defaultBranch, account: account)
    }

    static func harness(repoID: String, _ vm: VM.RepoInspectVm) -> HarnessDetection {
        if let config = vm.config {
            let summary = "Found ratchet.yaml: " + metricName(config.metric) + ", "
                + (config.direction == .minimize ? "lower" : "higher") + " is better."
            return HarnessDetection(repoID: repoID, found: true, summary: summary)
        }
        if let problem = vm.problem, !problem.isEmpty {
            return HarnessDetection(repoID: repoID, found: false, summary: problem, suggestedTemplateID: vm.templates.first?.id)
        }
        return HarnessDetection(
            repoID: repoID, found: false, summary: "No benchmark found in this repo. Start from a template.",
            suggestedTemplateID: vm.templates.first?.id)
    }

    // MARK: Connectors

    static func connector(_ vm: VM.ConnectorVm) -> Connector {
        let state: ConnectorState
        switch vm.status {
        case .connected:
            switch vm.kind {
            case .github: state = .connected
            case .aws: state = .valid
            case .agent: state = .signedIn
            }
        case .expired, .invalid: state = .expired
        case .signedOut, .unknown: state = .signedOut
        }
        let cap = vm.aws?.capUsdPerMonth
        let remaining = vm.aws?.remainingUsdThisMonth
        let used = (cap != nil && remaining != nil) ? max((cap ?? 0) - (remaining ?? 0), 0) : 0
        return Connector(
            id: vm.id, kind: kind(vm.kind), name: vm.name, detail: vm.detail, state: state, isDefault: vm.isDefault,
            monthlyCapUSD: cap, usedThisMonthUSD: used, canRunInCloud: vm.agent?.runsIn.contains(.cloud) ?? false)
    }

    static func kind(_ kind: VM.ConnectorKind) -> ConnectorKind {
        switch kind {
        case .github: .github
        case .aws: .aws
        case .agent: .agent
        }
    }

    static func placement(_ where_: VM.AgentsWhere) -> Placement {
        where_ == .cloud ? .cloud : .local
    }

    static func agentsWhere(_ placement: Placement) -> VM.AgentsWhere {
        placement == .cloud ? .cloud : .local
    }

    static func selection(_ vm: VM.ConnectorSelectionVm) -> ConnectionSelection {
        var agents: [String: Placement] = [:]
        for agent in vm.selection.agents { agents[agent.connectorId] = placement(agent.runsIn) }
        return ConnectionSelection(repoID: vm.repo, awsID: vm.selection.awsConnectorId, agents: agents)
    }

    static func selectionInput(_ selection: ConnectionSelection) -> VM.ConnectorSelectionInput {
        VM.ConnectorSelectionInput(
            agents: selection.agents.sorted { $0.key < $1.key }.map {
                VM.AgentSelection(connectorId: $0.key, runsIn: agentsWhere($0.value))
            },
            awsConnectorId: selection.awsID)
    }

    // MARK: Environment drafts

    static func draft(_ vm: VM.EnvironmentDraftVm) -> EnvironmentDraft {
        let scope: EnvironmentScope
        switch vm.scope {
        case .none: scope = .none
        case .single: scope = .single
        case .full: scope = .full
        }
        let phrases = ["full stack", "one machine", "single machine", "no environment"]
        let emphasis = phrases.first { vm.summary.lowercased().contains($0) } ?? ""
        let checks = vm.checks.map { check in
            check.label.prefix(1).lowercased() + check.label.dropFirst()
        }
        return EnvironmentDraft(
            id: vm.id, repoID: vm.repo, scope: scope, state: vm.status == .approved ? .approved : .draft,
            revision: Int(vm.revision), headline: vm.summary, emphasis: emphasis, reason: vm.rationale,
            draftedBy: title(of: vm.provenance.agent), draftSeconds: Double(vm.provenance.durationS),
            draftCostUSD: vm.provenance.costUsd, readyMinutes: Double(vm.cost.readyMinutes),
            resources: vm.resources.map {
                DraftResource(id: $0.id, name: $0.name, detail: $0.description, machine: $0.machine, usdPerHour: $0.hourlyUsd)
            },
            files: vm.filePaths, checks: checks.joined(separator: ", "))
    }

    // MARK: Sandbox

    private static let activePhases: Set<VM.EnvironmentPhase> = [.creating, .ready, .idle]

    static func sandbox(_ vm: VM.EnvironmentsVm) -> SandboxStatus {
        let active = vm.environments.filter { activePhases.contains($0.phase) }
        guard !active.isEmpty else { return .off }
        let offAt = active.compactMap(\.destroyAt).min().map(date(ms:))
        return SandboxStatus(
            isOn: true, account: active.first?.region ?? active.first?.sandboxId,
            costSoFarUSD: active.reduce(0) { $0 + $1.spendUsd }, offAt: offAt)
    }

    /// The sandbox that "End now" ends: the first one that is up.
    static func endableSandboxID(_ vm: VM.EnvironmentsVm) -> String? {
        vm.environments.first { activePhases.contains($0.phase) }?.sandboxId
    }

    static func receipt(_ vm: VM.TeardownReceiptVm, startedAt: Date? = nil) -> TeardownReceipt {
        var destroyed: [String] = []
        if vm.removed.instances > 0 { destroyed.append(Plural.count(Int(vm.removed.instances), "instance")) }
        if vm.removed.agentIdentities > 0 {
            destroyed.append(Plural.count(Int(vm.removed.agentIdentities), "agent identity", plural: "agent identities"))
        }
        if vm.removed.secrets > 0 { destroyed.append(Plural.count(Int(vm.removed.secrets), "secret")) }
        destroyed.append(contentsOf: vm.removed.resources)
        let end = date(ms: vm.destroyedAt)
        return TeardownReceipt(
            id: vm.sandboxId, account: vm.region, startedAt: startedAt ?? end, endedAt: end, costUSD: vm.spendUsd,
            destroyed: destroyed, allGone: vm.nothingRunning && vm.orphans.isEmpty && vm.errors.isEmpty)
    }
}
