import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

// The routes behind New optimization step 1, the Connectors forms, draft discarding and the development overrides.
// The generated client holds only the operations listed in tools/api/openapi-generator-config.yaml, and a PATCH that
// must say "no cap" with a JSON null cannot be written with its optional fields, so these go through the same
// transport and error middleware as JSON calls of their own. The bodies are built here and the answers decoded
// here (or into the generated view models where the client has them).

extension DaemonRatchetClient {
    /// One JSON request through the error middleware; the response body (empty for a 204).
    func jsonCall(_ method: HTTPRequest.Method, _ path: String, operationID: String, json: Data? = nil) async throws -> Data {
        let base = URL(string: "http://localhost") ?? URL(fileURLWithPath: "/")
        var request = HTTPRequest(method: method, scheme: nil, authority: nil, path: path)
        request.headerFields[.accept] = "application/json"
        var payload: HTTPBody?
        if let json {
            request.headerFields[.contentType] = "application/json"
            payload = HTTPBody(String(decoding: json, as: UTF8.self))
        }
        let transport = self.transport
        let (_, body) = try await APIErrorMiddleware().intercept(request, body: payload, baseURL: base, operationID: operationID) {
            request, body, url in
            try await transport.send(request, body: body, baseURL: url, operationID: operationID)
        }
        var bytes: [UInt8] = []
        if let body {
            for try await chunk in body { bytes.append(contentsOf: chunk) }
        }
        return Data(bytes)
    }

