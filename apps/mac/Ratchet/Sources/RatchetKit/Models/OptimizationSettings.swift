import Foundation

// What the settings rows of one optimization hold: its agents list, its value model and its schedule.

/// Where the agents list in force comes from.
public enum AgentsPolicySource: String, Codable, Sendable {
    /// The optimization's settings: this app changes them.
    case settings
    /// An `agents:` section in `ratchet.yaml` overrides the settings; it is changed in the repo.
    case config
}

/// Which harnesses an optimization may use, in order, and the rules for moving between them. The first runs; when it cannot, the
/// next takes over; nothing waits on a person.
public struct AgentsPolicy: Hashable, Sendable {
    public static let defaultWaitLimitSeconds = 1800
    public static let defaultCrashStrikes = 3
    public static let defaultRecheckSeconds = 300

    public var harnesses: [Harness]
    /// A rate limit that lifts within this many seconds is waited out; a longer or unknown one switches agent.
    public var waitLimitSeconds: Int
    /// Consecutive crashed turns on one agent before switching.
    public var crashStrikes: Int
    /// While every agent is unavailable, how often to look again.
    public var recheckSeconds: Int
    public var source: AgentsPolicySource
    /// "Claude Code → Codex → Grok".
    public var summary: String

    public init(
        harnesses: [Harness], waitLimitSeconds: Int = AgentsPolicy.defaultWaitLimitSeconds, crashStrikes: Int = AgentsPolicy.defaultCrashStrikes,
        recheckSeconds: Int = AgentsPolicy.defaultRecheckSeconds, source: AgentsPolicySource = .settings, summary: String? = nil
    ) {
        self.harnesses = harnesses
        self.waitLimitSeconds = waitLimitSeconds
        self.crashStrikes = crashStrikes
        self.recheckSeconds = recheckSeconds
        self.source = source
        self.summary = summary ?? harnesses.map(\.title).joined(separator: " → ")
    }

    /// The list is in `ratchet.yaml`: change it in the repo, not here.
    public var isLockedByConfig: Bool { source == .config }
}

/// The new list for an optimization; rules left nil keep the daemon's defaults.
public struct AgentsPolicyInput: Hashable, Sendable {
    public var harnesses: [Harness]
    public var waitLimitSeconds: Int?
    public var crashStrikes: Int?
    public var recheckSeconds: Int?

    public init(harnesses: [Harness], waitLimitSeconds: Int? = nil, crashStrikes: Int? = nil, recheckSeconds: Int? = nil) {
        self.harnesses = harnesses
        self.waitLimitSeconds = waitLimitSeconds
        self.crashStrikes = crashStrikes
        self.recheckSeconds = recheckSeconds
    }

    /// Why this cannot be saved, in words; nil when it can.
    public var problem: String? {
        if harnesses.isEmpty { return "Pick at least one agent." }
        if Set(harnesses).count != harnesses.count { return "Each agent can be in the list once." }
        if let waitLimitSeconds, waitLimitSeconds <= 0 { return "The wait limit must be more than zero." }
        if let crashStrikes, crashStrikes <= 0 { return "The crash limit must be more than zero." }
        if let recheckSeconds, recheckSeconds <= 0 { return "The re-check time must be more than zero." }
        return nil
    }
}

public enum ValuePeriod: String, CaseIterable, Codable, Sendable {
    case month, year

    public var title: String { self == .year ? "a year" : "a month" }
}

/// What an improvement is worth: dollars per unit of improvement per period, with the reasoning in words. The Value row; the
/// Overview footnote's "Edit value" opens it. Changing it re-values every win.
public struct ValueModel: Hashable, Sendable {
    public var isSet: Bool
    /// The metric the model prices, as the optimization measures it.
    public var metric: String
    public var lowerIsBetter: Bool
    public var usdPerUnit: Double?
    public var period: ValuePeriod?
    /// What one unit is called ("ms", "k/s").
    public var unit: String?
    public var basis: ValueBasisKind?
    /// The model in plain words: "ms saved × checkout volume".
    public var note: String?
    /// "$1,200 per ms a year · estimated · ms saved x checkout volume", or "Not set".
    public var summary: String

    public init(
        isSet: Bool, metric: String = "", lowerIsBetter: Bool = true, usdPerUnit: Double? = nil, period: ValuePeriod? = nil,
        unit: String? = nil, basis: ValueBasisKind? = nil, note: String? = nil, summary: String = "Not set"
    ) {
        self.isSet = isSet
        self.metric = metric
        self.lowerIsBetter = lowerIsBetter
        self.usdPerUnit = usdPerUnit
        self.period = period
        self.unit = unit
        self.basis = basis
        self.note = note
        self.summary = summary
    }

    public static let unset = ValueModel(isSet: false)
}

/// The value model to save: it replaces the whole model.
public struct ValueModelInput: Hashable, Sendable {
    public static let maxNote = 200
    public static let maxUnit = 24

    public var usdPerUnit: Double
    public var period: ValuePeriod
    public var basis: ValueBasisKind
    public var unit: String?
    public var note: String?

    public init(usdPerUnit: Double, period: ValuePeriod = .year, basis: ValueBasisKind = .estimated, unit: String? = nil, note: String? = nil) {
        self.usdPerUnit = usdPerUnit
        self.period = period
        self.basis = basis
        self.unit = unit
        self.note = note
    }

    /// Why this cannot be saved, in words; nil when it can.
    public var problem: String? {
        if !(usdPerUnit > 0) { return "Enter what one unit of improvement is worth, more than $0." }
        if (unit?.count ?? 0) > Self.maxUnit { return "The unit is at most \(Self.maxUnit) characters." }
        if (note?.count ?? 0) > Self.maxNote { return "The note is at most \(Self.maxNote) characters." }
        return nil
    }
}

/// How often the optimization runs; nil `kind` means it runs when started by hand.
public struct ScheduleSetting: Hashable, Sendable {
    public var kind: ScheduleKind?
    /// "Every night", "Once a week", "Not scheduled".
    public var summary: String
    /// Runs this schedule starts in the next 30 days; nil when it cannot be known ("on every push").
    public var runsNext30Days: Int?

    public init(kind: ScheduleKind?, summary: String? = nil, runsNext30Days: Int? = nil) {
        self.kind = kind
        self.summary = summary ?? kind?.title ?? "Not scheduled"
        self.runsNext30Days = runsNext30Days
    }
}
