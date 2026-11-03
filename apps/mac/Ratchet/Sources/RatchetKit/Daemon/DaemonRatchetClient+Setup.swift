import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

// The routes behind New Optimization step 1: repos, what a repo says about its benchmark, "What's missing", template pull
// requests, environment drafts, and the development overrides.

extension DaemonRatchetClient {
    // MARK: Repos

    public func repos() async throws -> [Repo] {
        do {
            return try await api.listRepos().ok.body.json.map(DaemonMapping.repo)
        } catch {
            // 409: not signed in to GitHub. The repo picker is simply empty until the user signs in.
            if let failure = daemonError(in: error), failure.status == 409 { return [] }
            throw wire.translate(error)
        }
    }

    public func detectBenchmark(repoID: String) async throws -> BenchmarkDetection {
        do {
            let input = VM.InspectRepoInput(source: .case1(.init(kind: .github, repo: repoID)))
            let inspected = try await api.inspectRepo(body: .json(input)).ok.body.json
            return DaemonMapping.benchmarkDetection(repoID: repoID, inspected)
        } catch {
            if let failure = daemonError(in: error), !failure.isUnsupported {
                return BenchmarkDetection(repoID: repoID, found: false, summary: failure.message)
            }
            throw wire.translate(error)
        }
    }

    // MARK: Repos

    private struct RepoPayload: Decodable {
        var fullName: String
        var defaultBranch: String
        var isPrivate: Bool?
        var description: String?

        private enum CodingKeys: String, CodingKey {
            case fullName, defaultBranch, description
            case isPrivate = "private"
        }
    }

    public func searchRepos(query: String) async throws -> [Repo] {
        do {
            let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
            let path = "/v1/github/repos" + (trimmed.isEmpty ? "" : "?" + DaemonTransport.queryItem("query", trimmed))
            let data = try await wire.jsonCall(.get, path, operationID: "listRepos")
            return try wire.decode([RepoPayload].self, data).map { repo in
                let account = repo.fullName.split(separator: "/").first.map(String.init) ?? repo.fullName
                return Repo(
                    id: repo.fullName, defaultBranch: repo.defaultBranch, account: account, isPrivate: repo.isPrivate,
                    summary: repo.description)
            }
        } catch {
            // 409: not signed in to GitHub. The list is simply empty until the user signs in.
            if let failure = daemonError(in: error), failure.status == 409 { return [] }
            throw wire.translate(error)
        }
    }

    public func inspectRepo(_ source: RepoSource) async throws -> RepoInspection {
        try await wire.calling {
            let described: [String: Any]
            switch source {
            case .github(let repo): described = ["kind": "github", "repo": repo]
            case .local(let path): described = ["kind": "local", "path": path]
            }
            let data = try await wire.jsonCall(
                .post, "/v1/repos/inspections", operationID: "inspectRepo", json: try DaemonTransport.jsonBody(["source": described]))
            return DaemonMapping.inspection(try wire.decode(VM.RepoInspectVm.self, data))
        }
    }

    // MARK: Preflight and template pull requests

    private struct PreflightPayload: Decodable {
        var id: String
        var label: String
        var state: String
        var detail: String
    }

    public func preflight(_ request: PreflightRequest) async throws -> [PreflightItem] {
        try await wire.calling {
            var kinds: [String] = []
            if request.source == .demo {
                kinds = ["claude-code", "codex", "grok"]
            } else if let list = try? await api.listConnectors().ok.body.json.connectors {
                for id in request.agentIDs.sorted() {
                    if case .case3(let agent)? = list.first(where: { DaemonMapping.connectorID($0) == id }) {
                        let kind = agent.agent.rawValue
                        if !kinds.contains(kind) { kinds.append(kind) }
                    }
                }
            }
            var fields: [String: Any] = [
                "agents": kinds,
                "agentsWhere": request.source == .demo ? "local" : request.placement.rawValue,
                "source": request.source.rawValue,
            ]
            if let repo = request.repo, request.source != .demo { fields["repo"] = repo }
            let data = try await wire.jsonCall(.post, "/v1/preflight", operationID: "runPreflight", json: try DaemonTransport.jsonBody(fields))
            return try wire.decode([PreflightPayload].self, data).map {
                PreflightItem(id: $0.id, label: $0.label, state: PreflightState(rawValue: $0.state) ?? .warn, detail: $0.detail)
            }
        }
    }

    private struct ProposalPayload: Decodable {
        var prUrl: String
        var branch: String
        var files: [String]
        var templateId: String
    }

