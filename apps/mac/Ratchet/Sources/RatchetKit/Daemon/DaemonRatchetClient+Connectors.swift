import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

// Connectors: accounts, profiles and logins, the GitHub device flow, per-repo selections, pasted tokens and keys, checks, and
// the harnesses found on this Mac.

extension DaemonRatchetClient {
    // MARK: Connectors

    public func connectors() async throws -> [Connector] {
        try await wire.calling {
            try await api.listConnectors().ok.body.json.connectors.map(DaemonMapping.connector)
        }
    }

    /// GitHub starts the device flow (the account appears in the list when it finishes); an AWS profile is connected by its
    /// name; an agent is a CLI login that was removed earlier. The UI for pasting keys and tokens comes later.
    public func addConnector(_ new: NewConnector) async throws -> Connector {
        try await wire.calling {
            let list: VM.ConnectorsVm
            switch new.kind {
            case .github:
                // GitHub signs in with the device flow: the code is in `githubSignInFlow()`, the account appears when it ends.
                list = try await api.startGithubConnectorSignIn(body: .json(.init())).ok.body.json
                guard let existing = list.connectors.first(where: { DaemonMapping.connector($0).kind == .github }) else {
                    return Connector(id: "github-pending", kind: .github, name: new.name, detail: "Signing in with GitHub", state: .signedOut)
                }
                return DaemonMapping.connector(existing)
            case .aws:
                if new.region != nil || new.monthlyCapUSD != nil {
                    list = try await addAwsConnector(new)
                } else {
                    list = try await api.addAwsConnector(body: .json(.init(profile: new.name))).ok.body.json
                }
            case .agent:
                let agent = VM.ConnectorAgent(rawValue: Harness(named: new.name).rawValue) ?? .claudeCode
                if let key = new.secret?.trimmingCharacters(in: .whitespacesAndNewlines), !key.isEmpty {
                    // The key goes to the daemon once; it files it in the Keychain and never returns it.
                    let label = new.name.trimmingCharacters(in: .whitespacesAndNewlines)
                    list = try await api.addAgentConnector(body: .json(.case2(.init(
                        agent: agent, apiKey: key, name: label.isEmpty ? nil : label, source: .apiKey)))).ok.body.json
                } else {
                    list = try await api.addAgentConnector(body: .json(.case1(.init(agent: agent, source: .login)))).ok.body.json
                }
            }
            let added = list.connectors.map(DaemonMapping.connector).first { $0.kind == new.kind && $0.name == new.name }
            guard let added = added ?? list.connectors.map(DaemonMapping.connector).last(where: { $0.kind == new.kind }) else {
                throw RatchetClientError.notFound(new.name)
            }
            return added
        }
    }

    public func githubSignInFlow() async throws -> GithubSignInFlow? {
        try await wire.calling { DaemonMapping.githubFlow(try await api.listConnectors().ok.body.json.githubFlow) }
    }

    public func startGithubSignIn(connectorID: String?) async throws -> GithubSignInFlow? {
        try await wire.calling {
            let list = try await api.startGithubConnectorSignIn(body: .json(.init(connectorId: connectorID))).ok.body.json
            return DaemonMapping.githubFlow(list.githubFlow)
        }
    }

    public func cancelGithubSignIn() async throws {
        try await wire.calling { _ = try await api.cancelGithubConnectorSignIn().ok }
    }

    public func removeConnector(id: String) async throws {
        try await wire.calling {
            _ = try await api.removeConnector(path: .init(id: id)).ok
        }
    }

    public func setDefaultConnector(id: String) async throws -> [Connector] {
        try await wire.calling {
            try await api.setDefaultConnector(path: .init(id: id)).ok.body.json.connectors.map(DaemonMapping.connector)
        }
    }

    public func signInConnector(id: String) async throws -> Connector {
        try await wire.calling {
            guard let connector = try await api.testConnector(path: .init(id: id)).ok.body.json.connector else {
                throw RatchetClientError.notFound(id)
            }
            return DaemonMapping.connector(connector)
        }
    }

    public func connectionSelection(repoID: String) async throws -> ConnectionSelection? {
        try await wire.calling {
            DaemonMapping.selection(try await api.getConnectorSelection(query: .init(repo: repoID)).ok.body.json)
        }
    }

    public func saveConnectionSelection(_ selection: ConnectionSelection) async throws {
        try await wire.calling {
            guard let aws = selection.awsID else { return }
            var github = selection.githubID
            if github == nil {
                github = try await api.getConnectorSelection(query: .init(repo: selection.repoID)).ok.body.json.effective.github
            }
            guard let github else { return }
            let input = VM.SaveRepoConnectionsInput(
                repo: selection.repoID, selection: DaemonMapping.repoConnections(selection, github: github, aws: aws))
            _ = try await api.saveConnectorSelection(body: .json(input)).ok
        }
    }

