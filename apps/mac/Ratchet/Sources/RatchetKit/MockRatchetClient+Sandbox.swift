import Foundation

// The sandbox of the sample world.

extension MockRatchetClient {
    // MARK: Sandbox

    public func sandboxStatus() async throws -> SandboxStatus { world.sandbox }

    public func endSandbox() async throws -> TeardownReceipt {
        guard world.sandbox.isOn else { throw RatchetClientError.invalid("The sandbox is already off.") }
        counter += 1
        let receipt = TeardownReceipt(
            id: "receipt-end-\(counter)", account: world.sandbox.account ?? "acme-prod",
            startedAt: world.now.addingTimeInterval(-6480), endedAt: world.now, costUSD: world.sandbox.costSoFarUSD,
            destroyed: ["EC2 c7i.2xlarge × 1", "EC2 r7g.large × 1", "EBS 120 GB", "Security group", "Key pair"])
        world.sandbox = .off
        world.receipt = receipt
        emit([.sandboxChanged(.off), .receiptIssued(receipt)])
        return receipt
    }

    public func sandboxReceipt() async throws -> TeardownReceipt? { world.receipt }
}
