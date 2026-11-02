import Foundation
import Observation

/// The cloud sandbox and the receipt of its last teardown.
@MainActor @Observable
public final class SandboxStore {
    public internal(set) var sandbox: SandboxStatus = .off
    public internal(set) var receipt: TeardownReceipt?

    private let client: any SandboxClient
    private let errors: StoreErrors

    public init(client: any SandboxClient, errors: StoreErrors) {
        self.client = client
        self.errors = errors
    }

    func reset() {
        sandbox = .off
        receipt = nil
    }

    /// Reads the sandbox again (an optimization that starts one just began).
    func refreshStatus() async {
        if let now = try? await client.sandboxStatus() { sandbox = now }
    }

    /// "End now": tears the sandbox down and keeps the receipt.
    public func endSandbox() async {
        errors.beginAction()
        do {
            receipt = try await client.endSandbox()
            sandbox = try await client.sandboxStatus()
        } catch {
            errors.fail(error, "Couldn't end the sandbox")
        }
    }
}
