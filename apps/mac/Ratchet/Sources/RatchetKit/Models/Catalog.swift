import Foundation

public struct BenchmarkRun: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var title: String
    public var optimizationTitle: String
    public var templateID: String
    public var metric: MetricSpec
    public var baseline: Double
    public var best: Double?
    public var noise: Double
    public var evals: Int
    public var date: Date

    public init(
        id: String, title: String, optimizationTitle: String, templateID: String, metric: MetricSpec,
        baseline: Double, best: Double? = nil, noise: Double, evals: Int, date: Date
    ) {
        self.id = id
        self.title = title
        self.optimizationTitle = optimizationTitle
        self.templateID = templateID
        self.metric = metric
        self.baseline = baseline
        self.best = best
        self.noise = noise
        self.evals = evals
        self.date = date
    }

    public var change: Double? {
        guard let best, baseline != 0 else { return nil }
        return (best - baseline) / baseline
    }

    public var isImprovement: Bool {
        guard let change else { return false }
        return metric.lowerIsBetter ? change < 0 : change > 0
    }
}

public enum TemplateKind: String, Codable, Sendable {
    case template, demo
}

public struct Template: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var kind: TemplateKind
    public var category: String
    public var title: String
    public var summary: String
    public var measures: String
    public var machine: String

    public init(id: String, kind: TemplateKind, category: String, title: String, summary: String, measures: String, machine: String) {
        self.id = id
        self.kind = kind
        self.category = category
        self.title = title
        self.summary = summary
        self.measures = measures
        self.machine = machine
    }
}

public struct Repo: Identifiable, Codable, Hashable, Sendable {
    /// "owner/name".
    public var id: String
    public var defaultBranch: String
    public var account: String
    /// Shown in the search results; nil when the source does not say.
    public var isPrivate: Bool?
    public var summary: String?

    public init(id: String, defaultBranch: String = "main", account: String, isPrivate: Bool? = nil, summary: String? = nil) {
        self.id = id
        self.defaultBranch = defaultBranch
        self.account = account
        self.isPrivate = isPrivate
        self.summary = summary
    }

    public var name: String { id.split(separator: "/").last.map(String.init) ?? id }
}

public struct BenchmarkDetection: Codable, Hashable, Sendable {
    public var repoID: String
    public var found: Bool
    public var summary: String
    public var suggestedTemplateID: String?

    public init(repoID: String, found: Bool, summary: String, suggestedTemplateID: String? = nil) {
        self.repoID = repoID
        self.found = found
        self.summary = summary
        self.suggestedTemplateID = suggestedTemplateID
    }
}
