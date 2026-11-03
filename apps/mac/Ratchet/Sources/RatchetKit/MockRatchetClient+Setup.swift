import Foundation

// Setting up an optimization in the sample world: repos, inspection, preflight, template pull requests, drafts, overrides.

extension MockRatchetClient {
    // MARK: Repos

    public func repos() async throws -> [Repo] { world.repos }

    public func detectBenchmark(repoID: String) async throws -> BenchmarkDetection {
        guard world.repos.contains(where: { $0.id == repoID }) else { throw RatchetClientError.notFound("repo \(repoID)") }
        switch repoID {
        case "acme-org/shop-api":
            return BenchmarkDetection(repoID: repoID, found: true,
                                    summary: "Found a k6 script in bench/ and a docker-compose.yml. Looks like an HTTP latency test.",
                                    suggestedTemplateID: "http-latency")
        case "srikanth235/acme-cli":
            return BenchmarkDetection(repoID: repoID, found: true, summary: "Found a hyperfine script in bench/.",
                                    suggestedTemplateID: "cli-cold-start")
        default:
            return BenchmarkDetection(repoID: repoID, found: false,
                                    summary: "No benchmark found in this repo. Start from a template.", suggestedTemplateID: "build-size")
        }
    }

    // MARK: Setup: repos, preflight, template pull requests

    public func searchRepos(query: String) async throws -> [Repo] {
        let text = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !text.isEmpty else { return world.repos }
        return world.repos.filter { $0.id.lowercased().contains(text) }
    }

    public func inspectRepo(_ source: RepoSource) async throws -> RepoInspection {
        switch source {
        case .local(let path):
            if path.contains("no-remote") {
                return RepoInspection(
                    source: source, repo: nil, baseRef: "main",
                    problem: "This folder has no GitHub remote, so Ratchet cannot open a pull request for it.")
            }
            let name = path.split(separator: "/").last.map(String.init) ?? "folder"
            return Self.foundInspection(source: source, repo: "srikanth235/" + name, suggested: nil)
        case .github(let repo):
            guard world.repos.contains(where: { $0.id == repo }) else { throw RatchetClientError.notFound("repo \(repo)") }
            switch repo {
            case "acme-org/shop-api": return Self.foundInspection(source: source, repo: repo, suggested: "http-latency")
            case "srikanth235/acme-cli": return Self.foundInspection(source: source, repo: repo, suggested: "cli-cold-start")
            case "acme-org/search-svc":
                return RepoInspection(
                    source: source, repo: repo, configYAML: "objective:\n  metric: ndcg\n  direction: bigger\nguardrails:\n  - peak_rss: 2GB\n",
                    problems: [
                        ConfigProblem(path: "objective.direction", message: "must be minimize or maximize"),
                        ConfigProblem(path: "guardrails[0].peak_rss", message: "unknown guardrail; did you mean memory_mb?"),
                    ])
            default:
                let offered = world.templates.filter { $0.kind == .template }.prefix(3)
                return RepoInspection(
                    source: source, repo: repo, baseRef: world.repos.first { $0.id == repo }?.defaultBranch ?? "main",
                    templates: Array(offered), suggestedTemplateID: "build-size")
            }
        }
    }

    private static func foundInspection(source: RepoSource, repo: String, suggested: String?) -> RepoInspection {
        let config = RepoConfig(
            metric: "http_req_duration.p95", lowerIsBetter: true, evaluator: "http-latency", evaluatorIsTemplate: true,
            holdout: "a hidden workload re-checks every win", image: "ghcr.io/acme/shop-api:ci", machine: "c7i.2xlarge",
            region: "us-east-1", protected: [
                ProtectedPath(glob: "tests/**", reason: "the tests are the guardrail"),
                ProtectedPath(glob: "bench/**", reason: "the evaluator is not the agent's to change"),
            ], name: "Checkout p95")
        let yaml = "objective:\n  name: Checkout p95\n  metric: http_req_duration.p95\n  direction: minimize\nevaluator:\n  template: http-latency\n"
        return RepoInspection(
            source: source, repo: repo, baseRef: "main", configYAML: yaml, config: config, suggestedTemplateID: suggested)
    }

    /// Test and snapshot hook: "What's missing" answers this list.
    public func simulate(preflight items: [PreflightItem]?) {
        preflightOverride = items
    }

    public func preflight(_ request: PreflightRequest) async throws -> [PreflightItem] {
        if let preflightOverride { return preflightOverride }
        guard request.source != .demo else { return [] }
        var items: [PreflightItem] = []
        for id in request.agentIDs.sorted() {
            guard let agent = world.connectors.first(where: { $0.id == id }) else { continue }
            if !agent.state.isUsable {
                items.append(PreflightItem(
                    id: "agent-" + id, label: agent.name + " is not signed in", state: .fail,
                    detail: "Run `codex login` in a terminal, or add an API key in Connectors."))
            } else if request.placement == .cloud && !agent.canRunInCloud {
                items.append(PreflightItem(
                    id: "cloud-" + id, label: agent.name + " cannot run in the cloud", state: .fail,
                    detail: "A login runs on this Mac only. Add an API key to run unattended."))
            }
        }
        if !world.connectors.contains(where: { $0.kind == .aws && $0.state.isUsable }) {
            items.append(PreflightItem(id: "aws", label: "No AWS account is connected", state: .fail, detail: "The evaluator runs in a sandbox in your AWS account."))
        }
        return items
    }

