import Foundation

/// The per-optimization settings the harness work added: the agents list (with its fallback rules), the value model and the
/// schedule. Separate from `RatchetClient` like `ChatClient`: new operations, one protocol, a daemon implementation
/// (`DaemonHarnessClient`) and a mock (`MockHarnessClient`), with one contract suite over both.
public protocol OptimizationSettingsClient: Sendable {
    func agentsPolicy(optimizationID: String) async throws -> AgentsPolicy
    /// Throws `RatchetClientError.invalid` for an empty list or a rule of zero, and while `ratchet.yaml` has an `agents:` section.
    func setAgentsPolicy(optimizationID: String, _ input: AgentsPolicyInput) async throws -> AgentsPolicy

    func valueModel(optimizationID: String) async throws -> ValueModel
    func setValueModel(optimizationID: String, _ input: ValueModelInput) async throws -> ValueModel
    func clearValueModel(optimizationID: String) async throws -> ValueModel

    func schedule(optimizationID: String) async throws -> ScheduleSetting
    /// nil clears the schedule: the optimization then runs only when started.
    func setSchedule(optimizationID: String, _ kind: ScheduleKind?) async throws -> ScheduleSetting
}

/// The value story Home and the Overview both tell: the same numbers by construction.
public protocol ValueStoryClient: Sendable {
    func home(range: TimeRange) async throws -> HomeSummary
    func valueOverview(range: TimeRange) async throws -> ValueOverview
}

public typealias HarnessClient = OptimizationSettingsClient & ValueStoryClient