    // MARK: Connectors

    public func connectorSetup() async throws -> ConnectorSetup {
        try await wire.calling {
            let list = try await api.listConnectors().ok.body.json
            return ConnectorSetup(encryptionAvailable: list.encryptionAvailable, clientIdConfigured: list.clientIdConfigured)
        }
    }

    public func addGithubToken(_ token: String, name: String?) async throws -> [Connector] {
        try await wire.calling {
            var fields: [String: Any] = ["token": token.trimmingCharacters(in: .whitespacesAndNewlines)]
            if let name = name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { fields["name"] = name }
            let data = try await wire.jsonCall(.post, "/v1/connectors/github", operationID: "addGithubConnector", json: try DaemonTransport.jsonBody(fields))
            return try wire.decode(VM.ConnectorsVm.self, data).connectors.map(DaemonMapping.connector)
        }
    }

    public func updateConnector(id: String, _ update: ConnectorUpdate) async throws -> [Connector] {
        try await wire.calling {
            var fields: [String: Any] = [:]
            if let name = update.name?.trimmingCharacters(in: .whitespacesAndNewlines) { fields["name"] = name }
            if let region = update.region?.trimmingCharacters(in: .whitespacesAndNewlines) { fields["region"] = region }
            switch update.cap {
            case .keep: break
            case .remove: fields["monthlyCapUsd"] = NSNull()
            case .set(let usd): fields["monthlyCapUsd"] = usd
            }
            let data = try await wire.jsonCall(
                .patch, "/v1/connectors/\(wire.escaped(id))", operationID: "updateConnector", json: try DaemonTransport.jsonBody(fields))
            return try wire.decode(VM.ConnectorsVm.self, data).connectors.map(DaemonMapping.connector)
        }
    }

    public func checkConnector(id: String) async throws -> ConnectorCheck {
        try await wire.calling {
            let result = try await api.testConnector(path: .init(id: id)).ok.body.json
            return ConnectorCheck(ok: result.ok, detail: result.detail)
        }
    }

    public func refreshConnectors() async throws -> [Connector] {
        try await wire.calling {
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
        try await wire.calling {
            let data = try await wire.jsonCall(.get, "/v1/aws/profiles", operationID: "listAwsProfiles")
            let list = try wire.decode(AwsProfilesPayload.self, data)
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
        var defaultModel: String?
        var models: [ModelPayload]?
        var billing: String?
        var hint: String?
        var path: String?

        struct ModelPayload: Decodable {
            var id: String
            var label: String
        }
    }

    public func detectedHarnesses(redetect: Bool) async throws -> [DetectedHarness] {
        try await wire.calling {
            let data: Data
            if redetect {
                data = try await wire.jsonCall(.post, "/v1/harnesses/detections", operationID: "detectHarnesses", json: Data("{}".utf8))
            } else {
                data = try await wire.jsonCall(.get, "/v1/harnesses", operationID: "listHarnesses")
            }
            return try wire.decode([HarnessPayload].self, data).compactMap { found in
                guard let harness = Harness(rawValue: found.kind) else { return nil }
                let auth: DetectedHarness.Auth = found.auth == "api-key" ? .apiKey : found.auth == "signed-in" ? .signedIn : .signedOut
                let models = (found.models ?? []).map { HarnessModel(id: $0.id, title: $0.label, harness: harness) }
                return DetectedHarness(
                    harness: harness, installed: found.installed, version: found.version, auth: auth,
                    models: models, defaultModel: found.defaultModel,
                    billing: found.billing == "subscription" ? .subscription : found.billing == "api-key" ? .apiKey : nil,
                    hint: found.hint, path: found.path)
            }
        }
    }

    // MARK: Adding an AWS profile with a region and a cap

    func addAwsConnector(_ new: NewConnector) async throws -> VM.ConnectorsVm {
        var fields: [String: Any] = ["profile": new.name]
        if let region = new.region?.trimmingCharacters(in: .whitespacesAndNewlines), !region.isEmpty { fields["region"] = region }
        if let cap = new.monthlyCapUSD { fields["monthlyCapUsd"] = cap }
        let data = try await wire.jsonCall(.post, "/v1/connectors/aws", operationID: "addAwsConnector", json: try DaemonTransport.jsonBody(fields))
        return try wire.decode(VM.ConnectorsVm.self, data)
    }
}