    public func proposeTemplate(_ request: TemplateProposalRequest) async throws -> TemplateProposal {
        guard let template = world.templates.first(where: { $0.id == request.templateID && $0.kind == .template }) else {
            throw RatchetClientError.invalid("That template cannot be added to a repo.")
        }
        guard world.repos.contains(where: { $0.id == request.repo }) else { throw RatchetClientError.notFound("repo \(request.repo)") }
        counter += 1
        return TemplateProposal(
            prURL: "https://github.com/\(request.repo)/pull/\(300 + counter)", branch: "ratchet/add-\(template.id)",
            files: ["ratchet.yaml", "bench/\(template.id).js"], templateID: template.id)
    }

    // MARK: Setup: drafts and development overrides

    public func discardDraft(id: String) async throws {
        guard let draft = world.drafts[id] else { throw RatchetClientError.notFound("draft \(id)") }
        guard draft.state == .draft else { throw RatchetClientError.invalid("That environment was approved; it cannot be discarded.") }
        world.drafts[id] = nil
    }

    public func advancedSettings() async throws -> AdvancedSettings { advanced }

    public func saveAdvancedSettings(_ settings: AdvancedSettings) async throws -> AdvancedSettings {
        let problems = SettingsCheck.problems(in: settings)
        if let first = AdvancedField.allCases.first(where: { problems[$0] != nil }), let message = problems[first] {
            throw RatchetClientError.invalid(first.label + ": " + message)
        }
        for field in AdvancedField.allCases {
            advanced.set(settings.value(of: field).trimmingCharacters(in: .whitespacesAndNewlines), for: field)
        }
        return advanced
    }

    // MARK: Environment drafts

    public func createDraft(_ request: DraftRequest) async throws -> EnvironmentDraft {
        guard world.repos.contains(where: { $0.id == request.repoID }) else { throw RatchetClientError.notFound("repo \(request.repoID)") }
        counter += 1
        let id = "draft-\(counter)"
        let singleTemplates: Set<String> = ["cli-cold-start", "build-size", "test-suite-time", "memory"]
        let small = singleTemplates.contains(request.templateID ?? "") || request.repoID.hasSuffix("cli") || request.repoID.hasSuffix("web-app")
        let draft = small ? Self.singleDraft(id: id, repoID: request.repoID) : MockWorld.fullStackDraft(id: id, repoID: request.repoID)
        world.drafts[id] = draft
        return draft
    }

    public func reviseDraft(id: String, instruction: String) async throws -> EnvironmentDraft {
        guard var draft = world.drafts[id] else { throw RatchetClientError.notFound("draft \(id)") }
        let text = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw RatchetClientError.invalid("Say what to change.") }
        let lower = text.lowercased()
        if lower.contains("smaller") || lower.contains("single") || lower.contains("one machine") {
            let small = Self.singleDraft(id: id, repoID: draft.repoID)
            draft.scope = small.scope
            draft.headline = small.headline
            draft.emphasis = small.emphasis
            draft.reason = small.reason
            draft.resources = small.resources
            draft.files = small.files
            draft.readyMinutes = small.readyMinutes
        } else if lower.contains("rds"), let index = draft.resources.firstIndex(where: { $0.id == "pg" }) {
            draft.resources[index] = DraftResource(
                id: "pg", name: "Amazon RDS Postgres 16", detail: "restored from a snapshot of anonymised orders",
                machine: "db.r6g.large", usdPerHour: 0.26)
            draft.reason = "Postgres now runs on RDS, as you asked."
        } else {
            draft.reason = "Noted: " + text
        }
        draft.revision += 1
        draft.state = .draft
        world.drafts[id] = draft
        return draft
    }

    public func approveDraft(id: String) async throws -> EnvironmentDraft {
        guard var draft = world.drafts[id] else { throw RatchetClientError.notFound("draft \(id)") }
        draft.state = .approved
        world.drafts[id] = draft
        return draft
    }

    private static func singleDraft(id: String, repoID: String) -> EnvironmentDraft {
        EnvironmentDraft(
            id: id, repoID: repoID, scope: .single,
            headline: "This task needs one machine: the project built from the repo, measured on identical hardware.",
            emphasis: "one machine",
            reason: "The measurement does not depend on other services.",
            draftedBy: "Claude Code", draftSeconds: 45, draftCostUSD: 0.12, readyMinutes: 3,
            resources: [DraftResource(id: "runner", name: "Runner", detail: "builds and measures the repo", machine: "c7i.large", usdPerHour: 0.09)],
            files: ["main.tf", "bench.sh"],
            checks: "isolated, results sealed, same hardware every time, full teardown")
    }
}
