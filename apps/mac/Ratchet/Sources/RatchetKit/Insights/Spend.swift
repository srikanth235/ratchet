import Foundation

/// One agent's share of the spend. `agentID` is nil for the shared infrastructure (the sandbox machines).
public struct AgentSpend: Identifiable, Hashable, Sendable {
    public var id: String
    public var agentID: String?
    public var name: String
    public var usd: Double
    public var wins: Int

    public init(id: String, agentID: String?, name: String, usd: Double, wins: Int) {
        self.id = id
        self.agentID = agentID
        self.name = name
        self.usd = usd
        self.wins = wins
    }

    public var isShared: Bool { agentID == nil }

    /// Spend for one verified win, or nil with no win.
    public var perWinUSD: Double? { wins > 0 ? usd / Double(wins) : nil }
}

/// Where one optimization's money went: by category (machines, experiments, tokens...) and by agent.
public struct SpendBreakdown: Hashable, Sendable {
    public var totalUSD: Double
    /// The part not yet reconciled with billing.
    public var estimatedUSD: Double
    public var budgetUSD: Double
    public var categories: [SpendLine]
    public var agents: [AgentSpend]
    /// Verified wins of the whole run.
    public var wins: Int

    public init(
        totalUSD: Double, estimatedUSD: Double, budgetUSD: Double, categories: [SpendLine], agents: [AgentSpend], wins: Int
    ) {
        self.totalUSD = totalUSD
        self.estimatedUSD = estimatedUSD
        self.budgetUSD = budgetUSD
        self.categories = categories
        self.agents = agents
        self.wins = wins
    }

    /// Spend for one verified win of the run, or nil with no win.
    public var perWinUSD: Double? { wins > 0 ? totalUSD / Double(wins) : nil }

    /// Each category's share of the total, 0 to 1, in the order of `categories`.
    public var categoryFractions: [Double] {
        let sum = categories.reduce(0) { $0 + $1.usd }
        guard sum > 0 else { return categories.map { _ in 0 } }
        return categories.map { $0.usd / sum }
    }

    /// Each agent's share of the total, 0 to 1.
    public func fraction(of agent: AgentSpend) -> Double {
        totalUSD > 0 ? min(max(agent.usd / totalUSD, 0), 1) : 0
    }

    /// "Includes $1.20 estimated" or "Reconciled with billing".
    public var reconciliationNote: String {
        estimatedUSD > 0 ? "Includes " + Money.exact(estimatedUSD) + " estimated" : "Reconciled with billing"
    }
}
