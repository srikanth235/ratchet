import Foundation

/// The kinds of notification the app can post. Each has its own switch in Settings.
public enum NotificationCategory: String, CaseIterable, Codable, Sendable, Identifiable {
    case winReady, environmentDrafted, budgetLimit, runFinished, connectorExpired

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .winReady: "Wins ready to merge"
        case .environmentDrafted: "Environment waiting for approval"
        case .budgetLimit: "Budget limit reached"
        case .runFinished: "Run finished or failed"
        case .connectorExpired: "Connector expired"
        }
    }

    public var explanation: String {
        switch self {
        case .winReady: "A verified win is ready to become a pull request."
        case .environmentDrafted: "An agent drafted a test environment and waits for your approval."
        case .budgetLimit: "An optimization stopped because it spent its budget."
        case .runFinished: "An optimization finished, or a run failed."
        case .connectorExpired: "A GitHub, AWS or agent sign-in needs renewing."
        }
    }

    /// The `UserDefaults` key of this category's switch (on unless set).
    public var defaultsKey: String { "notify." + rawValue }
}

/// Where a click on a notification goes.
public enum NotificationDestination: Equatable, Sendable {
    case optimization(String)
    case connectors
    /// The New optimization flow: an environment draft waits there.
    case newOptimization
    case overview

    /// A plain string for the notification's `userInfo`.
    public var token: String {
        switch self {
        case .optimization(let id): "optimization:" + id
        case .connectors: "connectors"
        case .newOptimization: "new"
        case .overview: "overview"
        }
    }

    public init?(token: String) {
        if token.hasPrefix("optimization:") {
            self = .optimization(String(token.dropFirst("optimization:".count)))
        } else if token == "connectors" {
            self = .connectors
        } else if token == "new" {
            self = .newOptimization
        } else if token == "overview" {
            self = .overview
        } else {
            return nil
        }
    }
}

public struct NotificationRequest: Equatable, Sendable, Identifiable {
    /// Stable per event, so the same event never posts twice.
    public var id: String
    public var category: NotificationCategory
    public var title: String
    public var body: String
    public var destination: NotificationDestination

    public init(id: String, category: NotificationCategory, title: String, body: String, destination: NotificationDestination) {
        self.id = id
        self.category = category
        self.title = title
        self.body = body
        self.destination = destination
    }
}

/// What the store shows at one moment; two of them are compared to find what is new.
public struct ObservedState: Equatable, Sendable {
    public var optimizations: [Optimization]
    public var needsYou: [NeedsYouItem]
    public var connectors: [Connector]

    public init(optimizations: [Optimization], needsYou: [NeedsYouItem], connectors: [Connector]) {
        self.optimizations = optimizations
        self.needsYou = needsYou
        self.connectors = connectors
    }
}

/// Decides which notifications a change in the store deserves. Pure, so the tests cover every rule.
public enum NotificationRouting {
    public static func changes(
        from old: ObservedState, to new: ObservedState, enabled: Set<NotificationCategory> = Set(NotificationCategory.allCases)
    ) -> [NotificationRequest] {
        var found: [NotificationRequest] = []
        let knownNeeds = Set(old.needsYou.map(\.id))
        var covered: Set<String> = []

        for item in new.needsYou where !knownNeeds.contains(item.id) {
            let category: NotificationCategory
            let destination: NotificationDestination
            switch item.kind {
            case .mergeWins: category = .winReady; destination = .optimization(item.optimizationID)
            case .approveEnvironment: category = .environmentDrafted; destination = .newOptimization
            case .budgetLimit: category = .budgetLimit; destination = .optimization(item.optimizationID)
            case .runFailed: category = .runFinished; destination = .optimization(item.optimizationID)
            case .signIn: category = .connectorExpired; destination = .connectors
            }
            if category == .budgetLimit || category == .runFinished { covered.insert(item.optimizationID) }
            found.append(NotificationRequest(
                id: "needs-" + item.id, category: category, title: item.title, body: item.detail, destination: destination))
        }

        let before = Dictionary(old.optimizations.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for optimization in new.optimizations {
            guard let previous = before[optimization.id], previous.status == .running, optimization.status.isFinished,
                  !covered.contains(optimization.id),
                  !new.needsYou.contains(where: { $0.optimizationID == optimization.id && $0.kind == .runFailed })
            else { continue }
            if let budget = optimization.budgetUSD, budget > 0, optimization.spentUSD >= budget * 0.98 {
                found.append(NotificationRequest(
                    id: "budget-" + optimization.id, category: .budgetLimit, title: optimization.title + " hit its budget",
                    body: "It spent \(Money.short(optimization.spentUSD)) of \(Money.short(budget)) and stopped.",
                    destination: .optimization(optimization.id)))
            } else {
                let wins = optimization.winsCount
                let body = wins == 0 ? "No win this time." : wins == 1 ? "1 verified win." : "\(wins) verified wins."
                found.append(NotificationRequest(
                    id: "finished-" + optimization.id, category: .runFinished, title: optimization.title + " finished",
                    body: body, destination: .optimization(optimization.id)))
            }
        }

        let connectorsBefore = Dictionary(old.connectors.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for connector in new.connectors where connector.state == .expired {
            guard let previous = connectorsBefore[connector.id], previous.state != .expired else { continue }
            found.append(NotificationRequest(
                id: "connector-" + connector.id, category: .connectorExpired, title: connector.name + " needs signing in again",
                body: "Its sign-in expired. Optimizations that use it cannot start.", destination: .connectors))
        }
        return found.filter { enabled.contains($0.category) }
    }

    /// An agent finished drafting an environment.
    public static func environmentDrafted(
        draftID: String, repoID: String, enabled: Set<NotificationCategory> = Set(NotificationCategory.allCases)
    ) -> NotificationRequest? {
        guard enabled.contains(.environmentDrafted) else { return nil }
        return NotificationRequest(
            id: "draft-" + draftID, category: .environmentDrafted, title: "Environment drafted for " + repoID,
            body: "Review it and approve to continue.", destination: .newOptimization)
    }
}
