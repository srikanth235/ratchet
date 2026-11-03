import Foundation
import Observation

/// The rows of an optimization's settings the daemon holds an API for.
public enum SettingsRowKey: String, CaseIterable, Sendable, Identifiable {
    case agents, value, schedule

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .agents: "Agents"
        case .value: "Value"
        case .schedule: "Schedule"
        }
    }
}

/// "Using default" or "Custom", the tag beside a row.
public enum SettingsTag: String, Sendable {
    case usingDefault = "Using default"
    case custom = "Custom"
}

/// One row as the optimization page draws it: its words, its tag, an Edit button and "Saved" after a save.
public struct SettingsRow: Identifiable, Equatable, Sendable {
    public var key: SettingsRowKey
    public var value: String
    public var tag: SettingsTag
    /// The Edit button works; false when `ratchet.yaml` holds the setting.
    public var isEditable: Bool
    /// Why Edit is off: "Set in ratchet.yaml: change it in the repo."
    public var lockedNote: String?
    public var isSaved: Bool
    public var error: String?

    public var id: String { key.rawValue }
    public var title: String { key.title }
    public var editTitle: String { "Edit" }
}

/// The value row's popover: the fields as the person types them, and what they say when they are saved.
public struct ValueModelEditor: Equatable, Sendable {
    public var usdPerUnitText: String
    public var period: ValuePeriod
    public var basis: ValueBasisKind
    public var unit: String
    public var note: String

    public init(model: ValueModel = .unset) {
        usdPerUnitText = model.usdPerUnit.map { $0 == $0.rounded() ? String(Int($0)) : String($0) } ?? ""
        period = model.period ?? .year
        basis = model.basis ?? .estimated
        unit = model.unit ?? ""
        note = model.note ?? ""
    }

    /// The amount typed, with "$" and "," allowed.
    var amount: Double? {
        Double(usdPerUnitText.filter { $0 != "$" && $0 != "," && $0 != " " })
    }

    public var input: ValueModelInput? {
        guard let amount else { return nil }
        return ValueModelInput(
            usdPerUnit: amount, period: period, basis: basis, unit: unit.isEmpty ? nil : unit, note: note.isEmpty ? nil : note)
    }

    /// Why Save is off, in words; nil when it can be saved.
    public var problem: String? {
        guard amount != nil else { return "Enter what one unit of improvement is worth, in dollars." }
        return input?.problem
    }
}

/// The editable rows of one optimization (Agents through the agents API, Value through the value API, Schedule) and Archive.
/// Each save shows "Saved" beside its row; a refusal is kept on the row with its reason.
@MainActor @Observable
public final class OptimizationSettingsModel {
    public let optimizationID: String
    public private(set) var policy: AgentsPolicy?
    public private(set) var valueModel: ValueModel?
    public private(set) var schedule: ScheduleSetting?
    public private(set) var isLoading = false
    public private(set) var isArchived = false
    /// The row that was just saved.
    public private(set) var savedRow: SettingsRowKey?
    public private(set) var errors: [SettingsRowKey: String] = [:]
    /// The order a new optimization starts with (the Agents page's order): a different list reads "Custom".
    public var defaultOrder: [Harness]

    @ObservationIgnored private let client: any OptimizationSettingsClient

    public init(client: any OptimizationSettingsClient, optimizationID: String, defaultOrder: [Harness] = []) {
        self.client = client
        self.optimizationID = optimizationID
        self.defaultOrder = defaultOrder
    }

    public func load() async {
        isLoading = true
        defer { isLoading = false }
        let id = optimizationID
        async let agents = Self.attempt { try await self.client.agentsPolicy(optimizationID: id) }
        async let value = Self.attempt { try await self.client.valueModel(optimizationID: id) }
        async let schedule = Self.attempt { try await self.client.schedule(optimizationID: id) }
        let (policy, model, setting) = await (agents, value, schedule)
        apply(policy, to: .agents) { self.policy = $0 }
        apply(model, to: .value) { self.valueModel = $0 }
        apply(setting, to: .schedule) { self.schedule = $0 }
    }

    private static func attempt<T: Sendable>(_ work: @Sendable () async throws -> T) async -> Result<T, any Error> {
        do { return .success(try await work()) } catch { return .failure(error) }
    }

    private func apply<T>(_ result: Result<T, any Error>, to key: SettingsRowKey, set: (T) -> Void) {
        switch result {
        case .success(let value):
            set(value)
            errors[key] = nil
        case .failure(let failure):
            errors[key] = failure.localizedDescription
        }
    }

    // MARK: Rows

