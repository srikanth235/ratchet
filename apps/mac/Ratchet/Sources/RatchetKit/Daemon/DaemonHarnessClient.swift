import Foundation
import OpenAPIRuntime

/// `HarnessClient` on the real daemon: JSON calls through the shared transport and its error middleware.
public final class DaemonHarnessClient: HarnessClient {
    let wire: DaemonTransport

    init(wire: DaemonTransport) {
        self.wire = wire
    }

    public convenience init(transport: any ClientTransport) {
        self.init(wire: DaemonTransport(transport: transport))
    }

    public convenience init(location: DaemonLocation) {
        self.init(wire: DaemonTransport(location: location))
    }

    private func path(_ optimizationID: String, _ part: String) -> String {
        "/v1/optimizations/\(wire.escaped(optimizationID))/\(part)"
    }

    // MARK: Agents

    public func agentsPolicy(optimizationID: String) async throws -> AgentsPolicy {
        try await wire.calling {
            let data = try await wire.jsonCall(.get, path(optimizationID, "agents"), operationID: "getOptimizationAgents")
            return HarnessWire.policy(try wire.decode(HarnessWire.Policy.self, data))
        }
    }

    public func setAgentsPolicy(optimizationID: String, _ input: AgentsPolicyInput) async throws -> AgentsPolicy {
        if let problem = input.problem { throw RatchetClientError.invalid(problem) }
        return try await wire.calling {
            let data = try await wire.jsonCall(
                .put, path(optimizationID, "agents"), operationID: "setOptimizationAgents", json: try HarnessWire.policyBody(input))
            return HarnessWire.policy(try wire.decode(HarnessWire.Policy.self, data))
        }
    }

    // MARK: Value

    public func valueModel(optimizationID: String) async throws -> ValueModel {
        try await wire.calling {
            let data = try await wire.jsonCall(.get, path(optimizationID, "value"), operationID: "getOptimizationValue")
            return HarnessWire.value(try wire.decode(HarnessWire.Value.self, data))
        }
    }

    public func setValueModel(optimizationID: String, _ input: ValueModelInput) async throws -> ValueModel {
        if let problem = input.problem { throw RatchetClientError.invalid(problem) }
        return try await wire.calling {
            let data = try await wire.jsonCall(
                .put, path(optimizationID, "value"), operationID: "setOptimizationValue", json: try HarnessWire.valueBody(input))
            return HarnessWire.value(try wire.decode(HarnessWire.Value.self, data))
        }
    }

    public func clearValueModel(optimizationID: String) async throws -> ValueModel {
        _ = try await wire.calling {
            try await wire.jsonCall(.delete, path(optimizationID, "value"), operationID: "clearOptimizationValue")
        }
        return try await valueModel(optimizationID: optimizationID)
    }

    // MARK: Schedule

    public func schedule(optimizationID: String) async throws -> ScheduleSetting {
        try await wire.calling {
            let data = try await wire.jsonCall(.get, path(optimizationID, "schedule"), operationID: "getOptimizationSchedule")
            return HarnessWire.schedule(try wire.decode(HarnessWire.Schedule.self, data))
        }
    }

    public func setSchedule(optimizationID: String, _ kind: ScheduleKind?) async throws -> ScheduleSetting {
        try await wire.calling {
            let data = try await wire.jsonCall(
                .put, path(optimizationID, "schedule"), operationID: "setOptimizationSchedule", json: try HarnessWire.scheduleBody(kind))
            return HarnessWire.schedule(try wire.decode(HarnessWire.Schedule.self, data))
        }
    }

    // MARK: The value story

    public func home(range: TimeRange) async throws -> HomeSummary {
        try await wire.calling {
            let query = DaemonTransport.queryItem("range", HarnessWire.rangeParameter(range))
            let data = try await wire.jsonCall(.get, "/v1/home?" + query, operationID: "getHome")
            return HarnessWire.home(try wire.decode(HarnessWire.Home.self, data), range: range)
        }
    }

    public func valueOverview(range: TimeRange) async throws -> ValueOverview {
        try await wire.calling {
            let query = DaemonTransport.queryItem("range", HarnessWire.rangeParameter(range))
            let data = try await wire.jsonCall(.get, "/v1/overview?" + query, operationID: "getOverview")
            return HarnessWire.overview(try wire.decode(HarnessWire.Overview.self, data), range: range)
        }
    }
}
