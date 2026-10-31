import Foundation

/// Everything the app asks of `ratchetd`, shaped as REST resources plus one server-sent event stream.
/// `MockRatchetClient` implements it with sample data and `DaemonRatchetClient` talks to the real daemon through the
/// client generated from its OpenAPI; `AppStore` and the views never learn which one they talk to.
public protocol RatchetClient: Sendable {
    // Overview and spend
    func overview(range: TimeRange) async throws -> Overview
    func spend(range: TimeRange) async throws -> SpendSummary

    // Optimizations
    func optimizations() async throws -> [Optimization]
    func optimization(id: String) async throws -> OptimizationDetail
    func candidates(optimizationID: String) async throws -> [Candidate]
    func ratchetHistory(optimizationID: String) async throws -> [RatchetMove]
    func startOptimization(_ request: StartRequest) async throws -> Optimization

    // Agents
    func agents(optimizationID: String) async throws -> [Agent]
    func pauseAgent(id: String) async throws -> Agent
    func resumeAgent(id: String) async throws -> Agent
    func messageAgent(id: String, text: String) async throws

    // Benchmarks and templates
    func benchmarks() async throws -> [BenchmarkRun]
    func templates() async throws -> [Template]

    // Repos and harness detection
    func repos() async throws -> [Repo]
    func detectHarness(repoID: String) async throws -> HarnessDetection

    // Connectors
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

    // Environment drafts
    func createDraft(_ request: DraftRequest) async throws -> EnvironmentDraft
    func reviseDraft(id: String, instruction: String) async throws -> EnvironmentDraft
    func approveDraft(id: String) async throws -> EnvironmentDraft

    // Sandbox
    func sandboxStatus() async throws -> SandboxStatus
    func endSandbox() async throws -> TeardownReceipt
    func sandboxReceipt() async throws -> TeardownReceipt?

    // Live updates (server-sent events on the real daemon)
    func events() async -> AsyncStream<RatchetEvent>
}

/// One server-sent event. The app applies it to what it already shows; nothing polls.
public enum RatchetEvent: Sendable, Equatable {
    case optimizationUpdated(Optimization)
    case candidateUpdated(optimizationID: String, candidate: Candidate)
    case moveAdded(optimizationID: String, move: RatchetMove)
    case agentUpdated(optimizationID: String, agent: Agent)
    case needsYouChanged([NeedsYouItem])
    case connectorsChanged([Connector])
    case githubFlowChanged(GithubSignInFlow?)
    /// An environment agent finished drafting (or revising): the draft waits for approval.
    case environmentDraftReady(draftID: String, repoID: String)
    case sandboxChanged(SandboxStatus)
    case receiptIssued(TeardownReceipt)
    /// The daemon could not replay what was missed (it restarted, or the gap is too old): reload everything shown.
    case resync
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