    public var rows: [SettingsRow] {
        SettingsRowKey.allCases.map { key in
            var row = SettingsRow(
                key: key, value: "", tag: .usingDefault, isEditable: true, lockedNote: nil, isSaved: savedRow == key, error: errors[key])
            switch key {
            case .agents:
                if let policy {
                    row.value = policy.summary
                    let custom = policy.isLockedByConfig || (!defaultOrder.isEmpty && policy.harnesses != defaultOrder)
                    row.tag = custom ? .custom : .usingDefault
                    row.isEditable = !policy.isLockedByConfig
                    if policy.isLockedByConfig { row.lockedNote = "Set in ratchet.yaml: change it in the repo." }
                }
            case .value:
                row.value = valueModel?.summary ?? ""
                row.tag = valueModel?.isSet == true ? .custom : .usingDefault
            case .schedule:
                row.value = schedule?.summary ?? ""
                row.tag = schedule?.kind != nil ? .custom : .usingDefault
            }
            return row
        }
    }

    /// The Agents popover's starting point; the Where switch is not here (the daemon holds the list only).
    public var agentsEditor: AgentOrderEditor? {
        policy.map { AgentOrderEditor(order: $0.harnesses, placement: .local, available: Harness.allCases) }
    }

    public var valueEditor: ValueModelEditor { ValueModelEditor(model: valueModel ?? .unset) }

    // MARK: Saving

    /// Saves the order (and the fallback rules, when given). True when it was saved.
    @discardableResult
    public func saveAgents(_ editor: AgentOrderEditor, rules: AgentsPolicyInput? = nil) async -> Bool {
        var input = rules ?? AgentsPolicyInput(harnesses: editor.order)
        input.harnesses = editor.order
        return await save(.agents) { self.policy = try await self.client.setAgentsPolicy(optimizationID: self.optimizationID, input) }
    }

    @discardableResult
    public func saveValue(_ editor: ValueModelEditor) async -> Bool {
        if let problem = editor.problem {
            errors[.value] = problem
            return false
        }
        guard let input = editor.input else { return false }
        return await save(.value) { self.valueModel = try await self.client.setValueModel(optimizationID: self.optimizationID, input) }
    }

    @discardableResult
    public func clearValue() async -> Bool {
        await save(.value) { self.valueModel = try await self.client.clearValueModel(optimizationID: self.optimizationID) }
    }

    /// nil clears the schedule.
    @discardableResult
    public func saveSchedule(_ kind: ScheduleKind?) async -> Bool {
        await save(.schedule) { self.schedule = try await self.client.setSchedule(optimizationID: self.optimizationID, kind) }
    }

    private func save(_ key: SettingsRowKey, _ work: () async throws -> Void) async -> Bool {
        savedRow = nil
        errors[key] = nil
        do {
            try await work()
            savedRow = key
            return true
        } catch {
            errors[key] = error.localizedDescription
            return false
        }
    }

    // MARK: Pause Schedule

    /// The schedule that was paused, so Resume Schedule can put it back. Kept for this session: the daemon holds no paused state,
    /// pausing is clearing the schedule.
    public private(set) var pausedSchedule: ScheduleKind?

    public var canPauseSchedule: Bool { schedule?.kind != nil }
    public var isSchedulePaused: Bool { pausedSchedule != nil && schedule?.kind == nil }

    /// "Pause Schedule" or "Resume Schedule".
    public var pauseTitle: String { isSchedulePaused ? "Resume Schedule" : "Pause Schedule" }

    /// Stops the schedule for now; the runs and wins stay.
    @discardableResult
    public func pauseSchedule() async -> Bool {
        guard let kind = schedule?.kind else { return false }
        let done = await saveSchedule(nil)
        if done { pausedSchedule = kind }
        return done
    }

    /// Puts the paused schedule back.
    @discardableResult
    public func resumeSchedule() async -> Bool {
        guard let kind = pausedSchedule else { return false }
        let done = await saveSchedule(kind)
        if done { pausedSchedule = nil }
        return done
    }

    /// The header button: pauses, or resumes.
    @discardableResult
    public func togglePause() async -> Bool {
        isSchedulePaused ? await resumeSchedule() : await pauseSchedule()
    }

    // MARK: Archive

    /// What the confirmation says: "Archive “Checkout p95”?" and what it does.
    public static func archivePrompt(title: String) -> (title: String, message: String, confirm: String) {
        ("Archive “\(title)”?", "Stops the schedule and keeps its history and wins.", "Archive")
    }

    /// Stops the schedule; the history and the wins stay. (The daemon has no archive route: clearing the schedule is what
    /// archiving does, and the optimization keeps its runs.)
    @discardableResult
    public func archive() async -> Bool {
        let done = await saveSchedule(nil)
        if done {
            isArchived = true
            savedRow = nil
        }
        return done
    }
}
