import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

// The sandbox and its teardown receipt.

extension DaemonRatchetClient {
    public func sandboxStatus() async throws -> SandboxStatus {
        try await wire.calling { DaemonMapping.sandbox(try await environments()) }
    }

    public func endSandbox() async throws -> TeardownReceipt {
        try await wire.calling {
            guard let id = DaemonMapping.endableSandboxID(try await environments()) else {
                throw RatchetClientError.invalid("The sandbox is already off.")
            }
            let after = try await api.endEnvironment(path: .init(sandboxId: id)).ok.body.json
            guard let receipt = after.lastReceipt else { throw RatchetClientError.unavailable("The sandbox ended, but left no receipt.") }
            return await mappedReceipt(receipt)
        }
    }

    public func sandboxReceipt() async throws -> TeardownReceipt? {
        try await wire.calling {
            guard let receipt = try await environments().lastReceipt else { return nil }
            return await mappedReceipt(receipt)
        }
    }

    /// The receipt has no start time; the earliest start of the runs it covers is the best answer.
    func mappedReceipt(_ receipt: VM.TeardownReceiptVm) async -> TeardownReceipt {
        let covered = Set(receipt.runs.map(\.optimizationId))
        let starts = ((try? await optimizationSnapshot()) ?? []).filter { covered.contains($0.id) }.map(\.createdAt)
        return DaemonMapping.receipt(receipt, startedAt: starts.min().map(DaemonMapping.date(ms:)))
    }
}
