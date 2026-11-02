import Foundation

/// A `RatchetClient` backed by sample data, with a fake live event stream so running optimizations visibly progress.
///
/// With `live: true` a timer calls `advance()` every `interval`; each call moves every running optimization one
/// simulated minute forward: a candidate appears, gets a verdict on the next step, a win becomes the new best, money
/// is spent, time runs down, and at zero the run ends, the sandbox is torn down and a receipt is issued. Tests and
/// snapshots use `live: false` and call `advance()` themselves.
public actor MockRatchetClient: RatchetClient {
    var world: MockWorld
    /// What happens when a simulated minute passes.
    var simulation = MockSimulation()
    let live: Bool
    let interval: Duration
    var subscribers: [UUID: AsyncStream<RatchetEvent>.Continuation] = [:]
    var ticker: Task<Void, Never>?
    var counter = 0
    var githubFlow: GithubSignInFlow?
    let startedAt: Date
    // Setup state (repo inspection, connector forms, development overrides).
    var keychainAvailable = true
    var redetected = false
    var preflightOverride: [PreflightItem]?
    var advanced = AdvancedSettings(defaultTofuBinary: "tofu", defaultModuleDir: "Ratchet.app/Contents/Resources/infra/aws")

    public init(now: Date = Date(), live: Bool = false, interval: Duration = .seconds(2)) {
        var sample = MockWorld.standard(now: now)
        sample.seedLogs()
        self.world = sample
        self.startedAt = now
        self.live = live
        self.interval = interval
    }

    // MARK: Events

    public func events() async -> AsyncStream<RatchetEvent> {
        let (stream, continuation) = AsyncStream<RatchetEvent>.makeStream()
        let id = UUID()
        subscribers[id] = continuation
        continuation.onTermination = { @Sendable [weak self] _ in
            Task { await self?.removeSubscriber(id) }
        }
        startTickerIfNeeded()
        return stream
    }

    private func removeSubscriber(_ id: UUID) {
        subscribers[id] = nil
    }

    func emit(_ events: [RatchetEvent]) {
        for event in events {
            for continuation in subscribers.values { continuation.yield(event) }
        }
    }

    private func startTickerIfNeeded() {
        guard live, ticker == nil else { return }
        let interval = self.interval
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: interval)
                guard let self else { return }
                await self.advance()
            }
        }
    }

    /// Stops the fake live updates.
    public func stop() {
        ticker?.cancel()
        ticker = nil
    }

    // MARK: Simulation

    /// Moves every running optimization one simulated minute forward and returns what changed (also sent to the event
    /// stream). The rules are `MockSimulation`'s.
    @discardableResult
    public func advance() -> [RatchetEvent] {
        let events = simulation.advance(&world)
        emit(events)
        return events
    }
}

extension MockRatchetClient: ClientScenario {}
