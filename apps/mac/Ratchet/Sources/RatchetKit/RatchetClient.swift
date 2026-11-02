import Foundation

// What the app asks of `ratchetd`, split by domain. `RatchetClient` is the composition the store holds; a screen or a
// test double can depend on only the domain it uses. `MockRatchetClient` and `DaemonRatchetClient` implement all of
// them, and one contract suite (Tests/RatchetKitTests/ClientContractTests.swift) runs against both.
// Chats are separate: see `ChatClient` (Chat/ChatClient.swift).

/// Overview, optimizations and their agents, benchmarks, templates, and the reads behind the live log and the Result details.
public protocol OptimizationsClient: Sendable {
    // Overview and spend
    func overview(range: TimeRange) async throws -> Overview
    func spend(range: TimeRange) async throws -> SpendSummary

    // Optimizations
    func optimizations() async throws -> [Optimization]
    func optimization(id: String) async throws -> OptimizationDetail
    func candidates(optimizationID: String) async throws -> [Candidate]
    func ratchetHistory(optimizationID: String) async throws -> [RatchetMove]
    func startOptimization(_ request: StartRequest) async throws -> Optimization
    /// The kill switch: stops the agents, revokes their keys and ends the run. Wins found so far are kept.
    func stopOptimization(id: String) async throws
    /// Opens the pull request with the squashed verified wins and returns where it is. Refused without a verified win.
    func openPullRequest(optimizationID: String) async throws -> URL
    /// The markdown report, for the save dialog.
    func report(optimizationID: String) async throws -> OptimizationReport

    // Agents
    func agents(optimizationID: String) async throws -> [Agent]
    func pauseAgent(id: String) async throws -> Agent
    func resumeAgent(id: String) async throws -> Agent
    func messageAgent(id: String, text: String) async throws

    // Benchmarks and templates
    func benchmarks() async throws -> [BenchmarkRun]
    func templates() async throws -> [Template]

    // Insights: the live log, the Result details, the Benchmarks page
    /// An agent's log. Without `cursor`: the newest `limit` lines (oldest first). With it: only lines newer than the cursor.
    func agentLog(agentID: String, after cursor: Int64?, limit: Int) async throws -> AgentLogPage
    /// Spend breakdown, "where the time went", before/after rows and the winning change of one optimization.
    func insights(optimizationID: String) async throws -> RunInsights
    /// The ratchet of one metric (`metric` is one of the series' options; nil is the objective).
    func benchmarkSeries(optimizationID: String, metric: String?) async throws -> BenchmarkSeries
    /// Every candidate with its verdict and bench results.
    func candidateResults(optimizationID: String) async throws -> [CandidateResult]
}

/// Connectors (GitHub accounts, AWS profiles, agent logins and keys), the GitHub device flow, per-repo selections, and the
/// harnesses detected on this Mac.
public protocol ConnectorsClient: Sendable {
    func connectors() async throws -> [Connector]
    func addConnector(_ new: NewConnector) async throws -> Connector
    func removeConnector(id: String) async throws
    func setDefaultConnector(id: String) async throws -> [Connector]
    /// Sign in again (an expired SSO session, a signed-out agent). Returns the refreshed connector.
    func signInConnector(id: String) async throws -> Connector
    /// The GitHub device flow in progress (its code to show), or nil. The daemon keeps one at a time.
    func githubSignInFlow() async throws -> GithubSignInFlow?
    /// Starts the device flow for a new account, or for `connectorID` again. The code is in the returned flow.
    func startGithubSignIn(connectorID: String?) async throws -> GithubSignInFlow?
    func cancelGithubSignIn() async throws
    func connectionSelection(repoID: String) async throws -> ConnectionSelection?
    func saveConnectionSelection(_ selection: ConnectionSelection) async throws

    // Pasted tokens, editing in place, checking, detecting
    func connectorSetup() async throws -> ConnectorSetup
    func addGithubToken(_ token: String, name: String?) async throws -> [Connector]
    func updateConnector(id: String, _ update: ConnectorUpdate) async throws -> [Connector]
    func checkConnector(id: String) async throws -> ConnectorCheck
    /// Checks every connector with its provider, then lists them.
    func refreshConnectors() async throws -> [Connector]
    func awsProfiles() async throws -> AwsProfiles
    /// The harnesses (agent CLIs) on this Mac; `redetect` probes the machine again first.
    func detectedHarnesses(redetect: Bool) async throws -> [DetectedHarness]
}

