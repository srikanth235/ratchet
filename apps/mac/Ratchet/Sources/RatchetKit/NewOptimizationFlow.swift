import Foundation
import Observation

public enum WizardStep: Int, CaseIterable, Sendable {
    case goal, connections, environment, budget

    public var title: String {
        switch self {
        case .goal: "Goal & repo"
        case .connections: "Connections"
        case .environment: "Environment"
        case .budget: "Budget & time"
        }
    }

    /// "Step 2 of 4 · Connections".
    public var caption: String { "Step \(rawValue + 1) of \(Self.allCases.count) · \(title)" }
}

/// The four steps of "New optimization": Goal & repo, Connections, Environment, Budget & time. All rules live here so
/// the views stay dumb and the tests can walk the flow.
@MainActor @Observable
public final class NewOptimizationFlow {
    public static let defaultBudgetUSD = 50.0
    public static let defaultTimeLimitHours = 2.0

    public private(set) var step: WizardStep = .goal
    public var goal = ""
    public private(set) var repoID: String?
    public var branch = "main"
    public private(set) var templateID: String?
    public private(set) var harness: HarnessDetection?
    public private(set) var repos: [Repo] = []
    public private(set) var connectors: [Connector] = []
    public private(set) var templates: [Template] = []

    public private(set) var awsID: String?
    /// Selected agent connector ids and where each one runs.
    public private(set) var agents: [String: Placement] = [:]

    public private(set) var draft: EnvironmentDraft?
    public var revisionText = ""

    public var budgetUSD = NewOptimizationFlow.defaultBudgetUSD
    public var timeLimitHours = NewOptimizationFlow.defaultTimeLimitHours

    // Step 1: finding the repo and reading what it declares.
    /// What the search box holds. `search()` fills `searchResults` from it.
    public var repoQuery = ""
    /// The signed-in account's repos matching `repoQuery`; nil until a search ran (then `repos` is shown).
    public private(set) var searchResults: [Repo]?
    public private(set) var isSearching = false
    /// What was read from the chosen repo or folder.
    public private(set) var inspection: RepoInspection?
    public private(set) var isInspecting = false
    /// Why the chosen repo could not be read (not signed in, no access...).
    public private(set) var inspectionError: String?
    /// The folder chosen with "Local folder…", when that is the source.
    public private(set) var localFolder: String?
    /// The pull request that adds a template to the chosen repo, and its progress.
    public private(set) var proposal: TemplateProposal?
    public private(set) var proposalError: String?
    public private(set) var proposingTemplateID: String?
    /// "View ratchet.yaml" is open.
    public var showsYAML = false
    /// "What's missing" on the Connections step.
    public private(set) var preflight: [PreflightItem] = []

    public private(set) var isBusy = false
    public private(set) var error: String?
    public private(set) var started: Optimization?

    private let client: any RatchetClient
    private var selectionLoadedFor: String?

    public init(client: any RatchetClient) {
        self.client = client
    }

    // MARK: Step 1: goal and repo

    public func load() async {
        do {
            repos = try await client.repos()
            connectors = try await client.connectors()
            templates = try await client.templates()
        } catch {
            self.error = error.localizedDescription
        }
        if repoID == nil, let first = repos.first { await selectRepo(first.id) }
    }

    /// The repos the picker shows: the search results, else the account's list (the first 60).
    public var shownRepos: [Repo] {
        Array((searchResults ?? repos).prefix(60))
    }

    /// True when there is a GitHub account to search with.
    public var isSignedInToGitHub: Bool {
        connectors.contains { $0.kind == .github && $0.state.isUsable }
    }

