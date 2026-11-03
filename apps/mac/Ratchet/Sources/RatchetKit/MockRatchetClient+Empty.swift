import Foundation

extension MockRatchetClient {
    /// Snapshot hook: a world with nothing in it (no optimizations, wins, benchmarks or sandbox), for the empty states.
    /// `false` brings the sample data back, but only if it was emptied; it never resets a world that has data.
    public func simulate(empty: Bool) {
        if empty {
            world.optimizations = []
            world.details = [:]
            world.winSeries = []
            world.benchmarks = []
            world.sandbox = .off
            world.receipt = nil
            world.drafts = [:]
            world.agentLogs = [:]
        } else if world.optimizations.isEmpty {
            var sample = MockWorld.standard(now: world.now)
            sample.seedLogs()
            world = sample
        }
    }
}