/// Setting up an optimization: repos, what step 1 learns about a repo, "What's missing", template pull requests,
/// environment drafts, and the development overrides.
public protocol SetupClient: Sendable {
    func repos() async throws -> [Repo]
    func detectBenchmark(repoID: String) async throws -> BenchmarkDetection
    /// The signed-in account's repos whose name contains `query`. Empty (not an error) when not signed in to GitHub.
    func searchRepos(query: String) async throws -> [Repo]
    /// Reads `ratchet.yaml` from a GitHub repo or a local folder.
    func inspectRepo(_ source: RepoSource) async throws -> RepoInspection
    func preflight(_ request: PreflightRequest) async throws -> [PreflightItem]
    /// Commits the template's files on a branch of the repo and opens the pull request.
    func proposeTemplate(_ request: TemplateProposalRequest) async throws -> TemplateProposal

    // Environment drafts
    func createDraft(_ request: DraftRequest) async throws -> EnvironmentDraft
    func reviseDraft(id: String, instruction: String) async throws -> EnvironmentDraft
    func approveDraft(id: String) async throws -> EnvironmentDraft
    /// Forgets a draft that was not approved.
    func discardDraft(id: String) async throws

    // Settings: development overrides
    func advancedSettings() async throws -> AdvancedSettings
    func saveAdvancedSettings(_ settings: AdvancedSettings) async throws -> AdvancedSettings
}

/// The cloud sandbox and its teardown receipt.
public protocol SandboxClient: Sendable {
    func sandboxStatus() async throws -> SandboxStatus
    func endSandbox() async throws -> TeardownReceipt
    func sandboxReceipt() async throws -> TeardownReceipt?
}

/// Live updates (server-sent events on the real daemon).
public protocol EventsClient: Sendable {
    func events() async -> AsyncStream<RatchetEvent>
}

/// Everything the app asks of `ratchetd` apart from chats: the five domain clients together. `AppStore` and the views
/// never learn which implementation they talk to.
public typealias RatchetClient = OptimizationsClient & ConnectorsClient & SetupClient & SandboxClient & EventsClient

/// Whether the live connection to the daemon is up. The store keeps it (`AppStore.streamState`) so a stalled stream is never
/// invisible.
public enum StreamState: Sendable, Equatable {
    /// The first connection is being made.
    case connecting
    case live
    /// The connection dropped; it is tried again after a pause. `attempt` counts failures since the last good connection.
    case reconnecting(attempt: Int, reason: String?)
}

/// One server-sent event. The app applies it to what it already shows; nothing polls.
public enum RatchetEvent: Sendable, Equatable {
    case optimizationUpdated(Optimization)
    case candidateUpdated(optimizationID: String, candidate: Candidate)
    case moveAdded(optimizationID: String, move: RatchetMove)
    case agentUpdated(optimizationID: String, agent: Agent)
    /// An agent wrote to its log: the lines after the last one shown are worth fetching.
    case agentLogAppended(optimizationID: String, agentID: String)
    case needsYouChanged([NeedsYouItem])
    case connectorsChanged([Connector])
    case githubFlowChanged(GithubSignInFlow?)
    /// An environment agent finished drafting (or revising): the draft waits for approval.
    case environmentDraftReady(draftID: String, repoID: String)
    case sandboxChanged(SandboxStatus)
    case receiptIssued(TeardownReceipt)
    /// The daemon could not replay what was missed (it restarted, or the gap is too old): reload everything shown.
    case resync
    /// The live connection came up, dropped or is being retried.
    case streamStateChanged(StreamState)
}

public enum RatchetClientError: Error, Sendable, Equatable, LocalizedError {
    case notFound(String)
    case invalid(String)
    case unavailable(String)

    public var errorDescription: String? {
        switch self {
        case .notFound(let what): "Not found: \(what)"
        case .invalid(let why): why
        case .unavailable(let why): why
        }
    }
}
