import Foundation
import HTTPTypes
import OpenAPIRuntime
import Testing
@testable import RatchetKit

// The assertions of the Electron renderer's tests for New optimization (repo search, ratchet.yaml review, the template
// pull request, "What's missing"), Connectors (tokens, AWS profiles, editing in place, Check connection, re-detect, the
// keychain notice) and Advanced (development), ported to the model, the store and the daemon mapping.
// Sources: fc503b1:packages/desktop/src/renderer/{screens,connectors}.test.tsx and shared/live-settings.ts.

private let githubToken = "ghp_" + String(repeating: "a", count: 36)

// MARK: - Format checks (live-settings.ts)

@Suite("Setup: format checks")
struct SettingsCheckTests {
    @Test func amiIdsHaveTheirOwnShape() {
        #expect(SettingsCheck.problem(for: .benchAMI, "") == nil)
        #expect(SettingsCheck.problem(for: .benchAMI, "ami-0123456789abcdef0") == nil)
        #expect(SettingsCheck.problem(for: .runnerAMI, "ami-1234abcd") == nil)
        #expect(SettingsCheck.problem(for: .runnerAMI, "  ami-1234abcd  ") == nil)
        #expect(SettingsCheck.problem(for: .benchAMI, "ami-xyz")?.hasPrefix("Use an AMI id such as ami-") == true)
        #expect(SettingsCheck.problem(for: .benchAMI, "i-0123456789abcdef0") != nil)
    }

    @Test func theTofuBinaryIsAPathOrACommand() {
        #expect(SettingsCheck.problem(for: .tofuBinary, "") == nil)
        #expect(SettingsCheck.problem(for: .tofuBinary, "tofu") == nil)
        #expect(SettingsCheck.problem(for: .tofuBinary, "/opt/homebrew/bin/tofu") == nil)
        #expect(SettingsCheck.problem(for: .tofuBinary, "/Users/me/My Tools/tofu") == nil)
        #expect(SettingsCheck.problem(for: .tofuBinary, "my tofu")?.contains("absolute path") == true)
        #expect(SettingsCheck.problem(for: .tofuBinary, "-version") != nil)
        #expect(SettingsCheck.problem(for: .tofuBinary, "tofu; rm -rf /") != nil)
        #expect(SettingsCheck.problem(for: .tofuBinary, "$(tofu)") != nil)
    }

    @Test func theModuleDirectoryIsAbsolute() {
        #expect(SettingsCheck.problem(for: .moduleDir, "") == nil)
        #expect(SettingsCheck.problem(for: .moduleDir, "/work/ratchet/infra/aws") == nil)
        #expect(SettingsCheck.problem(for: .moduleDir, "~/ratchet/infra/aws") == nil)
        #expect(SettingsCheck.problem(for: .moduleDir, "infra/aws") == "Use an absolute path")
        #expect(SettingsCheck.problem(for: .moduleDir, "/a/'b'") != nil)
    }

    @Test func problemsAreFoundPerField() {
        var settings = AdvancedSettings(benchAMI: "nope", moduleDir: "/ok")
        #expect(SettingsCheck.problems(in: settings).keys.sorted { $0.rawValue < $1.rawValue } == [.benchAMI])
        settings.benchAMI = "ami-0123456789abcdef0"
        #expect(SettingsCheck.problems(in: settings).isEmpty)
    }

    @Test func regionsAndCaps() {
        for good in ["us-east-1", "eu-west-2", "ap-southeast-2", "us-gov-west-1", ""] {
            #expect(SettingsCheck.regionProblem(good) == nil, "\(good)")
        }
        for bad in ["useast1", "us-east-1x", "US-EAST-1", "us-east"] {
            #expect(SettingsCheck.regionProblem(bad)?.hasPrefix("Use a region id") == true, "\(bad)")
        }
        #expect(SettingsCheck.cap("").value == nil && SettingsCheck.cap("").problem == nil)
        #expect(SettingsCheck.cap(" 500 ").value == 500)
        #expect(SettingsCheck.cap("12.5").value == 12.5)
        for bad in ["abc", "-5", "0", "inf"] {
            #expect(SettingsCheck.cap(bad).problem == "Use a positive number of dollars", "\(bad)")
        }
    }

    @Test func pastedSecretsAreCheckedBeforeAnythingIsSent() {
        #expect(SecretKind.githubToken.problem(githubToken) == nil)
        #expect(SecretKind.githubToken.problem("github_pat_" + String(repeating: "B", count: 30)) == nil)
        #expect(SecretKind.githubToken.problem(String(repeating: "a", count: 40)) == nil)
        #expect(SecretKind.githubToken.problem("hello")?.hasPrefix("GitHub tokens start with") == true)
        #expect(SecretKind.githubToken.problem(githubToken + " ") == "Remove spaces and line breaks around the value")

        #expect(SecretKind.anthropicKey.problem("sk-ant-api03-abcdefgh") == nil)
        #expect(SecretKind.anthropicKey.problem("sk-proj-abcdefghijklmnop")?.hasPrefix("Anthropic keys start with sk-ant-") == true)
        #expect(SecretKind.openaiKey.problem("sk-abcdefghijklmnopqrstuvwx") == nil)
        #expect(SecretKind.openaiKey.problem("sk-ant-abcdefghijklmnop") != nil)
        #expect(AgentCLI.codex.secretKind == .openaiKey && AgentCLI.claudeCode.secretKind == .anthropicKey)
        #expect(SecretKind.xaiKey.problem("xai-abcdefghijklmnopqrstuv") == nil)
        #expect(SecretKind.xaiKey.problem("sk-abcdefghijklmnopqrstuvwx")?.hasPrefix("xAI keys start with xai-") == true)
        #expect(AgentCLI.grok.secretKind == .xaiKey && AgentCLI.grok.vendor == "xAI" && AgentCLI.grok.title == "Grok")
    }

    @Test func anAgentIsNamedInFreeText() {
        #expect(AgentCLI(named: "Grok · team key") == .grok)
        #expect(AgentCLI(named: "codex") == .codex)
        #expect(AgentCLI(named: "Claude Code") == .claudeCode)
        #expect(AgentCLI(named: "something else") == .claudeCode)
    }

