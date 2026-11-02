import Foundation

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
