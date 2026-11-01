import Foundation

// What New optimization step 1 learns about a repo, the "What's missing" list, template pull requests, connector
// editing and the development overrides. Plain Sendable values like the rest of Models.swift.

// MARK: - Repo inspection (step 1)

/// Where an optimization starts from: a repo of the signed-in GitHub account, or a folder on this Mac.
public enum RepoSource: Hashable, Sendable {
    case github(String)
    case local(String)

    public var isLocal: Bool {
        if case .local = self { return true }
        return false
    }
}

/// Something the repo protects from the agents ("tests/**"), with why.
public struct ProtectedPath: Identifiable, Hashable, Sendable {
    public var glob: String
    public var reason: String

    public var id: String { glob }

    public init(glob: String, reason: String) {
        self.glob = glob
        self.reason = reason
    }
}

/// What `ratchet.yaml` declares, in the words the review shows.
public struct RepoConfig: Hashable, Sendable {
    /// The metric path as written ("http_req_duration.p95").
    public var metric: String
    public var lowerIsBetter: Bool
    /// The evaluator template, or the command it runs.
    public var evaluator: String
    public var evaluatorIsTemplate: Bool
    public var holdout: String?
    /// The repo's container image; nil: setup and evaluator run on the bench machine itself.
    public var image: String?
    public var machine: String
    public var region: String?
    /// Runs per candidate when the repo sets it; nil: measured from the noise during the baseline.
    public var repeats: Int?
    public var protected: [ProtectedPath]
    /// `objective.name`, the default name of the optimization.
    public var name: String?

    public init(
        metric: String, lowerIsBetter: Bool = true, evaluator: String = "", evaluatorIsTemplate: Bool = false,
        holdout: String? = nil, image: String? = nil, machine: String = "", region: String? = nil, repeats: Int? = nil,
        protected: [ProtectedPath] = [], name: String? = nil
    ) {
        self.metric = metric
        self.lowerIsBetter = lowerIsBetter
        self.evaluator = evaluator
        self.evaluatorIsTemplate = evaluatorIsTemplate
        self.holdout = holdout
        self.image = image
        self.machine = machine
        self.region = region
        self.repeats = repeats
        self.protected = protected
        self.name = name
    }

    /// "p95", "events per s": the last part of the path, in words.
    public var metricName: String {
        (metric.split(separator: ".").last.map(String.init) ?? metric).replacingOccurrences(of: "_", with: " ")
    }

    public var direction: String { lowerIsBetter ? "lower is better" : "higher is better" }
}

/// One problem found in `ratchet.yaml`.
public struct ConfigProblem: Identifiable, Hashable, Sendable {
    /// The YAML path ("guardrails[2].peak_rss"); empty for the whole document.
    public var path: String
    public var message: String

    public var id: String { path + "|" + message }

    public init(path: String, message: String) {
        self.path = path
        self.message = message
    }
}

/// What step 1 learned about the chosen repo, read from its default branch (never typed).
public struct RepoInspection: Hashable, Sendable {
    public var source: RepoSource
    /// "owner/name"; for a local folder from its `origin`. Nil: a local folder with no GitHub remote.
    public var repo: String?
    public var baseRef: String
    /// The `ratchet.yaml` text, read-only; nil when the repo has none.
    public var configYAML: String?
    public var config: RepoConfig?
    public var problems: [ConfigProblem]
    /// Offered when there is no `ratchet.yaml`: "Add via pull request".
    public var templates: [Template]
    /// Why the repo cannot be used here (no GitHub remote, not a git repo...).
    public var problem: String?
    /// The template to suggest first.
    public var suggestedTemplateID: String?

    public init(
        source: RepoSource, repo: String?, baseRef: String = "main", configYAML: String? = nil, config: RepoConfig? = nil,
        problems: [ConfigProblem] = [], templates: [Template] = [], problem: String? = nil, suggestedTemplateID: String? = nil
    ) {
        self.source = source
        self.repo = repo
        self.baseRef = baseRef
        self.configYAML = configYAML
        self.config = config
        self.problems = problems
        self.templates = templates
        self.problem = problem
        self.suggestedTemplateID = suggestedTemplateID
    }

