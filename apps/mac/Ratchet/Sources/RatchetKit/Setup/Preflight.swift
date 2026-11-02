import Foundation

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