    @Test func theAddButtonNeedsALongEnoughKeyWithNoProblem() {
        #expect(!SecretKind.githubToken.isAcceptable(""))
        #expect(!SecretKind.githubToken.isAcceptable("ghp_x"))
        #expect(SecretKind.githubToken.isAcceptable(githubToken))
    }
}

// MARK: - New optimization, step 1 and "What's missing"

@MainActor
@Suite("Setup: repo step")
struct RepoStepTests {
    private func make() async -> (NewOptimizationFlow, MockRatchetClient) {
        let client = MockRatchetClient(now: testNow)
        let flow = NewOptimizationFlow(client: client)
        await flow.load()
        flow.goal = "Make checkout p95 faster"
        return (flow, client)
    }

    @Test func searchingNarrowsTheListAndAnEmptyQueryBringsItBack() async {
        let (flow, _) = await make()
        #expect(flow.shownRepos.count == 6)
        flow.repoQuery = "shop"
        await flow.search()
        #expect(flow.shownRepos.map(\.id) == ["acme-org/shop-api"])
        flow.repoQuery = "zzz"
        await flow.search()
        #expect(flow.shownRepos.isEmpty)
        flow.repoQuery = "   "
        await flow.search()
        #expect(flow.searchResults == nil)
        #expect(flow.shownRepos.count == 6)
    }

    @Test func aSearchedRepoCanBeChosen() async {
        let (flow, _) = await make()
        flow.repoQuery = "notes"
        await flow.search()
        await flow.selectRepo("srikanth235/notes-sync")
        #expect(flow.repoID == "srikanth235/notes-sync")
        #expect(flow.branch == "trunk")
        #expect(flow.localFolder == nil)
    }

    @Test func aFoundRatchetYamlIsReviewedReadOnly() async throws {
        let (flow, _) = await make()
        let inspection = try #require(flow.inspection)
        #expect(inspection.outcome == .ready)
        #expect(flow.repoIsUsable)
        let config = try #require(inspection.config)
        #expect(config.metricName == "p95")
        #expect(config.direction == "lower is better")
        #expect(config.evaluatorIsTemplate)
        #expect(config.protected.map(\.glob) == ["tests/**", "bench/**"])
        #expect(inspection.configYAML?.contains("direction: minimize") == true)
        #expect(flow.harness?.summary == "Found ratchet.yaml: p95, lower is better.")
        #expect(!flow.showsYAML)
    }

    @Test func anInvalidRatchetYamlSaysWhyAndStopsTheFlow() async throws {
        let (flow, _) = await make()
        await flow.selectRepo("acme-org/search-svc")
        let inspection = try #require(flow.inspection)
        #expect(inspection.outcome == .invalid)
        #expect(inspection.config == nil)
        #expect(inspection.problems.map(\.path) == ["objective.direction", "guardrails[0].peak_rss"])
        #expect(inspection.problems.allSatisfy { !$0.message.isEmpty })
        #expect(!flow.repoIsUsable)
        #expect(!flow.canContinue)
        #expect(flow.harness?.found == false)
    }

    @Test func aRepoWithoutRatchetYamlOffersTemplatesAndOpensAPullRequest() async throws {
        let (flow, _) = await make()
        await flow.selectRepo("acme-org/web-app")
        let inspection = try #require(flow.inspection)
        #expect(inspection.outcome == .missing)
        #expect(inspection.templates.count == 3)
        #expect(flow.canContinue)

        await flow.proposeTemplate("build-size")
        let proposal = try #require(flow.proposal)
        #expect(proposal.prURL.contains("/acme-org/web-app/pull/"))
        #expect(proposal.url != nil)
        #expect(proposal.branch == "ratchet/add-build-size")
        #expect(proposal.files.contains("ratchet.yaml"))
        #expect(proposal.summary.contains("Merge it"))
        #expect(flow.proposingTemplateID == nil)
        #expect(flow.proposalError == nil)

        // "I merged it: check again" reads the repo again and clears the result.
        await flow.recheck()
        #expect(flow.proposal == nil)
        #expect(flow.inspection != nil)
    }

    @Test func aTemplateThatCannotBeProposedShowsTheReason() async {
        let (flow, _) = await make()
        await flow.selectRepo("acme-org/web-app")
        await flow.proposeTemplate("demo-search")
        #expect(flow.proposal == nil)
        #expect(flow.proposalError == "That template cannot be added to a repo.")
    }

    @Test func aLocalFolderIsReadThroughItsRemote() async throws {
        let (flow, _) = await make()
        await flow.chooseLocalFolder("/Users/dev/shop-api")
        #expect(flow.localFolder == "/Users/dev/shop-api")
        #expect(flow.repoID == "srikanth235/shop-api")
        #expect(flow.inspection?.source == .local("/Users/dev/shop-api"))
        #expect(flow.repoIsUsable)
        #expect(flow.repos.contains { $0.id == "srikanth235/shop-api" })
        #expect(flow.canContinue)
    }

    @Test func aFolderWithoutAGitHubRemoteSaysWhyItCannotBeUsed() async throws {
        let (flow, _) = await make()
        await flow.chooseLocalFolder("/Users/dev/scratch-no-remote")
        let inspection = try #require(flow.inspection)
        guard case .unusable(let why) = inspection.outcome else {
            Issue.record("expected an unusable folder, got \(inspection.outcome)")
            return
        }
        #expect(why.contains("no GitHub remote"))
        #expect(!flow.repoIsUsable)
        #expect(!flow.canContinue)
        #expect(flow.harness?.summary == why)
        // Choosing a repo of the account again leaves the folder.
        await flow.selectRepo("acme-org/shop-api")
        #expect(flow.localFolder == nil)
        #expect(flow.canContinue)
    }

    @Test func notBeingSignedInToGitHubIsKnownFromTheConnectors() async {
        let (flow, _) = await make()
        #expect(flow.isSignedInToGitHub)
        let none = NewOptimizationFlow(client: MockRatchetClient(now: testNow))
        #expect(!none.isSignedInToGitHub)
    }