    public enum Outcome: Hashable, Sendable {
        /// The repo cannot be used here, and why.
        case unusable(String)
        /// `ratchet.yaml` found and valid.
        case ready
        /// `ratchet.yaml` found but it does not parse; `problems` say why.
        case invalid
        /// No `ratchet.yaml`: offer the templates.
        case missing
    }

    public var outcome: Outcome {
        if let problem, !problem.isEmpty { return .unusable(problem) }
        if config != nil, repo != nil { return .ready }
        if configYAML != nil, config == nil { return .invalid }
        if repo != nil { return .missing }
        return .unusable("This folder has no GitHub remote, so Ratchet cannot open a pull request for it.")
    }

    /// Step 1 can continue.
    public var isUsable: Bool { outcome == .ready }

    /// The summary line the Goal step shows under "Benchmark".
    public var harness: HarnessDetection {
        let id = repo ?? ""
        switch outcome {
        case .ready:
            let direction = (config?.lowerIsBetter ?? true) ? "lower" : "higher"
            return HarnessDetection(
                repoID: id, found: true, summary: "Found ratchet.yaml: \(config?.metricName ?? ""), \(direction) is better.",
                suggestedTemplateID: suggestedTemplateID)
        case .unusable(let why):
            return HarnessDetection(repoID: id, found: false, summary: why, suggestedTemplateID: suggestedTemplateID)
        case .invalid:
            return HarnessDetection(repoID: id, found: false, summary: "ratchet.yaml does not parse.", suggestedTemplateID: nil)
        case .missing:
            return HarnessDetection(
                repoID: id, found: false, summary: "No benchmark found in this repo. Start from a template.",
                suggestedTemplateID: suggestedTemplateID ?? templates.first?.id)
        }
    }
}

// MARK: - "What's missing"

public enum PreflightSource: String, Sendable {
    case demo, github, local
}

public enum PreflightState: String, Sendable {
    case ok, warn, fail
}

public struct PreflightItem: Identifiable, Hashable, Sendable {
    public var id: String
    public var label: String
    public var state: PreflightState
    public var detail: String

    public init(id: String, label: String, state: PreflightState, detail: String) {
        self.id = id
        self.label = label
        self.state = state
        self.detail = detail
    }
}

public struct PreflightRequest: Hashable, Sendable {
    public var source: PreflightSource
    /// "owner/name": check the connectors remembered for it instead of the defaults.
    public var repo: String?
    /// The agent connectors chosen for the run.
    public var agentIDs: [String]
    public var placement: Placement

    public init(source: PreflightSource, repo: String? = nil, agentIDs: [String], placement: Placement) {
        self.source = source
        self.repo = repo
        self.agentIDs = agentIDs
        self.placement = placement
    }
}

// MARK: - Template pull requests

public struct TemplateProposalRequest: Hashable, Sendable {
    public var repo: String
    /// The branch the pull request targets.
    public var baseRef: String
    public var templateID: String

    public init(repo: String, baseRef: String, templateID: String) {
        self.repo = repo
        self.baseRef = baseRef
        self.templateID = templateID
    }
}

/// The pull request that adds a template to a repo.
public struct TemplateProposal: Hashable, Sendable {
    public var prURL: String
    public var branch: String
    /// Repo paths the commit adds.
    public var files: [String]
    public var templateID: String

    public init(prURL: String, branch: String, files: [String], templateID: String) {
        self.prURL = prURL
        self.branch = branch
        self.files = files
        self.templateID = templateID
    }

    public var url: URL? { URL(string: prURL) }

    /// "Adds ratchet.yaml, bench/k6.js on ratchet/add-template. Merge it, then pick the repository again."
    public var summary: String {
        let list = files.isEmpty ? "the template" : files.joined(separator: ", ")
        return "Adds \(list) on \(branch). Merge it, then choose the repository again."
    }
}

