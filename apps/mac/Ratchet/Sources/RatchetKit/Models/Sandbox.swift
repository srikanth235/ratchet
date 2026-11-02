import Foundation

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