    // MARK: What's missing

    @Test func whatIsMissingBlocksContinueAndNamesEachThing() async {
        let (flow, client) = await make()
        await client.simulate(preflight: [
            PreflightItem(id: "codex", label: "Codex is not signed in", state: .fail, detail: "Run `codex login`."),
            PreflightItem(id: "aws", label: "acme-staging needs signing in", state: .warn, detail: "Run `aws sso login`."),
        ])
        await flow.go(to: .connections)
        #expect(flow.preflight.count == 2)
        #expect(flow.blockers.map(\.id) == ["codex"])
        #expect(!flow.canContinue)

        await client.simulate(preflight: nil)
        await flow.refreshPreflight()
        #expect(flow.preflight.isEmpty)
        #expect(flow.canContinue)
    }

    @Test func aWarningAloneDoesNotBlock() async {
        let (flow, client) = await make()
        await client.simulate(preflight: [PreflightItem(id: "w", label: "Slow", state: .warn, detail: "")])
        await flow.go(to: .connections)
        #expect(flow.blockers.isEmpty)
        #expect(flow.canContinue)
    }

    @Test func theCheckFollowsTheChosenAgentsAndWhereTheyRun() async {
        let (flow, _) = await make()
        await flow.go(to: .connections)
        let before = flow.preflightKey
        flow.togglePlacement("agent-claude-team")
        #expect(flow.preflightKey != before)
        flow.toggleAgent("agent-claude-login")
        #expect(flow.preflightKey.contains("agent-claude-team:local") || flow.preflightKey.contains("agent-claude-team:cloud"))
        #expect(!flow.preflightKey.contains("agent-claude-login"))
    }

    @Test func theMockFlagsAnAgentThatCannotRunWhereItWasAsked() async throws {
        let client = MockRatchetClient(now: testNow)
        let items = try await client.preflight(PreflightRequest(
            source: .github, repo: "acme-org/shop-api", agentIDs: ["agent-claude-login"], placement: .cloud))
        #expect(items.map(\.state) == [.fail])
        let ok = try await client.preflight(PreflightRequest(
            source: .github, repo: "acme-org/shop-api", agentIDs: ["agent-claude-team"], placement: .cloud))
        #expect(ok.isEmpty)
        let demo = try await client.preflight(PreflightRequest(source: .demo, agentIDs: [], placement: .local))
        #expect(demo.isEmpty)
    }

    // MARK: Discarding a draft

    @Test func aDraftThatWasNotApprovedCanBeDiscarded() async throws {
        let (flow, _) = await make()
        await flow.go(to: .environment)
        #expect(flow.draft?.state == .draft)
        await flow.discardDraft()
        #expect(flow.draft == nil)
        #expect(flow.step == .connections)
        // Going to the environment again drafts a new one.
        await flow.go(to: .environment)
        #expect(flow.draft != nil)
    }

    @Test func anApprovedDraftStays() async {
        let (flow, _) = await make()
        await flow.go(to: .environment)
        await flow.approveDraft()
        await flow.discardDraft()
        #expect(flow.draft?.state == .approved)
        #expect(flow.step == .environment)
    }

    @Test func theMockRefusesToDiscardAnApprovedOrUnknownDraft() async throws {
        let client = MockRatchetClient(now: testNow)
        let draft = try await client.createDraft(DraftRequest(repoID: "acme-org/shop-api", goal: "Go"))
        _ = try await client.approveDraft(id: draft.id)
        await #expect(throws: RatchetClientError.self) { try await client.discardDraft(id: draft.id) }
        await #expect(throws: RatchetClientError.self) { try await client.discardDraft(id: "nope") }
    }
}

// MARK: - "Use template"

@MainActor
@Suite("Setup: Use template")
struct UseTemplateTests {
    private func make(_ id: String = "build-size") async throws -> (TemplateProposalFlow, MockRatchetClient) {
        let client = MockRatchetClient(now: testNow)
        let templates = try await client.templates()
        let template = try #require(templates.first { $0.id == id })
        let flow = TemplateProposalFlow(template: template, client: client)
        await flow.load()
        return (flow, client)
    }

