import Foundation

// Connectors, environment drafts and the sandbox.

public enum ConnectorKind: String, CaseIterable, Codable, Sendable, Identifiable {
    case github, aws, agent

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .github: "GitHub"
        case .aws: "AWS"
        case .agent: "Agents"
        }
    }
}

public enum ConnectorState: String, Codable, Sendable {
    case connected, signedIn, valid, expired, signedOut

    public var title: String {
        switch self {
        case .connected: "Connected"
        case .signedIn: "Signed in"
        case .valid: "Valid"
        case .expired: "Expired"
        case .signedOut: "Signed out"
        }
    }

    /// Usable in a new optimization.
    public var isUsable: Bool { self == .connected || self == .signedIn || self == .valid }
}

public struct Connector: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var kind: ConnectorKind
    public var name: String
    /// The grey line under the name: "Personal · 42 repos".
    public var detail: String
    public var state: ConnectorState
    public var isDefault: Bool
    /// AWS: the monthly cap and what is used; nil for other kinds.
    public var monthlyCapUSD: Double?
    public var usedThisMonthUSD: Double
    /// Agents: whether a cloud sandbox can run it (a team key can, a personal login cannot).
    public var canRunInCloud: Bool

    public init(
        id: String, kind: ConnectorKind, name: String, detail: String, state: ConnectorState,
        isDefault: Bool = false, monthlyCapUSD: Double? = nil, usedThisMonthUSD: Double = 0, canRunInCloud: Bool = false
    ) {
        self.id = id
        self.kind = kind
        self.name = name
        self.detail = detail
        self.state = state
        self.isDefault = isDefault
        self.monthlyCapUSD = monthlyCapUSD
        self.usedThisMonthUSD = usedThisMonthUSD
        self.canRunInCloud = canRunInCloud
    }

    public var leftThisMonthUSD: Double? {
        monthlyCapUSD.map { max($0 - usedThisMonthUSD, 0) }
    }
}

public struct NewConnector: Codable, Hashable, Sendable {
    public var kind: ConnectorKind
    public var name: String
    public var detail: String
    /// An API key typed once in the Add sheet (agents). It goes to `ratchetd`, which files it in the Keychain; the app
    /// keeps it nowhere and never saves it.
    public var secret: String?

    public init(kind: ConnectorKind, name: String, detail: String = "", secret: String? = nil) {
        self.kind = kind
        self.name = name
        self.detail = detail
        self.secret = secret
    }

    private enum CodingKeys: String, CodingKey { case kind, name, detail }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decode(ConnectorKind.self, forKey: .kind)
        name = try c.decode(String.self, forKey: .name)
        detail = try c.decode(String.self, forKey: .detail)
        secret = nil
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(kind, forKey: .kind)
        try c.encode(name, forKey: .name)
        try c.encode(detail, forKey: .detail)
    }
}

/// The GitHub device flow in progress: the user types `userCode` at `verificationURL`, and the account appears in the
/// connectors list when GitHub confirms (a `connectorsChanged` event).
public struct GithubSignInFlow: Codable, Hashable, Sendable {
    public enum State: String, Codable, Sendable { case pending, expired, denied, error }

    public var state: State
    public var userCode: String
    public var verificationURL: String
    public var expiresAt: Date
    public var message: String?

    public init(state: State, userCode: String, verificationURL: String, expiresAt: Date, message: String? = nil) {
        self.state = state
        self.userCode = userCode
        self.verificationURL = verificationURL
        self.expiresAt = expiresAt
        self.message = message
    }
}

/// Which AWS account and which agents a repo uses, remembered per repo.
public struct ConnectionSelection: Codable, Hashable, Sendable {
    public var repoID: String
    public var awsID: String?
    /// Agent connector id to where it runs.
    public var agents: [String: Placement]
    /// The GitHub account of the repo; nil: the default one.
    public var githubID: String?

    public init(repoID: String, awsID: String? = nil, agents: [String: Placement] = [:], githubID: String? = nil) {
        self.repoID = repoID
        self.awsID = awsID
        self.agents = agents
        self.githubID = githubID
    }
}