    /// A JSON object from a dictionary; `NSNull()` writes a null.
    static func jsonBody(_ object: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: object)
    }

    /// `?name=value`, with the characters that would end a value escaped.
    static func queryItem(_ name: String, _ value: String) -> String {
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "&=+#?/")
        return name + "=" + (value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value)
    }

    private func decode<T: Decodable>(_ type: T.Type, _ data: Data) throws -> T {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw RatchetClientError.unavailable("ratchetd answered with something the app cannot read.")
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
            let path = "/v1/github/repos" + (trimmed.isEmpty ? "" : "?" + Self.queryItem("query", trimmed))
            let data = try await jsonCall(.get, path, operationID: "listRepos")
            return try decode([RepoPayload].self, data).map { repo in
                let account = repo.fullName.split(separator: "/").first.map(String.init) ?? repo.fullName
                return Repo(
                    id: repo.fullName, defaultBranch: repo.defaultBranch, account: account, isPrivate: repo.isPrivate,
                    summary: repo.description)
            }
        } catch {
            // 409: not signed in to GitHub. The list is simply empty until the user signs in.
            if let failure = daemonError(in: error), failure.status == 409 { return [] }
            throw translate(error)
        }
    }

    public func inspectRepo(_ source: RepoSource) async throws -> RepoInspection {
        try await calling {
            let wire: [String: Any]
            switch source {
            case .github(let repo): wire = ["kind": "github", "repo": repo]
            case .local(let path): wire = ["kind": "local", "path": path]
            }
            let data = try await jsonCall(
                .post, "/v1/repos/inspections", operationID: "inspectRepo", json: try Self.jsonBody(["source": wire]))
            return DaemonMapping.inspection(try decode(VM.RepoInspectVm.self, data))
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
        try await calling {
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
            var wire: [String: Any] = [
                "agents": kinds,
                "agentsWhere": request.source == .demo ? "local" : request.placement.rawValue,
                "source": request.source.rawValue,
            ]
            if let repo = request.repo, request.source != .demo { wire["repo"] = repo }
            let data = try await jsonCall(.post, "/v1/preflight", operationID: "runPreflight", json: try Self.jsonBody(wire))
            return try decode([PreflightPayload].self, data).map {
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
        try await calling {
            let body = try Self.jsonBody(["repoUrl": request.repo, "baseRef": request.baseRef, "templateId": request.templateID])
            let data = try await jsonCall(.post, "/v1/template-proposals", operationID: "proposeTemplate", json: body)
            let result = try decode(ProposalPayload.self, data)
            return TemplateProposal(prURL: result.prUrl, branch: result.branch, files: result.files, templateID: result.templateId)
        }
    }

    // MARK: Connectors

    public func connectorSetup() async throws -> ConnectorSetup {
        try await calling {
            let list = try await api.listConnectors().ok.body.json
            return ConnectorSetup(encryptionAvailable: list.encryptionAvailable, clientIdConfigured: list.clientIdConfigured)
        }
    }

    public func addGithubToken(_ token: String, name: String?) async throws -> [Connector] {
        try await calling {
            var wire: [String: Any] = ["token": token.trimmingCharacters(in: .whitespacesAndNewlines)]
            if let name = name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { wire["name"] = name }
            let data = try await jsonCall(.post, "/v1/connectors/github", operationID: "addGithubConnector", json: try Self.jsonBody(wire))
            return try decode(VM.ConnectorsVm.self, data).connectors.map(DaemonMapping.connector)
        }
    }

    public func updateConnector(id: String, _ update: ConnectorUpdate) async throws -> [Connector] {
        try await calling {
            var wire: [String: Any] = [:]
            if let name = update.name?.trimmingCharacters(in: .whitespacesAndNewlines) { wire["name"] = name }
            if let region = update.region?.trimmingCharacters(in: .whitespacesAndNewlines) { wire["region"] = region }
            switch update.cap {
            case .keep: break
            case .remove: wire["monthlyCapUsd"] = NSNull()
            case .set(let usd): wire["monthlyCapUsd"] = usd
            }
            let data = try await jsonCall(
                .patch, "/v1/connectors/\(escaped(id))", operationID: "updateConnector", json: try Self.jsonBody(wire))
            return try decode(VM.ConnectorsVm.self, data).connectors.map(DaemonMapping.connector)
        }
    }

    public func checkConnector(id: String) async throws -> ConnectorCheck {
        try await calling {
            let result = try await api.testConnector(path: .init(id: id)).ok.body.json
            return ConnectorCheck(ok: result.ok, detail: result.detail)
        }
    }

    public func refreshConnectors() async throws -> [Connector] {
        try await calling {
            try await api.refreshConnectors().ok.body.json.connectors.map(DaemonMapping.connector)
        }
    }

    private struct AwsProfilesPayload: Decodable {
        struct Profile: Decodable {
            var name: String
            var kind: String
            var ssoSession: String?
        }
        var profiles: [Profile]
        var ssoSessions: [String]
        var filesFound: Bool
    }

    public func awsProfiles() async throws -> AwsProfiles {
        try await calling {
            let data = try await jsonCall(.get, "/v1/aws/profiles", operationID: "listAwsProfiles")
            let list = try decode(AwsProfilesPayload.self, data)
            return AwsProfiles(
                profiles: list.profiles.map {
                    AwsProfile(name: $0.name, kind: AwsProfileKind(rawValue: $0.kind) ?? .other, ssoSession: $0.ssoSession)
                },
                ssoSessions: list.ssoSessions, filesFound: list.filesFound)
        }
    }

    private struct HarnessPayload: Decodable {
        var kind: String
        var installed: Bool
        var version: String?
        var auth: String
    }

    public func detectedAgents(redetect: Bool) async throws -> [DetectedAgent] {
        try await calling {
            let data: Data
            if redetect {
                data = try await jsonCall(.post, "/v1/harnesses/detections", operationID: "detectHarnesses", json: Data("{}".utf8))
            } else {
                data = try await jsonCall(.get, "/v1/harnesses", operationID: "listHarnesses")
            }
            return try decode([HarnessPayload].self, data).compactMap { found in
                guard let cli = AgentCLI(rawValue: found.kind) else { return nil }
                let auth: DetectedAgent.Auth = found.auth == "api-key" ? .apiKey : found.auth == "signed-in" ? .signedIn : .signedOut
                return DetectedAgent(cli: cli, installed: found.installed, version: found.version, auth: auth)
            }
        }
    }

    // MARK: Drafts

    public func discardDraft(id: String) async throws {
        try await calling {
            _ = try await jsonCall(.delete, "/v1/environment-drafts/\(escaped(id))", operationID: "discardEnvironmentDraft")
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
        try await calling {
            try decode(SettingsPayload.self, try await jsonCall(.get, "/v1/settings", operationID: "getSettings")).model
        }
    }

    public func saveAdvancedSettings(_ settings: AdvancedSettings) async throws -> AdvancedSettings {
        try await calling {
            let body = try Self.jsonBody([
                "runnerImageRef": settings.runnerAMI.trimmingCharacters(in: .whitespacesAndNewlines),
                "benchImageRef": settings.benchAMI.trimmingCharacters(in: .whitespacesAndNewlines),
                "tofuBinary": settings.tofuBinary.trimmingCharacters(in: .whitespacesAndNewlines),
                "moduleDir": settings.moduleDir.trimmingCharacters(in: .whitespacesAndNewlines),
            ])
            let data = try await jsonCall(.put, "/v1/settings/advanced", operationID: "saveAdvancedSettings", json: body)
            return try decode(SettingsPayload.self, data).model
        }
    }

    // MARK: Adding an AWS profile with a region and a cap

    func addAwsConnector(_ new: NewConnector) async throws -> VM.ConnectorsVm {
        var wire: [String: Any] = ["profile": new.name]
        if let region = new.region?.trimmingCharacters(in: .whitespacesAndNewlines), !region.isEmpty { wire["region"] = region }
        if let cap = new.monthlyCapUSD { wire["monthlyCapUsd"] = cap }
        let data = try await jsonCall(.post, "/v1/connectors/aws", operationID: "addAwsConnector", json: try Self.jsonBody(wire))
        return try decode(VM.ConnectorsVm.self, data)
    }
}
