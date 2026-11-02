import Foundation

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
    public var detection: BenchmarkDetection {
        let id = repo ?? ""
        switch outcome {
        case .ready:
            let direction = (config?.lowerIsBetter ?? true) ? "lower" : "higher"
            return BenchmarkDetection(
                repoID: id, found: true, summary: "Found ratchet.yaml: \(config?.metricName ?? ""), \(direction) is better.",
                suggestedTemplateID: suggestedTemplateID)
        case .unusable(let why):
            return BenchmarkDetection(repoID: id, found: false, summary: why, suggestedTemplateID: suggestedTemplateID)
        case .invalid:
            return BenchmarkDetection(repoID: id, found: false, summary: "ratchet.yaml does not parse.", suggestedTemplateID: nil)
        case .missing:
            return BenchmarkDetection(
                repoID: id, found: false, summary: "No benchmark found in this repo. Start from a template.",
                suggestedTemplateID: suggestedTemplateID ?? templates.first?.id)
        }
    }
}