// MARK: - Connectors

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
public struct DetectedAgent: Identifiable, Hashable, Sendable {
    public enum Auth: String, Sendable { case signedIn, apiKey, signedOut }

    public var cli: AgentCLI
    public var installed: Bool
    public var version: String?
    public var auth: Auth

    public var id: String { cli.rawValue }

    public init(cli: AgentCLI, installed: Bool, version: String? = nil, auth: Auth = .signedOut) {
        self.cli = cli
        self.installed = installed
        self.version = version
        self.auth = auth
    }

    /// "Claude Code · v2.1.4".
    public var title: String { cli.title + (version.map { " · v" + $0 } ?? "") }

    /// The detected CLIs that have no login connector yet.
    public static func addable(_ found: [DetectedAgent], connected: [Connector]) -> [DetectedAgent] {
        let present = Set(connected.filter { $0.kind == .agent && $0.origin == .login }.compactMap(\.cli))
        return found.filter { $0.installed && !present.contains($0.cli) }
    }

    /// What to say when there is nothing to add.
    public static func emptyNote(_ found: [DetectedAgent]) -> String {
        found.contains { $0.installed }
            ? "Your Claude Code and Codex logins are already listed."
            : "Neither Claude Code nor Codex was found."
    }
}

// MARK: - Advanced (development)

/// The overrides for working on Ratchet itself. Empty means "use the default".
public struct AdvancedSettings: Hashable, Sendable {
    public var runnerAMI: String
    public var benchAMI: String
    public var tofuBinary: String
    public var moduleDir: String
    /// What an empty `tofuBinary` and `moduleDir` resolve to.
    public var defaultTofuBinary: String
    public var defaultModuleDir: String

    public init(
        runnerAMI: String = "", benchAMI: String = "", tofuBinary: String = "", moduleDir: String = "",
        defaultTofuBinary: String = "", defaultModuleDir: String = ""
    ) {
        self.runnerAMI = runnerAMI
        self.benchAMI = benchAMI
        self.tofuBinary = tofuBinary
        self.moduleDir = moduleDir
        self.defaultTofuBinary = defaultTofuBinary
        self.defaultModuleDir = defaultModuleDir
    }

    public func value(of field: AdvancedField) -> String {
        switch field {
        case .runnerAMI: runnerAMI
        case .benchAMI: benchAMI
        case .tofuBinary: tofuBinary
        case .moduleDir: moduleDir
        }
    }

    public mutating func set(_ value: String, for field: AdvancedField) {
        switch field {
        case .runnerAMI: runnerAMI = value
        case .benchAMI: benchAMI = value
        case .tofuBinary: tofuBinary = value
        case .moduleDir: moduleDir = value
        }
    }

    public func placeholder(for field: AdvancedField) -> String {
        switch field {
        case .runnerAMI, .benchAMI: "ami-0123456789abcdef0"
        case .tofuBinary: defaultTofuBinary
        case .moduleDir: defaultModuleDir
        }
    }
}

public enum AdvancedField: String, CaseIterable, Identifiable, Sendable {
    case runnerAMI, benchAMI, tofuBinary, moduleDir

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .runnerAMI: "Runner AMI"
        case .benchAMI: "Bench AMI"
        case .tofuBinary: "OpenTofu binary"
        case .moduleDir: "OpenTofu module directory"
        }
    }

    public var hint: String {
        switch self {
        case .runnerAMI: "Overrides the tag lookup for cloud agent runners."
        case .benchAMI: "Overrides the tag lookup for the evaluator instances."
        case .tofuBinary: "Empty = the bundled one."
        case .moduleDir: "Empty = the bundled infra/aws."
        }
    }
}

// MARK: - Checks while typing

/// The format checks of the Connectors and Advanced forms, run while typing and again by the daemon before anything is
/// stored. `nil` means valid. Empty values are valid for settings (they mean "not set").
public enum SettingsCheck {
    private static func matches(_ text: String, _ pattern: String) -> Bool {
        text.range(of: pattern, options: .regularExpression) != nil
    }