    @Test func pickARepoThenOpenThePullRequest() async throws {
        let (flow, _) = try await make()
        #expect(flow.isSignedIn && flow.isLoaded)
        #expect(flow.targetLine == "Choose a repo.")
        #expect(!flow.canOpen)
        flow.query = "web"
        #expect(flow.shown.map(\.id) == ["acme-org/web-app"])
        flow.select(try #require(flow.shown.first))
        #expect(flow.targetLine == "Target: acme-org/web-app (main)")
        #expect(flow.canOpen)
        await flow.openPullRequest()
        let proposal = try #require(flow.proposal)
        #expect(proposal.templateID == "build-size")
        #expect(proposal.prURL.contains("/acme-org/web-app/pull/"))
        #expect(!flow.canOpen)
        // Choosing another repo starts over.
        flow.select(try #require(flow.repos.first))
        #expect(flow.proposal == nil)
    }

    @Test func aDemoCannotBeProposed() async throws {
        let (flow, _) = try await make("demo-search")
        flow.select(try #require(flow.repos.first))
        #expect(!flow.canOpen)
    }

    @Test func withoutAGitHubAccountThereIsNothingToChooseFrom() async throws {
        let client = MockRatchetClient(now: testNow)
        for connector in try await client.connectors() where connector.kind == .github {
            try await client.removeConnector(id: connector.id)
        }
        let templates = try await client.templates()
        let template = try #require(templates.first { $0.id == "build-size" })
        let flow = TemplateProposalFlow(template: template, client: client)
        await flow.load()
        #expect(flow.isLoaded && !flow.isSignedIn)
        #expect(flow.repos.isEmpty)
    }
}

// MARK: - Connectors

@MainActor
@Suite("Setup: connector forms")
struct ConnectorFormTests {
    private func make() async -> (AppStore, MockRatchetClient) {
        let client = MockRatchetClient(now: testNow)
        let store = AppStore(client: client)
        await store.refresh()
        return (store, client)
    }

    @Test func aPastedTokenIsKeptButNeverShownAgain() async throws {
        let (store, _) = await make()
        let before = store.connectors.count
        try await store.addGithubToken(githubToken, name: "octocat-bot")
        #expect(store.connectors.count == before + 1)
        let added = try #require(store.connectors.first { $0.name == "octocat-bot" })
        #expect(added.kind == .github && added.origin == .token && added.state == .connected)
        let encoded = try JSONEncoder().encode(store.connectors)
        #expect(!String(decoding: encoded, as: UTF8.self).contains(githubToken))
    }

    @Test func aTokenGitHubRejectsOrOfTheWrongShapeAddsNothing() async throws {
        let (store, _) = await make()
        let before = store.connectors.count
        await #expect(throws: RatchetClientError.self) { try await store.addGithubToken("nope") }
        do {
            try await store.addGithubToken("ghp_" + String(repeating: "a", count: 32) + "0000")
            Issue.record("a rejected token must not be added")
        } catch let error as RatchetClientError {
            #expect(error.localizedDescription.contains("rejected"))
        }
        #expect(store.connectors.count == before)
    }

    @Test func withoutAKeychainTheFormsSayWhy() async throws {
        let (store, client) = await make()
        #expect(store.connectorSetup.encryptionAvailable)
        await client.simulate(keychainAvailable: false)
        await store.refresh()
        #expect(!store.connectorSetup.encryptionAvailable)
        await #expect(throws: RatchetClientError.self) { try await store.addGithubToken(githubToken) }
    }

    @Test func awsProfilesFromTheMachineExcludeTheOnesAlreadyConnected() async throws {
        let (store, client) = await make()
        let profiles = try await client.awsProfiles()
        #expect(profiles.profiles.map(\.name) == ["acme-prod", "personal-dev", "acme-staging", "sandbox-eu"])
        #expect(profiles.profiles.first?.hint == "SSO · session acme")
        #expect(profiles.profiles.first { $0.name == "personal-dev" }?.hint == "access keys")
        let available = profiles.available(excluding: store.connectors)
        #expect(available.map(\.name) == ["sandbox-eu"])
        #expect(profiles.emptyNote(excluding: store.connectors) == nil)

        try await store.addConnectorChecked(NewConnector(kind: .aws, name: "sandbox-eu", region: "eu-west-2", monthlyCapUSD: 500))
        let added = try #require(store.connectors.first { $0.profile == "sandbox-eu" })
        #expect(added.region == "eu-west-2" && added.monthlyCapUSD == 500 && !added.isDefault)
        #expect(profiles.available(excluding: store.connectors).isEmpty)
        #expect(profiles.emptyNote(excluding: store.connectors) == "Every profile in ~/.aws is already connected.")
    }

    @Test func theNotesForAMachineWithoutProfiles() {
        #expect(AwsProfiles(filesFound: false).emptyNote(excluding: []) == "No ~/.aws/config or ~/.aws/credentials on this Mac.")
        #expect(AwsProfiles(filesFound: true).emptyNote(excluding: []) == "No profiles found in ~/.aws/config or ~/.aws/credentials.")
        #expect(AwsProfile(name: "a", kind: .role).hint == "assume role")
    }

    @Test func anAwsConnectorIsEditedInPlace() async throws {
        let (store, _) = await make()
        try await store.updateConnector("aws-personal-dev", ConnectorUpdate(region: "eu-west-1", cap: .remove))
        var edited = try #require(store.connectors.first { $0.id == "aws-personal-dev" })
        #expect(edited.region == "eu-west-1" && edited.monthlyCapUSD == nil)
        #expect(edited.detail.contains("eu-west-1") && !edited.detail.contains("cap"))
        try await store.updateConnector("aws-personal-dev", ConnectorUpdate(cap: .set(250)))
        edited = try #require(store.connectors.first { $0.id == "aws-personal-dev" })
        #expect(edited.monthlyCapUSD == 250 && edited.region == "eu-west-1")
        try await store.updateConnector("aws-personal-dev", ConnectorUpdate(region: "us-west-2", cap: .keep))
        #expect(store.connectors.first { $0.id == "aws-personal-dev" }?.monthlyCapUSD == 250)
        await #expect(throws: RatchetClientError.self) {
            try await store.updateConnector("aws-personal-dev", ConnectorUpdate(region: "mars"))
        }
    }

    @Test func renamingKeepsTheRestAndNeedsAName() async throws {
        let (store, _) = await make()
        try await store.updateConnector("gh-acme-org", ConnectorUpdate(name: "  Acme organisation "))
        let renamed = try #require(store.connectors.first { $0.id == "gh-acme-org" })
        #expect(renamed.name == "Acme organisation" && renamed.state == .connected)
        await #expect(throws: RatchetClientError.self) { try await store.updateConnector("gh-acme-org", ConnectorUpdate(name: "  ")) }
        await #expect(throws: RatchetClientError.self) { try await store.updateConnector("nope", ConnectorUpdate(name: "x")) }
    }

    @Test func anAccountFromTheEnvironmentCannotBeRenamedOrRemoved() {
        let env = Connector(id: "gh-env", kind: .github, name: "env", detail: "GITHUB_TOKEN", state: .connected, origin: .environment)
        let token = Connector(id: "gh-t", kind: .github, name: "t", detail: "", state: .connected, origin: .token)
        #expect(env.isFromEnvironment)
        #expect(!token.isFromEnvironment)
    }

    @Test func checkConnectionAnswersInOneLine() async throws {
        let (store, _) = await make()
        let ok = try await store.checkConnector("gh-srikanth235")
        #expect(ok.ok && ok.detail.contains("srikanth235"))
        let expired = try await store.checkConnector("aws-acme-staging")
        #expect(!expired.ok && expired.detail.contains("signing in"))
        await #expect(throws: RatchetClientError.self) { try await store.checkConnector("nope") }
    }

    @Test func refreshAllKeepsTheList() async {
        let (store, _) = await make()
        let before = store.connectors
        await store.refreshConnectors()
        #expect(store.connectors == before)
        #expect(store.lastError == nil)
    }

    @Test func reDetectPicksUpANewlyInstalledCLI() async throws {
        let (store, client) = await make()
        try await store.client.removeConnector(id: "agent-codex-login")
        await store.refreshConnectors()
        let seen = try await client.detectedAgents(redetect: false)
        let connected = try await client.connectors()
        #expect(DetectedAgent.addable(seen, connected: connected).isEmpty)
        #expect(DetectedAgent.emptyNote(seen) == "Your Claude Code, Codex and Grok logins are already listed.")

        let after = try await client.detectedAgents(redetect: true)
        let addable = DetectedAgent.addable(after, connected: connected)
        #expect(addable.map(\.cli) == [.codex])
        #expect(addable.first?.title == "Codex · v0.166")

        try await store.addConnectorChecked(NewConnector(kind: .agent, name: AgentCLI.codex.title))
        let login = try #require(store.connectors.first { $0.cli == .codex })
        #expect(login.origin == .login && !login.canRunInCloud)
        #expect(DetectedAgent.addable(after, connected: store.connectors).isEmpty)
    }

    @Test func noCLIAtAllHasItsOwnNote() {
        let none = AgentCLI.allCases.map { DetectedAgent(cli: $0, installed: false) }
        #expect(DetectedAgent.emptyNote(none) == "None of Claude Code, Codex or Grok was found.")
    }

    @Test func anAgentKeyIsListedAsACloudCapableKey() async throws {
        let (store, _) = await make()
        try await store.addConnectorChecked(NewConnector(kind: .agent, name: "Claude Code", secret: "sk-ant-api03-abcdefgh"))
        let key = try #require(store.connectors.last { $0.kind == .agent })
        #expect(key.origin == .apiKey && key.canRunInCloud && key.cli == .claudeCode)
        let encoded = try JSONEncoder().encode(store.connectors)
        #expect(!String(decoding: encoded, as: UTF8.self).contains("sk-ant-api03"))
    }
}

// MARK: - Advanced (development)

@Suite("Setup: advanced settings")
struct AdvancedSettingsTests {
    @Test func theOverridesAreSavedAndCleared() async throws {
        let client = MockRatchetClient(now: testNow)
        let start = try await client.advancedSettings()
        #expect(start.runnerAMI.isEmpty && start.benchAMI.isEmpty)
        #expect(start.placeholder(for: .benchAMI) == "ami-0123456789abcdef0")
        #expect(start.placeholder(for: .tofuBinary) == start.defaultTofuBinary)

        var edited = start
        edited.benchAMI = " ami-0123456789abcdef0 "
        edited.tofuBinary = "/opt/homebrew/bin/tofu"
        let saved = try await client.saveAdvancedSettings(edited)
        #expect(saved.benchAMI == "ami-0123456789abcdef0")
        #expect(saved.tofuBinary == "/opt/homebrew/bin/tofu")
        let reread = try await client.advancedSettings()
        #expect(reread.benchAMI == "ami-0123456789abcdef0")

        edited.tofuBinary = ""
        let cleared = try await client.saveAdvancedSettings(edited)
        #expect(cleared.tofuBinary.isEmpty)
    }

    @Test func nothingIsSavedWhenAFieldIsInvalid() async throws {
        let client = MockRatchetClient(now: testNow)
        let bad = AdvancedSettings(runnerAMI: "ami-0123456789abcdef0", moduleDir: "relative/dir")
        await #expect(throws: RatchetClientError.self) { try await client.saveAdvancedSettings(bad) }
        let unchanged = try await client.advancedSettings()
        #expect(unchanged.runnerAMI.isEmpty)
        var good = bad
        good.moduleDir = "/abs/dir"
        let saved = try await client.saveAdvancedSettings(good)
        #expect(saved.moduleDir == "/abs/dir")
    }

    @Test func eachFieldHasALabelAndAHint() {
        #expect(AdvancedField.allCases.map(\.label) == ["Runner AMI", "Bench AMI", "OpenTofu binary", "OpenTofu module directory"])
        #expect(AdvancedField.allCases.allSatisfy { !$0.hint.isEmpty })
    }
}

// MARK: - The daemon routes

/// A stand-in for the daemon that keeps the whole path (with its query) and the body of every request.
private final class RouteRecorder: ClientTransport, @unchecked Sendable {
    struct Seen {
        var method: String
        var path: String
        var body: String
        var json: [String: Any] {
            (try? JSONSerialization.jsonObject(with: Data(body.utf8))) as? [String: Any] ?? [:]
        }
    }

    private let lock = NSLock()
    private var routes: [String: (Int, String)] = [:]
    private var log: [Seen] = []

    func route(_ key: String, _ status: Int = 200, _ body: String = "") {
        lock.withLock { routes[key] = (status, body) }
    }

    var seen: [Seen] { lock.withLock { log } }

    func last(_ method: String, _ path: String) -> Seen? {
        seen.last { $0.method == method && $0.path.split(separator: "?").first.map(String.init) == path }
    }

    func send(_ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String) async throws -> (HTTPResponse, HTTPBody?) {
        var text = ""
        if let body {
            for try await chunk in body { text += String(decoding: chunk, as: UTF8.self) }
        }
        let path = request.path ?? ""
        let key = "\(request.method.rawValue) \(path.split(separator: "?").first.map(String.init) ?? path)"
        let reply = lock.withLock { () -> (Int, String)? in
            log.append(Seen(method: request.method.rawValue, path: path, body: text))
            return routes[key]
        }
        guard let reply else {
            var response = HTTPResponse(status: .init(code: 404))
            response.headerFields[.contentType] = "application/json"
            return (response, HTTPBody(#"{"code":"not_found","message":"no such route"}"#))
        }
        var response = HTTPResponse(status: .init(code: reply.0))
        response.headerFields[.contentType] = "application/json"
        return (response, reply.1.isEmpty ? nil : HTTPBody(reply.1))
    }
}

@Suite("Setup: daemon routes")
struct SetupDaemonTests {
    private func make(_ recorder: RouteRecorder) -> DaemonRatchetClient { DaemonRatchetClient(transport: recorder) }

    private let inspectWithoutConfig = #"{"source":{"kind":"github","repo":"acme/bare"},"repo":"acme/bare","baseRef":"main","configYaml":null,"config":null,"errors":[],"templates":[],"problem":null}"#
    private let inspectInvalid = #"{"source":{"kind":"github","repo":"acme/bad"},"repo":"acme/bad","baseRef":"main","configYaml":"objective: [","config":null,"errors":[{"path":"objective","message":"expected a mapping"}],"templates":[],"problem":null}"#
    private let inspectNoRemote = #"{"source":{"kind":"local","path":"/work/x"},"repo":null,"baseRef":"main","configYaml":null,"config":null,"errors":[],"templates":[],"problem":"This folder has no GitHub remote."}"#

    // MARK: Repos

    @Test func theSearchTextIsEscapedIntoTheQuery() async throws {
        let recorder = RouteRecorder()
        recorder.route("GET /v1/github/repos", 200, Fixtures.repos)
        let client = make(recorder)
        let repos = try await client.searchRepos(query: " shop&api ")
        #expect(repos.first?.id == "acme-org/shop-api")
        #expect(repos.first?.isPrivate == true)
        #expect(repos.first?.summary == "Checkout and orders API")
        #expect(recorder.seen.last?.path == "/v1/github/repos?query=shop%26api")
        _ = try await client.searchRepos(query: "  ")
        #expect(recorder.seen.last?.path == "/v1/github/repos")
    }

    @Test func notBeingSignedInIsAnEmptySearchNotAnError() async throws {
        let recorder = RouteRecorder()
        recorder.route("GET /v1/github/repos", 409, #"{"code":"not_ready","message":"sign in to GitHub first"}"#)
        #expect(try await make(recorder).searchRepos(query: "x").isEmpty)
        recorder.route("GET /v1/github/repos", 502, #"{"code":"upstream","message":"GitHub is down"}"#)
        await #expect(throws: RatchetClientError.self) { try await make(recorder).searchRepos(query: "x") }
    }

    @Test func aLocalFolderIsSentByPathAndTheConfigIsMapped() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/repos/inspections", 200, Fixtures.inspect)
        let inspection = try await make(recorder).inspectRepo(.local("/work/search-api"))
        let source = recorder.last("POST", "/v1/repos/inspections")?.json["source"] as? [String: Any]
        #expect(source?["kind"] as? String == "local")
        #expect(source?["path"] as? String == "/work/search-api")
        #expect(inspection.outcome == .ready)
        #expect(inspection.config?.metricName == "p95")
        #expect(inspection.config?.lowerIsBetter == true)
        #expect(inspection.configYAML?.contains("direction: minimize") == true)
        #expect(inspection.harness.found)
    }

    @Test func aGitHubRepoIsSentByName() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/repos/inspections", 200, inspectWithoutConfig)
        let inspection = try await make(recorder).inspectRepo(.github("acme/bare"))
        let source = recorder.last("POST", "/v1/repos/inspections")?.json["source"] as? [String: Any]
        #expect(source?["kind"] as? String == "github" && source?["repo"] as? String == "acme/bare")
        #expect(inspection.outcome == .missing)
        #expect(inspection.harness.summary.contains("template"))
    }

    @Test func anInvalidConfigKeepsItsErrors() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/repos/inspections", 200, inspectInvalid)
        let inspection = try await make(recorder).inspectRepo(.github("acme/bad"))
        #expect(inspection.outcome == .invalid)
        #expect(inspection.problems == [ConfigProblem(path: "objective", message: "expected a mapping")])
        #expect(inspection.configYAML == "objective: [")
    }

    @Test func aFolderWithoutARemoteComesBackWithItsReason() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/repos/inspections", 200, inspectNoRemote)
        let inspection = try await make(recorder).inspectRepo(.local("/work/x"))
        #expect(inspection.outcome == .unusable("This folder has no GitHub remote."))
        #expect(inspection.repo == nil)
        #expect(!inspection.isUsable)
    }

    // MARK: Preflight and template pull requests

    @Test func preflightSendsTheAgentKindsAndWhereTheyRun() async throws {
        let recorder = RouteRecorder()
        recorder.route("GET /v1/connectors", 200, Fixtures.connectors)
        recorder.route("POST /v1/preflight", 200, #"[{"id":"codex","label":"Codex is not signed in","state":"fail","detail":"Run `codex login`."},{"id":"aws","label":"AWS","state":"ok","detail":""}]"#)
        let client = make(recorder)
        let agents = try await client.connectors().filter { $0.kind == .agent }.map(\.id)
        let items = try await client.preflight(PreflightRequest(source: .github, repo: "acme/x", agentIDs: agents, placement: .cloud))
        let sent = try #require(recorder.last("POST", "/v1/preflight")?.json)
        #expect(sent["source"] as? String == "github")
        #expect(sent["agentsWhere"] as? String == "cloud")
        #expect(sent["repo"] as? String == "acme/x")
        #expect(sent["agents"] as? [String] == ["claude-code"])
        #expect(items.map(\.state) == [.fail, .ok])
        #expect(items.first?.detail == "Run `codex login`.")
    }

    @Test func aDemoChecksEveryAgentOnThisMac() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/preflight", 200, "[]")
        _ = try await make(recorder).preflight(PreflightRequest(source: .demo, repo: "ignored", agentIDs: [], placement: .cloud))
        let sent = try #require(recorder.last("POST", "/v1/preflight")?.json)
        #expect(sent["agents"] as? [String] == ["claude-code", "codex", "grok"])
        #expect(sent["agentsWhere"] as? String == "local")
        #expect(sent["repo"] == nil)
    }

    @Test func aTemplateProposalSendsTheRepoBranchAndTemplate() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/template-proposals", 201, #"{"prUrl":"https://github.com/acme/x/pull/7","branch":"ratchet/add-http-latency","sha":"abc","templateId":"http-latency","files":["ratchet.yaml","bench/k6.js"]}"#)
        let proposal = try await make(recorder).proposeTemplate(
            TemplateProposalRequest(repo: "acme/x", baseRef: "main", templateID: "http-latency"))
        let sent = try #require(recorder.last("POST", "/v1/template-proposals")?.json)
        #expect(sent["repoUrl"] as? String == "acme/x")
        #expect(sent["baseRef"] as? String == "main")
        #expect(sent["templateId"] as? String == "http-latency")
        #expect(proposal.prURL == "https://github.com/acme/x/pull/7")
        #expect(proposal.files == ["ratchet.yaml", "bench/k6.js"])
        #expect(proposal.url?.absoluteString == "https://github.com/acme/x/pull/7")
        #expect(proposal.summary == "Adds ratchet.yaml, bench/k6.js on ratchet/add-http-latency. Merge it, then choose the repository again.")
    }

    @Test func aRefusedProposalKeepsTheDaemonsSentence() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/template-proposals", 400, #"{"code":"invalid_request","message":"ratchet.yaml already exists on main"}"#)
        do {
            _ = try await make(recorder).proposeTemplate(TemplateProposalRequest(repo: "a/b", baseRef: "main", templateID: "x"))
            Issue.record("expected a refusal")
        } catch let error as RatchetClientError {
            #expect(error == .invalid("ratchet.yaml already exists on main"))
        }
    }

