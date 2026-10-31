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

    static func needsYou(_ item: VM.OverviewNeed) -> NeedsYouItem {
        // `ship`: verified wins waiting for a pull request; `failed`: the run failed.
        NeedsYouItem(
            id: item.id, kind: item.kind == .ship ? .mergeWins : .runFailed, title: item.title, detail: item.sub,
            actionTitle: item.action, optimizationID: item.optimizationId)
    }

    /// The staircase starts at nothing: the value locked in over a range is the sum of the wins inside it.
    static func overview(_ vm: VM.OverviewVm) -> Overview {
        let wins = vm.wins.sorted { $0.at < $1.at }
        var series: [ValuePoint] = [ValuePoint(id: "start", date: date(ms: vm.from), valuePerYearUSD: 0)]
        for (index, win) in wins.enumerated() {
            series.append(ValuePoint(
                id: "win-\(index)-\(win.at)", date: date(ms: win.at), valuePerYearUSD: win.cumulativeUsdPerYear,
                win: "Verified win", optimizationTitle: win.optimization))
        }
        series.append(ValuePoint(id: "end", date: date(ms: vm.to), valuePerYearUSD: wins.last?.cumulativeUsdPerYear ?? 0))
        return Overview(
            range: timeRange(vm.range), spentUSD: vm.spent.usd, budgetUSD: vm.spent.budgetUsd,
            valueLockedPerYearUSD: vm.value?.usdPerYear ?? 0, winsCount: Int(vm.value?.wins ?? 0), series: series,
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
        switch vm {
        case .case1(let github):
            return Connector(
                id: github.id, kind: .github, name: github.name, detail: github.detail,
                state: state(github.status, kind: .github), isDefault: github.isDefault)
        case .case2(let aws):
            return Connector(
                id: aws.id, kind: .aws, name: aws.name, detail: aws.detail, state: state(aws.status, kind: .aws),
                isDefault: aws.isDefault, monthlyCapUSD: aws.monthlyCapUsd)
        case .case3(let agent):
            return Connector(
                id: agent.id, kind: .agent, name: agent.name, detail: agent.detail,
                state: state(agent.status, kind: .agent), isDefault: agent.isDefault,
                canRunInCloud: agent.runsOn.contains(.cloud))
        }
    }

    static func githubFlow(_ vm: VM.GithubFlowVm?) -> GithubSignInFlow? {
        guard let vm else { return nil }
        let state: GithubSignInFlow.State
        switch vm.state {
        case .pending: state = .pending
        case .expired: state = .expired
        case .denied: state = .denied
        case .error: state = .error
        }
        return GithubSignInFlow(
            state: state, userCode: vm.userCode, verificationURL: vm.verificationUri,
            expiresAt: date(ms: vm.expiresAt), message: vm.message)
    }

    static func connectorID(_ vm: VM.ConnectorVm) -> String {
        switch vm {
        case .case1(let github): github.id
        case .case2(let aws): aws.id
        case .case3(let agent): agent.id
        }
    }

    /// `connected` reads as green on every kind; the sheet words it per kind (an AWS profile is "valid", an agent "signed in").
    static func state(_ status: VM.ConnectorStatus, kind: ConnectorKind) -> ConnectorState {
        switch status {
        case .connected:
            switch kind {
            case .github: .connected
            case .aws: .valid
            case .agent: .signedIn
            }
        case .valid: .valid
        case .expired, .invalid: .expired
        case .signedOut: .signedOut
        }
    }

    static func placement(_ where_: VM.AgentsWhere) -> Placement {
        where_ == .cloud ? .cloud : .local
    }

    static func agentsWhere(_ placement: Placement) -> VM.AgentsWhere {
        placement == .cloud ? .cloud : .local
    }

    /// What was saved for the repo; nil when nothing usable was.
    static func selection(_ vm: VM.RepoConnectionsVm) -> ConnectionSelection? {
        guard let saved = vm.saved else { return nil }
        var agents: [String: Placement] = [:]
        for agent in saved.agents { agents[agent.connectorId] = placement(agent._where) }
        return ConnectionSelection(repoID: vm.repo, awsID: saved.aws, agents: agents, githubID: saved.github)
    }

    static func repoConnections(_ selection: ConnectionSelection, github: String, aws: String) -> VM.RepoConnections {
        VM.RepoConnections(
            agents: selection.agents.sorted { $0.key < $1.key }.map {
                VM.AgentSelection(connectorId: $0.key, _where: agentsWhere($0.value))
            },
            aws: aws, github: github)
    }

    // MARK: Environment drafts

    static func draft(_ vm: VM.EnvironmentDraftVm) -> EnvironmentDraft {
        // The proposal is absent only while the first draft is being made; callers wait for it (`settledDraft`).
        let proposal = vm.draft
        let scope: EnvironmentScope
        switch proposal?.scope {
        case .single: scope = .single
        case .full: scope = .full
        default: scope = .none
        }
        // The agent's reason is one text: its first sentence is the headline, the rest is the note under it.
        let reasonText = proposal?.reason ?? ""
        let cut = reasonText.firstIndex { ".!?".contains($0) }.map { reasonText.index(after: $0) } ?? reasonText.endIndex
        let headline = String(reasonText[..<cut])
        let note = reasonText[cut...].trimmingCharacters(in: .whitespaces)
        let phrases = ["full stack", "one machine", "single machine", "no environment"]
        let emphasis = phrases.first { headline.lowercased().contains($0) } ?? ""
        let checks = (proposal?.checks ?? []).map { check in
            check.label.prefix(1).lowercased() + check.label.dropFirst()
        }
        return EnvironmentDraft(
            id: vm.id, repoID: vm.repo, scope: scope, state: vm.status == .approved ? .approved : .draft,
            revision: Int(vm.revision), headline: headline, emphasis: emphasis, reason: note,
            draftedBy: proposal.map { title(of: $0.agent) } ?? "", draftSeconds: Double(proposal?.durationS ?? 0),
            draftCostUSD: proposal?.usdSpent ?? 0, readyMinutes: Double(proposal?.cost.startupMinutes ?? 0),
            resources: (proposal?.resources ?? []).enumerated().map { index, r in
                DraftResource(id: "\(index)-\(r.name)", name: r.name, detail: r.detail, machine: r.machine, usdPerHour: r.usdPerHour)
            },
            files: (proposal?.files ?? []).map(\.path), checks: checks.joined(separator: ", "))
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
