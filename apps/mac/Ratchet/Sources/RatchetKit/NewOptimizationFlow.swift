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

    public func selectRepo(_ id: String) async {
        guard let repo = repos.first(where: { $0.id == id }) else { return }
        if repoID != id {
            draft = nil
            selectionLoadedFor = nil
        }
        repoID = id
        branch = repo.defaultBranch
        harness = try? await client.detectHarness(repoID: id)
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
        case .goal: !goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && repoID != nil
        case .connections: awsID != nil && !agents.isEmpty
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
        if target.rawValue >= WizardStep.connections.rawValue { await prepareConnections() }
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