// MARK: - Environment drafts

public enum EnvironmentScope: String, Codable, Sendable {
    case none, single, full
}

public enum DraftState: String, Codable, Sendable {
    case draft, approved
}

public struct DraftResource: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var name: String
    public var detail: String
    public var machine: String
    public var usdPerHour: Double

    public init(id: String, name: String, detail: String, machine: String, usdPerHour: Double) {
        self.id = id
        self.name = name
        self.detail = detail
        self.machine = machine
        self.usdPerHour = usdPerHour
    }
}

public struct DraftRequest: Codable, Hashable, Sendable {
    public var repoID: String
    public var goal: String
    public var templateID: String?

    public init(repoID: String, goal: String, templateID: String? = nil) {
        self.repoID = repoID
        self.goal = goal
        self.templateID = templateID
    }
}

public struct EnvironmentDraft: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var repoID: String
    public var scope: EnvironmentScope
    public var state: DraftState
    public var revision: Int
    /// The sentence at the top, and the part of it to highlight ("full stack").
    public var headline: String
    public var emphasis: String
    public var reason: String
    public var draftedBy: String
    public var draftSeconds: Double
    public var draftCostUSD: Double
    public var readyMinutes: Double
    public var resources: [DraftResource]
    public var files: [String]
    public var checks: String

    public init(
        id: String, repoID: String, scope: EnvironmentScope, state: DraftState = .draft, revision: Int = 1,
        headline: String, emphasis: String, reason: String, draftedBy: String, draftSeconds: Double,
        draftCostUSD: Double, readyMinutes: Double, resources: [DraftResource], files: [String], checks: String
    ) {
        self.id = id
        self.repoID = repoID
        self.scope = scope
        self.state = state
        self.revision = revision
        self.headline = headline
        self.emphasis = emphasis
        self.reason = reason
        self.draftedBy = draftedBy
        self.draftSeconds = draftSeconds
        self.draftCostUSD = draftCostUSD
        self.readyMinutes = readyMinutes
        self.resources = resources
        self.files = files
        self.checks = checks
    }

    public var usdPerHour: Double { resources.reduce(0) { $0 + $1.usdPerHour } }

    public func cost(forHours hours: Double) -> Double { usdPerHour * hours }
}

// MARK: - Sandbox

public struct TeardownReceipt: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var account: String
    public var startedAt: Date
    public var endedAt: Date
    public var costUSD: Double
    public var destroyed: [String]
    public var allGone: Bool

    public init(id: String, account: String, startedAt: Date, endedAt: Date, costUSD: Double, destroyed: [String], allGone: Bool = true) {
        self.id = id
        self.account = account
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.costUSD = costUSD
        self.destroyed = destroyed
        self.allGone = allGone
    }

    public var duration: TimeInterval { endedAt.timeIntervalSince(startedAt) }
}

public struct SandboxStatus: Codable, Hashable, Sendable {
    public var isOn: Bool
    public var account: String?
    public var costSoFarUSD: Double
    public var offAt: Date?

    public init(isOn: Bool, account: String? = nil, costSoFarUSD: Double = 0, offAt: Date? = nil) {
        self.isOn = isOn
        self.account = account
        self.costSoFarUSD = costSoFarUSD
        self.offAt = offAt
    }

    public static let off = SandboxStatus(isOn: false)
}

public struct StartRequest: Codable, Hashable, Sendable {
    public var repoID: String
    public var branch: String
    public var goal: String
    public var templateID: String?
    public var draftID: String?
    public var selection: ConnectionSelection
    public var budgetUSD: Double
    public var timeLimitHours: Double

    public init(
        repoID: String, branch: String, goal: String, templateID: String? = nil, draftID: String? = nil,
        selection: ConnectionSelection, budgetUSD: Double, timeLimitHours: Double
    ) {
        self.repoID = repoID
        self.branch = branch
        self.goal = goal
        self.templateID = templateID
        self.draftID = draftID
        self.selection = selection
        self.budgetUSD = budgetUSD
        self.timeLimitHours = timeLimitHours
    }
}