    public func proposeTemplate(_ request: TemplateProposalRequest) async throws -> TemplateProposal {
        try await wire.calling {
            let body = try DaemonTransport.jsonBody(["repoUrl": request.repo, "baseRef": request.baseRef, "templateId": request.templateID])
            let data = try await wire.jsonCall(.post, "/v1/template-proposals", operationID: "proposeTemplate", json: body)
            let result = try wire.decode(ProposalPayload.self, data)
            return TemplateProposal(prURL: result.prUrl, branch: result.branch, files: result.files, templateID: result.templateId)
        }
    }

    // MARK: Environment drafts

    public func createDraft(_ request: DraftRequest) async throws -> EnvironmentDraft {
        try await wire.calling {
            let input = VM.CreateEnvironmentDraftInput(agentConnector: .claudeCode, goal: request.goal, repo: request.repoID)
            let created = try await api.createEnvironmentDraft(body: .json(input)).created.body.json
            return try await settledDraft(created)
        }
    }

    public func reviseDraft(id: String, instruction: String) async throws -> EnvironmentDraft {
        try await wire.calling {
            let revised = try await api.reviseEnvironmentDraft(path: .init(id: id), body: .json(.init(message: instruction))).ok.body.json
            return try await settledDraft(revised)
        }
    }

    public func approveDraft(id: String) async throws -> EnvironmentDraft {
        try await wire.calling {
            DaemonMapping.draft(try await api.approveEnvironmentDraft(path: .init(id: id), body: .json(.init())).ok.body.json)
        }
    }

    /// An agent drafts the environment, so a new or revised draft is `drafting` for a while. Waits (up to 5 minutes) until
    /// it is ready, the way the mock answers at once.
    private func settledDraft(_ first: VM.EnvironmentDraftVm) async throws -> EnvironmentDraft {
        var draft = first
        var polls = 0
        while draft.status == .drafting {
            polls += 1
            guard polls <= 300 else { throw RatchetClientError.unavailable("Drafting the environment is taking too long.") }
            try await Task.sleep(for: .seconds(1))
            draft = try await api.getEnvironmentDraft(path: .init(id: draft.id)).ok.body.json
        }
        if draft.status == .failed {
            throw RatchetClientError.unavailable(draft.error ?? "The environment could not be drafted.")
        }
        return DaemonMapping.draft(draft)
    }

    // MARK: Drafts

    public func discardDraft(id: String) async throws {
        try await wire.calling {
            _ = try await wire.jsonCall(.delete, "/v1/environment-drafts/\(wire.escaped(id))", operationID: "discardEnvironmentDraft")
        }
    }

    // MARK: Advanced settings

    private struct SettingsPayload: Decodable {
        struct Advanced: Decodable {
            struct Values: Decodable {
                var runnerImageRef: String
                var benchImageRef: String
                var tofuBinary: String
                var moduleDir: String
            }
            struct Defaults: Decodable {
                var tofuBinary: String
                var moduleDir: String
            }
            var values: Values
            var defaults: Defaults
        }
        var advanced: Advanced

        var model: AdvancedSettings {
            AdvancedSettings(
                runnerAMI: advanced.values.runnerImageRef, benchAMI: advanced.values.benchImageRef,
                tofuBinary: advanced.values.tofuBinary, moduleDir: advanced.values.moduleDir,
                defaultTofuBinary: advanced.defaults.tofuBinary, defaultModuleDir: advanced.defaults.moduleDir)
        }
    }

    public func advancedSettings() async throws -> AdvancedSettings {
        try await wire.calling {
            try wire.decode(SettingsPayload.self, try await wire.jsonCall(.get, "/v1/settings", operationID: "getSettings")).model
        }
    }

    public func saveAdvancedSettings(_ settings: AdvancedSettings) async throws -> AdvancedSettings {
        try await wire.calling {
            let body = try DaemonTransport.jsonBody([
                "runnerImageRef": settings.runnerAMI.trimmingCharacters(in: .whitespacesAndNewlines),
                "benchImageRef": settings.benchAMI.trimmingCharacters(in: .whitespacesAndNewlines),
                "tofuBinary": settings.tofuBinary.trimmingCharacters(in: .whitespacesAndNewlines),
                "moduleDir": settings.moduleDir.trimmingCharacters(in: .whitespacesAndNewlines),
            ])
            let data = try await wire.jsonCall(.put, "/v1/settings/advanced", operationID: "saveAdvancedSettings", json: body)
            return try wire.decode(SettingsPayload.self, data).model
        }
    }
}