    /// Searches the signed-in account's repos for `repoQuery`; an empty query shows the whole list again.
    public func search() async {
        let text = repoQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            searchResults = nil
            return
        }
        isSearching = true
        defer { isSearching = false }
        do {
            let found = try await client.searchRepos(query: text)
            // A newer query replaced this one while it was out.
            guard text == repoQuery.trimmingCharacters(in: .whitespacesAndNewlines) else { return }
            searchResults = found
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    public func selectRepo(_ id: String) async {
        guard let repo = (repos + (searchResults ?? [])).first(where: { $0.id == id }) else { return }
        if !repos.contains(where: { $0.id == id }) { repos.append(repo) }
        localFolder = nil
        await choose(repo)
    }

    private func choose(_ repo: Repo) async {
        if repoID != repo.id {
            draft = nil
            selectionLoadedFor = nil
        }
        repoID = repo.id
        branch = repo.defaultBranch
        proposal = nil
        proposalError = nil
        await inspect(.github(repo.id))
    }

    /// "Local folder…": reads `ratchet.yaml` from a folder on this Mac. A usable folder becomes the chosen repo (by its
    /// GitHub `origin`); one without a remote says why it cannot be used.
    public func chooseLocalFolder(_ path: String) async {
        localFolder = path
        proposal = nil
        proposalError = nil
        await inspect(.local(path))
        guard let inspection, let name = inspection.repo else { return }
        if case .unusable = inspection.outcome { return }
        if !repos.contains(where: { $0.id == name }) {
            repos.append(Repo(id: name, defaultBranch: inspection.baseRef, account: name.split(separator: "/").first.map(String.init) ?? name))
        }
        if repoID != name {
            draft = nil
            selectionLoadedFor = nil
        }
        repoID = name
        branch = inspection.baseRef
    }

    /// Reads the repo again (after the pull request that adds `ratchet.yaml` was merged).
    public func recheck() async {
        if let localFolder {
            await chooseLocalFolder(localFolder)
        } else if let repoID {
            proposal = nil
            await inspect(.github(repoID))
        }
    }

    private func inspect(_ source: RepoSource) async {
        isInspecting = true
        inspectionError = nil
        defer { isInspecting = false }
        do {
            let read = try await client.inspectRepo(source)
            inspection = read
            harness = read.harness
        } catch {
            inspection = nil
            inspectionError = error.localizedDescription
            harness = HarnessDetection(repoID: repoID ?? "", found: false, summary: error.localizedDescription)
        }
    }

    /// "Add via pull request": commits the template's files on a branch of the repo and opens the pull request.
    public func proposeTemplate(_ templateID: String) async {
        guard let inspection, let repo = inspection.repo else { return }
        proposingTemplateID = templateID
        proposalError = nil
        proposal = nil
        defer { proposingTemplateID = nil }
        do {
            proposal = try await client.proposeTemplate(
                TemplateProposalRequest(repo: repo, baseRef: inspection.baseRef, templateID: templateID))
        } catch {
            proposalError = error.localizedDescription
        }
    }

    /// Step 1 may continue: a repo that cannot be read, or whose `ratchet.yaml` does not parse, blocks it. A repo with
    /// no `ratchet.yaml` does not (the template path starts one).
    public var repoIsUsable: Bool {
        guard let inspection else { return repoID != nil }
        switch inspection.outcome {
        case .unusable, .invalid: return false
        case .ready, .missing: return repoID != nil
        }
    }

    /// Starts from a template: its title becomes the goal when none was typed.
    public func useTemplate(_ id: String) {
        templateID = id
        if goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, let template = templates.first(where: { $0.id == id }) {
            goal = "Improve " + template.title
        }
    }

    public var title: String {
        let text = goal.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? "New optimization" : text
    }

    public var subtitle: String {
        guard let repoID else { return "Choose a repo" }
        return repoID + " · " + branch
    }

    // MARK: Step 2: connections

    public func toggleAgent(_ id: String) {
        guard let connector = connectors.first(where: { $0.id == id && $0.state.isUsable }) else { return }
        if agents[id] != nil {
            agents[id] = nil
        } else {
            agents[id] = connector.canRunInCloud ? .cloud : .local
        }
    }

    /// "change" under an agent: flips between this Mac and the cloud, when the connector can run in the cloud.
    public func togglePlacement(_ id: String) {
        guard let placement = agents[id], connectors.first(where: { $0.id == id })?.canRunInCloud == true else { return }
        agents[id] = placement == .local ? .cloud : .local
    }

    public func selectAWS(_ id: String) {
        guard connectors.contains(where: { $0.id == id && $0.kind == .aws && $0.state.isUsable }) else { return }
        awsID = id
    }

    public var selection: ConnectionSelection {
        ConnectionSelection(repoID: repoID ?? "", awsID: awsID, agents: agents)
    }

    private func prepareConnections() async {
        guard let repoID, selectionLoadedFor != repoID else { return }
        selectionLoadedFor = repoID
        if connectors.isEmpty { connectors = (try? await client.connectors()) ?? [] }
        awsID = nil
        agents = [:]
        if let saved = try? await client.connectionSelection(repoID: repoID) {
            if let aws = saved.awsID { selectAWS(aws) }
            for (id, placement) in saved.agents where connectors.contains(where: { $0.id == id && $0.state.isUsable }) {
                agents[id] = placement
            }
        }
        if awsID == nil {
            let usableAWS = connectors.filter { $0.kind == .aws && $0.state.isUsable }
            awsID = (usableAWS.first(where: { $0.isDefault }) ?? usableAWS.first)?.id
        }
        if agents.isEmpty, let agent = connectors.first(where: { $0.kind == .agent && $0.isDefault && $0.state.isUsable }) {
            agents[agent.id] = agent.canRunInCloud ? .cloud : .local
        }
    }

    /// Identifies what "What's missing" depends on, so the view can refresh it when it changes.
    public var preflightKey: String {
        agents.sorted { $0.key < $1.key }.map { $0.key + ":" + $0.value.rawValue }.joined(separator: ",")
            + "|" + (repoID ?? "") + "|" + (localFolder ?? "")
    }

    /// "What's missing" for this run: the short list under Connections.
    public func refreshPreflight() async {
        let request = PreflightRequest(
            source: preflightSource, repo: repoID, agentIDs: agents.keys.sorted(),
            placement: agents.values.contains(.cloud) && !agents.values.contains(.local) ? .cloud : .local)
        let key = preflightKey
        do {
            let items = try await client.preflight(request)
            guard key == preflightKey else { return }
            preflight = items
        } catch {
            // The list is a help, not a gate we can verify: an unreachable check does not stop the run.
            preflight = []
        }
    }

    private var preflightSource: PreflightSource {
        if templateID?.hasPrefix("demo-") == true { return .demo }
        return localFolder == nil ? .github : .local
    }

    /// What blocks the run, in the daemon's words.
    public var blockers: [PreflightItem] { preflight.filter { $0.state == .fail } }

    // MARK: Step 3: environment

    private func prepareDraft() async {
        guard let repoID, draft == nil else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            draft = try await client.createDraft(DraftRequest(repoID: repoID, goal: title, templateID: templateID))
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// "Ask for a change": the agent revises the draft, which then needs approving again.
    public func sendRevision(_ text: String? = nil) async {
        guard let draft else { return }
        let instruction = (text ?? revisionText).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !instruction.isEmpty else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            self.draft = try await client.reviseDraft(id: draft.id, instruction: instruction)
            revisionText = ""
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// "Discard draft": forgets a draft that was not approved and goes back to Connections. An approved one stays.
    public func discardDraft() async {
        guard let draft, draft.state == .draft else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            try await client.discardDraft(id: draft.id)
            self.draft = nil
            error = nil
            step = .connections
        } catch {
            self.error = error.localizedDescription
        }
    }

    public func approveDraft() async {
        guard let draft else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            self.draft = try await client.approveDraft(id: draft.id)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    // MARK: Step 4 and navigation

    /// What the sandbox costs for the whole time limit.
    public var estimatedSandboxUSD: Double {
        (draft?.usdPerHour ?? 0) * timeLimitHours
    }

    public var canContinue: Bool {
        switch step {
        case .goal: !goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && repoID != nil && repoIsUsable
        case .connections: awsID != nil && !agents.isEmpty && !preflight.contains { $0.state == .fail }
        case .environment: draft?.state == .approved
        case .budget: budgetUSD > 0 && timeLimitHours > 0
        }
    }

    public var isLastStep: Bool { step == WizardStep.allCases.last }

    public func next() async {
        guard canContinue, let target = WizardStep(rawValue: step.rawValue + 1) else { return }
        await go(to: target)
    }

    public func back() async {
        guard let target = WizardStep(rawValue: step.rawValue - 1) else { return }

        await go(to: target)
    }

    /// Moves to a step and loads what it shows. Safe to call again.
    public func go(to target: WizardStep) async {
        if repos.isEmpty { await load() }
        if target.rawValue >= WizardStep.connections.rawValue {
            await prepareConnections()
            await refreshPreflight()
        }
        if target.rawValue >= WizardStep.environment.rawValue {
            await saveSelection()
            await prepareDraft()
        }
        step = target
    }

    private func saveSelection() async {
        guard repoID != nil, awsID != nil, !agents.isEmpty else { return }
        try? await client.saveConnectionSelection(selection)
    }

    /// Starts the optimization with the chosen limits.
    public func start() async -> Optimization? {
        guard let repoID, canContinue, step == .budget else { return nil }
        isBusy = true
        defer { isBusy = false }
        do {
            let request = StartRequest(
                repoID: repoID, branch: branch, goal: title, templateID: templateID, draftID: draft?.id,
                selection: selection, budgetUSD: budgetUSD, timeLimitHours: timeLimitHours)
            let optimization = try await client.startOptimization(request)
            started = optimization
            error = nil
            return optimization
        } catch {
            self.error = error.localizedDescription
            return nil
        }
    }
}