    private static func isAbsolute(_ path: String) -> Bool {
        path.hasPrefix("/") || path.hasPrefix("~/")
    }

    /// A path or a bare command name: no shell metacharacters, no control characters.
    private static func isPathLike(_ text: String) -> Bool {
        let forbidden: Set<Character> = [";", "&", "|", "`", "$", "<", ">", "*", "?", "\"", "'", "\\", "\0", "\r", "\n"]
        return !text.isEmpty && !text.contains { forbidden.contains($0) }
    }

    public static func problem(for field: AdvancedField, _ raw: String) -> String? {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        switch field {
        case .runnerAMI, .benchAMI:
            return value.isEmpty || matches(value, "^ami-([0-9a-f]{8}|[0-9a-f]{17})$") ? nil : "Use an AMI id such as ami-0123456789abcdef0"
        case .tofuBinary:
            if value.isEmpty { return nil }
            if value.count > 500 || !isPathLike(value) || value.hasPrefix("-") {
                return "Use a path or command name without quotes or shell characters"
            }
            return value.contains(where: \.isWhitespace) && !isAbsolute(value)
                ? "Use an absolute path (or just \"tofu\" for the one on PATH)" : nil
        case .moduleDir:
            if value.isEmpty { return nil }
            if value.count > 500 || !isPathLike(value) || value.hasPrefix("-") {
                return "Use a directory path without quotes or shell characters"
            }
            return isAbsolute(value) ? nil : "Use an absolute path"
        }
    }

    /// The first problem among the four overrides, or nil.
    public static func problems(in settings: AdvancedSettings) -> [AdvancedField: String] {
        var found: [AdvancedField: String] = [:]
        for field in AdvancedField.allCases {
            if let problem = problem(for: field, settings.value(of: field)) { found[field] = problem }
        }
        return found
    }

    public static func regionProblem(_ raw: String) -> String? {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty || matches(value, "^[a-z]{2}(-[a-z]+)+-[0-9]$") ? nil : "Use a region id such as us-east-1 or eu-west-2"
    }

    /// The monthly cap typed as text: empty is "no cap".
    public static func cap(_ raw: String) -> (value: Double?, problem: String?) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return (nil, nil) }
        guard let number = Double(text), number.isFinite, number > 0 else { return (nil, "Use a positive number of dollars") }
        return (number, nil)
    }
}

/// Which secret a pasted value must look like.
public enum SecretKind: String, Sendable {
    case anthropicKey, openaiKey, githubToken

    /// Loose prefix checks that catch the common paste mistakes (the wrong key in the wrong box, stray spaces).
    public func problem(_ value: String) -> String? {
        if value != value.trimmingCharacters(in: .whitespacesAndNewlines) || value.contains(where: \.isWhitespace) {
            return "Remove spaces and line breaks around the value"
        }
        func matches(_ pattern: String) -> Bool { value.range(of: pattern, options: .regularExpression) != nil }
        switch self {
        case .anthropicKey:
            return matches("^sk-ant-[A-Za-z0-9_-]{8,}$") ? nil : "Anthropic keys start with sk-ant-"
        case .openaiKey:
            return matches("^sk-(?!ant-)[A-Za-z0-9_-]{16,}$") ? nil : "OpenAI keys start with sk- (not sk-ant-) and are at least 20 characters"
        case .githubToken:
            return matches("^(gh[pousr]_[A-Za-z0-9]{20,255}|github_pat_[A-Za-z0-9_]{20,255}|[0-9a-f]{40})$")
                ? nil : "GitHub tokens start with ghp_, gho_, ghu_, ghs_ or github_pat_"
        }
    }

    /// Long enough to try, and without a format problem: the Add button's condition.
    public func isAcceptable(_ value: String) -> Bool {
        value.count >= 8 && problem(value) == nil
    }
}