    // MARK: Connectors

    @Test func aPastedTokenGoesToTheDaemonOnceAndTheListComesBack() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/connectors/github", 200, Fixtures.connectors)
        let list = try await make(recorder).addGithubToken("  \(githubToken)\n", name: " bot ")
        let sent = try #require(recorder.last("POST", "/v1/connectors/github")?.json)
        #expect(sent["token"] as? String == githubToken)
        #expect(sent["name"] as? String == "bot")
        #expect(list.contains { $0.kind == .github && $0.origin == .token && $0.name == "acme-org" })
        #expect(list.contains { $0.kind == .github && $0.origin == .device })
        let encoded = try JSONEncoder().encode(list)
        #expect(!String(decoding: encoded, as: UTF8.self).contains(githubToken))
    }

    @Test func aRejectedTokenIsTheDaemonsSentence() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/connectors/github", 400, #"{"code":"invalid_request","message":"GitHub rejected the sign-in with that token."}"#)
        await #expect(throws: RatchetClientError.invalid("GitHub rejected the sign-in with that token.")) {
            try await make(recorder).addGithubToken(githubToken, name: nil)
        }
        #expect(recorder.last("POST", "/v1/connectors/github")?.json["name"] == nil)
    }

    @Test func anAwsProfileIsAddedWithItsRegionAndCap() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/connectors/aws", 200, Fixtures.connectors)
        let added = try await make(recorder).addConnector(NewConnector(kind: .aws, name: "acme-prod", region: " eu-west-2 ", monthlyCapUSD: 500))
        let sent = try #require(recorder.last("POST", "/v1/connectors/aws")?.json)
        #expect(sent["profile"] as? String == "acme-prod")
        #expect(sent["region"] as? String == "eu-west-2")
        #expect(sent["monthlyCapUsd"] as? Double == 500)
        #expect(added.kind == .aws && added.name == "acme-prod")
        #expect(added.profile != nil && added.region != nil)
    }

    @Test func removingTheCapSendsANullAndKeepingItSendsNothing() async throws {
        let recorder = RouteRecorder()
        recorder.route("PATCH /v1/connectors/aws-2fc2aa59", 200, Fixtures.connectors)
        let client = make(recorder)
        _ = try await client.updateConnector(id: "aws-2fc2aa59", ConnectorUpdate(name: " prod ", region: "eu-west-1", cap: .remove))
        var sent = try #require(recorder.last("PATCH", "/v1/connectors/aws-2fc2aa59"))
        #expect(sent.body.contains(#""monthlyCapUsd":null"#))
        #expect(sent.json["name"] as? String == "prod" && sent.json["region"] as? String == "eu-west-1")

        _ = try await client.updateConnector(id: "aws-2fc2aa59", ConnectorUpdate(cap: .set(250)))
        sent = try #require(recorder.last("PATCH", "/v1/connectors/aws-2fc2aa59"))
        #expect(sent.json["monthlyCapUsd"] as? Double == 250)
        #expect(sent.json["name"] == nil && sent.json["region"] == nil)

        _ = try await client.updateConnector(id: "aws-2fc2aa59", ConnectorUpdate(name: "x"))
        sent = try #require(recorder.last("PATCH", "/v1/connectors/aws-2fc2aa59"))
        #expect(!sent.body.contains("monthlyCapUsd"))
    }

    @Test func checkConnectionReturnsTheDaemonsOneLine() async throws {
        let recorder = RouteRecorder()
        let connector = #"{"kind":"github","id":"github-f07d9ca7","name":"srikanth235","detail":"Signed in with GitHub","status":"connected","isDefault":true,"login":"srikanth235","source":"device"}"#
        recorder.route("POST /v1/connectors/github-f07d9ca7/test", 200, #"{"ok":true,"detail":"GitHub says you are signed in as srikanth235.","connector":\#(connector)}"#)
        let check = try await make(recorder).checkConnector(id: "github-f07d9ca7")
        #expect(check == ConnectorCheck(ok: true, detail: "GitHub says you are signed in as srikanth235."))
    }

    @Test func refreshAllIsOnePostThatReturnsTheList() async throws {
        let recorder = RouteRecorder()
        recorder.route("POST /v1/connectors/refresh", 200, Fixtures.connectors)
        let list = try await make(recorder).refreshConnectors()
        #expect(list.count == 7)
        #expect(recorder.seen.count == 1)
    }

    @Test func theKeychainNoticeComesFromTheConnectorList() async throws {
        let recorder = RouteRecorder()
        recorder.route("GET /v1/connectors", 200, Fixtures.connectors.replacingOccurrences(of: #""encryptionAvailable":true"#, with: #""encryptionAvailable":false"#))
        let setup = try await make(recorder).connectorSetup()
        #expect(!setup.encryptionAvailable)
        #expect(setup.clientIdConfigured)
    }

    @Test func connectorsKnowTheirOriginAndWhatTheyDrive() async throws {
        let recorder = RouteRecorder()
        recorder.route("GET /v1/connectors", 200, Fixtures.connectors)
        let list = try await make(recorder).connectors()
        #expect(list.first { $0.name == "acme-prod" }?.profile == "acme-prod")
        #expect(list.first { $0.name == "acme-prod" }?.region?.isEmpty == false)
        #expect(list.first { $0.name == "Claude Code · your login" }?.origin == .login)
        #expect(list.first { $0.name == "Claude Code · team key" }?.origin == .apiKey)
        #expect(list.filter { $0.kind == .agent }.allSatisfy { $0.cli == .claudeCode })
    }

    @Test func awsProfilesAndDetectedCLIsDecode() async throws {
        let recorder = RouteRecorder()
        recorder.route("GET /v1/aws/profiles", 200, #"{"profiles":[{"name":"acme","kind":"sso","source":"config","ssoSession":"corp"},{"name":"old","kind":"weird","source":"credentials","ssoSession":null}],"ssoSessions":["corp"],"selected":"","region":"us-east-1","defaultRegion":"us-east-1","filesFound":true}"#)
        recorder.route("GET /v1/harnesses", 200, #"[{"kind":"claude-code","installed":true,"version":"2.1.4","path":"/usr/local/bin/claude","auth":"signed-in","hint":""},{"kind":"codex","installed":false,"version":null,"path":null,"auth":"signed-out","hint":"npm i -g @openai/codex"}]"#)
        recorder.route("POST /v1/harnesses/detections", 200, #"[{"kind":"codex","installed":true,"version":"0.166","path":"/x/codex","auth":"api-key","hint":""}]"#)
        let client = make(recorder)
        let profiles = try await client.awsProfiles()
        #expect(profiles.filesFound && profiles.ssoSessions == ["corp"])
        #expect(profiles.profiles == [AwsProfile(name: "acme", kind: .sso, ssoSession: "corp"), AwsProfile(name: "old", kind: .other)])

        let found = try await client.detectedAgents(redetect: false)
        #expect(found.map(\.installed) == [true, false])
        #expect(found.first?.title == "Claude Code · v2.1.4")
        let again = try await client.detectedAgents(redetect: true)
        #expect(again == [DetectedAgent(cli: .codex, installed: true, version: "0.166", auth: .apiKey)])
        #expect(recorder.seen.map(\.method) == ["GET", "GET", "POST"])
    }

    // MARK: Drafts and settings

    @Test func discardingADraftIsADeleteAndAnApprovedOneIsRefused() async throws {
        let recorder = RouteRecorder()
        recorder.route("DELETE /v1/environment-drafts/env-0001", 204)
        let client = make(recorder)
        try await client.discardDraft(id: "env-0001")
        #expect(recorder.seen.map(\.method) == ["DELETE"])

        recorder.route("DELETE /v1/environment-drafts/env-0002", 409, #"{"code":"conflict","message":"That environment was approved."}"#)
        await #expect(throws: RatchetClientError.invalid("That environment was approved.")) { try await client.discardDraft(id: "env-0002") }
    }

    private func settingsJSON(runner: String = "", tofu: String = "") -> String {
        #"{"awsProfile":"","awsRegion":"us-east-1","secrets":{"anthropicApiKey":false,"openaiApiKey":false,"githubToken":false},"advanced":{"values":{"awsProfile":"","awsRegion":"us-east-1","awsAuth":"profile","runnerImageRef":"\#(runner)","benchImageRef":"","tofuBinary":"\#(tofu)","moduleDir":""},"defaults":{"tofuBinary":"tofu","moduleDir":"/Applications/Ratchet.app/Contents/Resources/infra/aws"}},"encryptionAvailable":true,"engine":{"kind":"real","mode":"real"}}"#
    }

    @Test func theOverridesAreReadAndSavedAsAWhole() async throws {
        let recorder = RouteRecorder()
        recorder.route("GET /v1/settings", 200, settingsJSON())
        recorder.route("PUT /v1/settings/advanced", 200, settingsJSON(runner: "ami-0123456789abcdef0", tofu: "/opt/homebrew/bin/tofu"))
        let client = make(recorder)
        var settings = try await client.advancedSettings()
        #expect(settings.defaultTofuBinary == "tofu")
        #expect(settings.defaultModuleDir.hasSuffix("infra/aws"))
        #expect(settings.runnerAMI.isEmpty)

        settings.runnerAMI = " ami-0123456789abcdef0 "
        settings.tofuBinary = "/opt/homebrew/bin/tofu"
        let saved = try await client.saveAdvancedSettings(settings)
        let sent = try #require(recorder.last("PUT", "/v1/settings/advanced")?.json)
        #expect(sent["runnerImageRef"] as? String == "ami-0123456789abcdef0")
        #expect(sent["tofuBinary"] as? String == "/opt/homebrew/bin/tofu")
        #expect(sent["benchImageRef"] as? String == "" && sent["moduleDir"] as? String == "")
        #expect(saved.runnerAMI == "ami-0123456789abcdef0" && saved.tofuBinary == "/opt/homebrew/bin/tofu")
    }

    @Test func aFieldTheDaemonRefusesIsNamedInTheError() async throws {
        let recorder = RouteRecorder()
        recorder.route("PUT /v1/settings/advanced", 400, #"{"code":"invalid_request","message":"Some settings are not valid.","fields":[{"path":"runnerImageRef","message":"Use an AMI id such as ami-0123456789abcdef0"}]}"#)
        do {
            _ = try await make(recorder).saveAdvancedSettings(AdvancedSettings(runnerAMI: "nope"))
            Issue.record("expected a refusal")
        } catch let error as RatchetClientError {
            #expect(error.localizedDescription.contains("runnerImageRef"))
            #expect(error.localizedDescription.contains("Use an AMI id"))
        }
    }
}
