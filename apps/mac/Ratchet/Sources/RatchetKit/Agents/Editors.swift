import Foundation

// The small editors behind the "Change agents", "Change limits" and "Change schedule" popovers. One value each, no views: the
// Home composer chips, the draft card rows and the optimization settings rows all use the same ones, so the words and the
// order read the same on every screen (S-102).

/// How often an optimization runs (the daemon's `ScheduleKind`).
public enum ScheduleKind: String, CaseIterable, Codable, Sendable, Identifiable {
    case once, nightly, weekdays, weekly
    case onPush = "on-push"

    public var id: String { rawValue }

    /// The words of a row or a chip ("Every night"), as the daemon writes them.
    public var title: String {
        switch self {
        case .once: "Once"
        case .nightly: "Every night"
        case .weekdays: "Weekdays at night"
        case .weekly: "Once a week"
        case .onPush: "On every push"
        }
    }

    /// What a person says to the chat to pick it.
    public var instruction: String {
        switch self {
        case .once: "Run it once and do not add a schedule."
        case .nightly: "Run it every night."
        case .weekdays: "Run it on weekdays."
        case .weekly: "Run it once a week."
        case .onPush: "Run it on every push."
        }
    }

    /// Reads the schedule a draft card row shows ("every night at 02:00", "weekdays at 02:00", "run once, now").
    public init?(shown text: String) {
        let lower = text.lowercased()
        if lower.contains("every night") || lower.contains("nightly") {
            self = .nightly
        } else if lower.contains("weekday") {
            self = .weekdays
        } else if lower.contains("week") {
            self = .weekly
        } else if lower.contains("push") {
            self = .onPush
        } else if lower.contains("once") {
            self = .once
        } else {
            return nil
        }
    }
}

/// Where agents run, in the words of the Where switch.
extension Placement {
    /// "This Mac" or "Cloud".
    public var switchTitle: String { self == .local ? "This Mac" : "Cloud" }
    /// "on this Mac" or "in the cloud".
    public var phrase: String { self == .local ? "on this Mac" : "in the cloud" }
}

/// Which harnesses an optimization uses, in order, and where they run: the "Change agents" popover. The first runs; when it
/// cannot, the next takes over.
public struct AgentOrderEditor: Equatable, Sendable {
    public private(set) var order: [Harness]
    public var placement: Placement
    /// The harnesses that can be added (signed in on this Mac, or holding a key).
    public var available: [Harness]

    public init(order: [Harness], placement: Placement = .local, available: [Harness] = Harness.allCases) {
        self.order = order
        self.placement = placement
        self.available = available
    }

    /// Reads a draft card's Agents row: `raw` is `["claude-code", "codex"]`, the shown value ends "· on this Mac" or "· in the cloud".
    public init(row: ChatCardRow) {
        let fromRaw = row.raw.strings.compactMap { Harness(rawValue: $0) }
        self.order = fromRaw.isEmpty ? [Harness.claudeCode] : fromRaw
        self.placement = row.value.lowercased().contains("cloud") ? .cloud : .local
        self.available = row.options.compactMap { Harness(rawValue: $0.value) }
        if available.isEmpty { available = Harness.allCases }
    }

    public func canMoveUp(at index: Int) -> Bool { index > 0 && index < order.count }
    public func canMoveDown(at index: Int) -> Bool { index >= 0 && index < order.count - 1 }

    public mutating func moveUp(at index: Int) {
        guard canMoveUp(at: index) else { return }
        order.swapAt(index, index - 1)
    }

    public mutating func moveDown(at index: Int) {
        guard canMoveDown(at: index) else { return }
        order.swapAt(index, index + 1)
    }

    /// Harnesses not in the list yet that can be added.
    public var addable: [Harness] { available.filter { !order.contains($0) } }

    public mutating func add(_ harness: Harness) {
        if !order.contains(harness) { order.append(harness) }
    }

    /// The list keeps at least one agent: removing the last one does nothing.
    public mutating func remove(_ harness: Harness) {
        guard order.count > 1 else { return }
        order.removeAll { $0 == harness }
    }

    public var canRemove: Bool { order.count > 1 }

    /// "Claude Code → Codex → Grok".
    public var orderSummary: String { order.map(\.title).joined(separator: " → ") }

    /// "Claude Code → Codex → Grok · on this Mac": the row on the card, the optimization page and the Agents page.
    public var summary: String { orderSummary + " · " + placement.phrase }

    /// The chip: the first agent, and how many more are behind it.
    public var chipTitle: String {
        guard let first = order.first else { return "No agent" }
        return order.count > 1 ? "\(first.title) + \(order.count - 1) more" : first.title
    }

    /// What a person says to the chat to set it.
    public var instruction: String {
        let names = order.map(\.title)
        let list: String
        switch names.count {
        case 0: list = "Claude Code"
        case 1: list = names[0]
        default: list = names.joined(separator: ", then ")
        }
        return "Use \(list). Run them \(placement.phrase)."
    }
}

/// The budget and the time one run may use: the "Change limits" popover.
public struct LimitsEditor: Equatable, Sendable {
    public static let budgetChoices = [10, 20, 50, 100, 200, 500]
    public static let hourChoices: [Double] = [1, 2, 4, 8]

    public var budgetUSD: Int
    public var hours: Double

    public init(budgetUSD: Int = 50, hours: Double = 2) {
        self.budgetUSD = budgetUSD
        self.hours = hours
    }

    /// Reads a draft card's Each run row: `raw` is `{"budgetUsd": 50}`, the shown value "up to $50 and 2 h".
    public init(row: ChatCardRow) {
        let budget = row.raw["budgetUsd"]?.number.map { Int($0.rounded()) } ?? 50
        self.init(budgetUSD: budget, hours: Self.hours(in: row.value) ?? 2)
    }

    /// The number before " h" in "up to $50 and 2 h".
    static func hours(in text: String) -> Double? {
        let parts = text.split(separator: " ")
        guard let at = parts.firstIndex(where: { $0 == "h" }), at > 0 else { return nil }
        return Double(parts[at - 1])
    }

    public var summary: String { "up to \(Money.short(Double(budgetUSD))) and \(Elapsed.hm(hours * 3600))" }

    public var instruction: String { "Change each run to \(summary)." }
}

/// One edit of a draft card row, from its popover: what the person chose, ready to be said to the chat.
public enum DraftRowEdit: Equatable, Sendable {
    case agents(AgentOrderEditor)
    case limits(LimitsEditor)
    case schedule(ScheduleKind)
    /// One of the row's options (the measure, the output).
    case option(rowLabel: String, option: ChatCardOption)
    /// Free words for a row without options ("make it about the cold start").
    case text(rowLabel: String, String)

    public var instruction: String {
        switch self {
        case .agents(let editor): editor.instruction
        case .limits(let editor): editor.instruction
        case .schedule(let kind): kind.instruction
        case .option(let label, let option): "Change the \(label.lowercased()) to \(option.label)."
        case .text(let label, let text): "Change the \(label.lowercased()): \(text)"
        }
    }

    /// What a person says to put a row back as it was.
    public static func restoring(_ row: ChatCardRow) -> DraftRowEdit {
        switch row.key {
        case "agents": .agents(AgentOrderEditor(row: row))
        case "limits": .limits(LimitsEditor(row: row))
        case "schedule": .schedule(ScheduleKind(shown: row.value) ?? .once)
        default: .text(rowLabel: row.label, "put it back to \(row.value)")
        }
    }
}
