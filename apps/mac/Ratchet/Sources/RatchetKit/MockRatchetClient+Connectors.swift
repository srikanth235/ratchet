import Foundation

// Connectors of the sample world: accounts, the GitHub device flow, selections, forms, checks and detection.

extension MockRatchetClient {
    // MARK: Connectors

    public func connectors() async throws -> [Connector] { world.connectors }

    public func addConnector(_ new: NewConnector) async throws -> Connector {
        let name = new.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw RatchetClientError.invalid("Give the connector a name.") }
        counter += 1
        let hasDefault = world.connectors.contains { $0.kind == new.kind && $0.isDefault }
        let origin: ConnectorOrigin?
        switch new.kind {
        case .github: origin = .token
        case .agent: origin = (new.secret ?? "").isEmpty ? .login : .apiKey
        case .aws: origin = nil
        }
        let isAWS = new.kind == .aws
        let isAgent = new.kind == .agent
        let cap: Double? = isAWS ? (new.monthlyCapUSD ?? (new.region == nil ? 100 : nil)) : nil
        let harness: Harness? = isAgent ? Harness(named: name) : nil
        let connector = Connector(
            id: "\(new.kind.rawValue)-added-\(counter)", kind: new.kind, name: name,
            detail: new.detail.isEmpty ? "Added just now" : new.detail,
            state: isAgent ? .valid : .connected, isDefault: !hasDefault, monthlyCapUSD: cap,
            canRunInCloud: isAgent && !(new.secret ?? "").isEmpty, origin: origin,
            profile: isAWS ? name : nil, region: isAWS ? (new.region ?? "us-east-1") : nil, harness: harness)
        world.connectors.append(connector)
        emit([.connectorsChanged(world.connectors)])
        return connector
    }

    public func githubSignInFlow() async throws -> GithubSignInFlow? { githubFlow }

    public func startGithubSignIn(connectorID: String?) async throws -> GithubSignInFlow? {
        let flow = GithubSignInFlow(
            state: .pending, userCode: "WDJB-MJHT", verificationURL: "https://github.com/login/device",
            expiresAt: startedAt.addingTimeInterval(900))
        githubFlow = flow
        emit([.githubFlowChanged(flow)])
        return flow
    }

    public func cancelGithubSignIn() async throws {
        githubFlow = nil
        emit([.githubFlowChanged(nil)])
    }

    public func removeConnector(id: String) async throws {
        guard let index = world.connectors.firstIndex(where: { $0.id == id }) else {
            throw RatchetClientError.notFound("connector \(id)")
        }
        let removed = world.connectors.remove(at: index)
        if removed.isDefault, let next = world.connectors.firstIndex(where: { $0.kind == removed.kind && $0.state.isUsable }) {
            world.connectors[next].isDefault = true
        }
        emit([.connectorsChanged(world.connectors)])
    }

    public func setDefaultConnector(id: String) async throws -> [Connector] {
        guard let kind = world.connectors.first(where: { $0.id == id })?.kind else {
            throw RatchetClientError.notFound("connector \(id)")
        }
        for index in world.connectors.indices where world.connectors[index].kind == kind {
            world.connectors[index].isDefault = world.connectors[index].id == id
        }
        emit([.connectorsChanged(world.connectors)])
        return world.connectors
    }

    public func signInConnector(id: String) async throws -> Connector {
        guard let index = world.connectors.firstIndex(where: { $0.id == id }) else {
            throw RatchetClientError.notFound("connector \(id)")
        }
        world.connectors[index].state = world.connectors[index].kind == .agent ? .signedIn : .connected
        emit([.connectorsChanged(world.connectors)])
        return world.connectors[index]
    }

    public func connectionSelection(repoID: String) async throws -> ConnectionSelection? {
        world.selections[repoID]
    }

    public func saveConnectionSelection(_ selection: ConnectionSelection) async throws {
        world.selections[selection.repoID] = selection
    }


    // MARK: Setup: connector forms

    /// Test and snapshot hook: the keychain is (not) available.
    public func simulate(keychainAvailable available: Bool) {
        keychainAvailable = available
    }

    public func connectorSetup() async throws -> ConnectorSetup {
        ConnectorSetup(encryptionAvailable: keychainAvailable, clientIdConfigured: true)
    }

    public func addGithubToken(_ token: String, name: String?) async throws -> [Connector] {
        guard keychainAvailable else { throw RatchetClientError.unavailable("The keychain is unavailable, so a token cannot be stored.") }
        if let problem = SecretKind.githubToken.problem(token) { throw RatchetClientError.invalid(problem) }
        if token.hasSuffix("0000") { throw RatchetClientError.invalid("GitHub rejected the sign-in with that token.") }
        counter += 1
        let label = (name ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let hasDefault = world.connectors.contains { $0.kind == .github && $0.isDefault }
        world.connectors.append(Connector(
            id: "gh-token-\(counter)", kind: .github, name: label.isEmpty ? "octocat" : label, detail: "Access token", state: .connected,
            isDefault: !hasDefault, origin: .token))
        emit([.connectorsChanged(world.connectors)])
        return world.connectors
    }

    public func updateConnector(id: String, _ update: ConnectorUpdate) async throws -> [Connector] {
        guard let index = world.connectors.firstIndex(where: { $0.id == id }) else { throw RatchetClientError.notFound("connector \(id)") }
        if let name = update.name?.trimmingCharacters(in: .whitespacesAndNewlines) {
            guard !name.isEmpty else { throw RatchetClientError.invalid("Give the connector a name.") }
            world.connectors[index].name = name
        }
        if world.connectors[index].kind == .aws {
            if let region = update.region?.trimmingCharacters(in: .whitespacesAndNewlines), !region.isEmpty {
                if let problem = SettingsCheck.regionProblem(region) { throw RatchetClientError.invalid(problem) }
                world.connectors[index].region = region
            }
            switch update.cap {
            case .keep: break
            case .remove: world.connectors[index].monthlyCapUSD = nil
            case .set(let usd): world.connectors[index].monthlyCapUSD = usd
            }
            let connector = world.connectors[index]
            let cap = connector.monthlyCapUSD.map { " · cap \(Money.short($0)) / month" } ?? ""
            world.connectors[index].detail = "\(connector.profile ?? connector.name) · \(connector.region ?? "us-east-1")" + cap
        }
        emit([.connectorsChanged(world.connectors)])
        return world.connectors
    }

    public func checkConnector(id: String) async throws -> ConnectorCheck {
        guard let index = world.connectors.firstIndex(where: { $0.id == id }) else { throw RatchetClientError.notFound("connector \(id)") }
        let connector = world.connectors[index]
        if connector.state == .expired || connector.state == .signedOut {
            return ConnectorCheck(ok: false, detail: "\(connector.name) needs signing in again.")
        }
        return ConnectorCheck(ok: true, detail: connector.kind == .github ? "GitHub says you are signed in as \(connector.name)." : "\(connector.name) answered.")
    }

    public func refreshConnectors() async throws -> [Connector] {
        world.connectors
    }

    public func awsProfiles() async throws -> AwsProfiles {
        AwsProfiles(
            profiles: [
                AwsProfile(name: "acme-prod", kind: .sso, ssoSession: "acme"),
                AwsProfile(name: "personal-dev", kind: .keys),
                AwsProfile(name: "acme-staging", kind: .sso, ssoSession: "acme"),
                AwsProfile(name: "sandbox-eu", kind: .role),
            ],
            ssoSessions: ["acme"], filesFound: true)
    }

    public func detectedHarnesses(redetect: Bool) async throws -> [DetectedHarness] {
        if redetect { redetected = true }
        return [
            DetectedHarness(
                harness: .claudeCode, installed: true, version: "2.1.314", auth: .signedIn,
                models: [
                    HarnessModel(id: "opus", title: "Opus", harness: .claudeCode),
                    HarnessModel(id: "sonnet", title: "Sonnet", harness: .claudeCode),
                ],
                defaultModel: "opus"),
            DetectedHarness(harness: .codex, installed: redetected, version: redetected ? "0.166" : nil, auth: .signedOut),
        ]
    }
}
