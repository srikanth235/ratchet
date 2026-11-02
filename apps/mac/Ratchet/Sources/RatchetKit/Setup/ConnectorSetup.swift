import Foundation

/// What the Connectors page needs to know besides the list.
public struct ConnectorSetup: Hashable, Sendable {
    /// False when the keychain is unavailable: tokens and keys cannot be stored (the environment is still honoured).
    public var encryptionAvailable: Bool
    /// True when a real OAuth client id is configured for "Sign in with GitHub".
    public var clientIdConfigured: Bool

    public init(encryptionAvailable: Bool = true, clientIdConfigured: Bool = true) {
        self.encryptionAvailable = encryptionAvailable
        self.clientIdConfigured = clientIdConfigured
    }
}

/// What changes the monthly cap of an AWS connector.
public enum CapChange: Hashable, Sendable {
    case keep
    case remove
    case set(Double)
}

/// "Rename", and for AWS "Edit region and cap".
public struct ConnectorUpdate: Hashable, Sendable {
    public var name: String?
    public var region: String?
    public var cap: CapChange

    public init(name: String? = nil, region: String? = nil, cap: CapChange = .keep) {
        self.name = name
        self.region = region
        self.cap = cap
    }
}

/// The answer of "Check connection": one line, never a key.
public struct ConnectorCheck: Hashable, Sendable {
    public var ok: Bool
    public var detail: String

    public init(ok: Bool, detail: String) {
        self.ok = ok
        self.detail = detail
    }
}

public enum AwsProfileKind: String, Sendable {
    case sso, role, keys, process, other

    public var title: String {
        switch self {
        case .sso: "SSO"
        case .role: "assume role"
        case .keys: "access keys"
        case .process: "credential process"
        case .other: "profile"
        }
    }
}

/// A profile found in `~/.aws/config` or `~/.aws/credentials` (names only, never a key).
public struct AwsProfile: Identifiable, Hashable, Sendable {
    public var name: String
    public var kind: AwsProfileKind
    public var ssoSession: String?

    public var id: String { name }

    public init(name: String, kind: AwsProfileKind = .other, ssoSession: String? = nil) {
        self.name = name
        self.kind = kind
        self.ssoSession = ssoSession
    }

    /// "SSO · session acme".
    public var hint: String {
        kind.title + (ssoSession.map { " · session " + $0 } ?? "")
    }
}

public struct AwsProfiles: Hashable, Sendable {
    public var profiles: [AwsProfile]
    public var ssoSessions: [String]
    /// False when neither `~/.aws/config` nor `~/.aws/credentials` exists.
    public var filesFound: Bool

    public init(profiles: [AwsProfile] = [], ssoSessions: [String] = [], filesFound: Bool = false) {
        self.profiles = profiles
        self.ssoSessions = ssoSessions
        self.filesFound = filesFound
    }

    /// The profiles not yet connected.
    public func available(excluding connected: [Connector]) -> [AwsProfile] {
        let have = Set(connected.filter { $0.kind == .aws }.map { $0.profile ?? $0.name })
        return profiles.filter { !have.contains($0.name) }
    }

    /// What to say when nothing can be picked.
    public func emptyNote(excluding connected: [Connector]) -> String? {
        guard available(excluding: connected).isEmpty else { return nil }
        if !profiles.isEmpty { return "Every profile in ~/.aws is already connected." }
        if filesFound { return "No profiles found in ~/.aws/config or ~/.aws/credentials." }
        return "No ~/.aws/config or ~/.aws/credentials on this Mac."
    }
}

/// One agent CLI the daemon found (or did not) on this Mac.
public struct DetectedHarness: Identifiable, Hashable, Sendable {
    public enum Auth: String, Sendable { case signedIn, apiKey, signedOut }

    public var harness: Harness
    public var installed: Bool
    public var version: String?
    public var auth: Auth
    /// The models its CLI offers and the one used when none is chosen; empty when the daemon did not say.
    public var models: [HarnessModel]
    public var defaultModel: String?
    /// How its tokens are paid for: the user's plan (a login) or an API key. nil while signed out.
    public var billing: Billing?
    /// What to do next, or what was found ("Run `codex login`"); never a credential.
    public var hint: String?
    /// Where the CLI is on this Mac, when the daemon says.
    public var path: String?

    public enum Billing: String, Sendable { case subscription, apiKey }

    public var id: String { harness.rawValue }

    public init(
        harness: Harness, installed: Bool, version: String? = nil, auth: Auth = .signedOut,
        models: [HarnessModel] = [], defaultModel: String? = nil, billing: Billing? = nil, hint: String? = nil, path: String? = nil
    ) {
        self.billing = billing
        self.hint = hint
        self.path = path
        self.harness = harness
        self.installed = installed
        self.version = version
        self.auth = auth
        self.models = models
        self.defaultModel = defaultModel
    }

    /// "Claude Code · v2.1.4".
    public var title: String { harness.title + (version.map { " · v" + $0 } ?? "") }

    /// The detected CLIs that have no login connector yet.
    public static func addable(_ found: [DetectedHarness], connected: [Connector]) -> [DetectedHarness] {
        let present = Set(connected.filter { $0.kind == .agent && $0.origin == .login }.compactMap(\.harness))
        return found.filter { $0.installed && !present.contains($0.harness) }
    }

    /// What to say when there is nothing to add.
    public static func emptyNote(_ found: [DetectedHarness]) -> String {
        found.contains { $0.installed }
            ? "Your Claude Code, Codex and Grok logins are already listed."
            : "None of Claude Code, Codex or Grok was found."
    }
}
