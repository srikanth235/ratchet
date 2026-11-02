import Foundation

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

/// How a connector got here.
public enum ConnectorOrigin: String, Codable, Sendable {
    /// GitHub: signed in with the device flow.
    case device
    /// GitHub: a pasted access token.
    case token
    /// GitHub: `GITHUB_TOKEN` from the environment.
    case environment
    /// An agent CLI's own login on this Mac.
    case login
    /// An agent API key kept in the keychain.
    case apiKey
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
    /// How it was added; nil when the source does not say. An account taken from the environment (`GITHUB_TOKEN`) cannot
    /// be renamed or removed here.
    public var origin: ConnectorOrigin?
    /// AWS: the profile name in `~/.aws` and the region it runs in.
    public var profile: String?
    public var region: String?
    /// Agents: which CLI it drives.
    public var harness: Harness?

    public init(
        id: String, kind: ConnectorKind, name: String, detail: String, state: ConnectorState,
        isDefault: Bool = false, monthlyCapUSD: Double? = nil, usedThisMonthUSD: Double = 0, canRunInCloud: Bool = false,
        origin: ConnectorOrigin? = nil, profile: String? = nil, region: String? = nil, harness: Harness? = nil
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
        self.origin = origin
        self.profile = profile
        self.region = region
        self.harness = harness
    }

    /// Taken from the environment (`GITHUB_TOKEN`): the app can use it but neither rename nor remove it.
    public var isFromEnvironment: Bool { origin == .environment }

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
    /// AWS: the region and the optional monthly cap typed in the Add sheet.
    public var region: String?
    public var monthlyCapUSD: Double?

    public init(
        kind: ConnectorKind, name: String, detail: String = "", secret: String? = nil, region: String? = nil,
        monthlyCapUSD: Double? = nil
    ) {
        self.kind = kind
        self.name = name
        self.detail = detail
        self.secret = secret
        self.region = region
        self.monthlyCapUSD = monthlyCapUSD
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
