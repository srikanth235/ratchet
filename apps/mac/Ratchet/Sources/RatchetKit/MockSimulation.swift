import Foundation

/// The life of the sample world: what happens when a simulated minute passes. Each call moves every running optimization one
/// step forward: a candidate appears, gets a verdict on the next step, a win becomes the new best, money is spent, time runs
/// down, and at zero the run ends, the sandbox is torn down and a receipt is issued. A value type with no clock and no
/// randomness, so a test can drive it without a client (`MockRatchetClient` holds one and sends its events to the stream).
struct MockSimulation: Sendable {
    private var scriptIndex: [String: Int] = [:]
    private var pending: [String: String] = [:]

    private struct ScriptStep {
        var title: String
        var agent: String
        var verdict: Verdict
        var gain: Double
    }

    private static let script: [ScriptStep] = [
        ScriptStep(title: "Prefetch the shipping rates", agent: "Claude Code · team key", verdict: .verified, gain: 0.04),
        ScriptStep(title: "Trim the session lookup", agent: "Claude Code · your login", verdict: .noise, gain: 0),
        ScriptStep(title: "Warm the pricing cache", agent: "Claude Code · your login", verdict: .verified, gain: 0.03),
        ScriptStep(title: "Rewrite the cart serializer", agent: "Claude Code · team key", verdict: .blocked, gain: 0),
        ScriptStep(title: "Make the audit write async", agent: "Claude Code · team key", verdict: .verified, gain: 0.02),
    ]

    /// Moves every running optimization one simulated minute forward and returns what changed.
    mutating func advance(_ world: inout MockWorld) -> [RatchetEvent] {
        world.now = world.now.addingTimeInterval(60)
        var events: [RatchetEvent] = []
        for optimization in world.optimizations where optimization.status == .running {
            events += advance(optimizationID: optimization.id, in: &world)
        }
        return events
    }

    private mutating func advance(optimizationID id: String, in world: inout MockWorld) -> [RatchetEvent] {
        guard var detail = world.details[id] else { return [] }
        var optimization = detail.optimization
        guard detail.agents.contains(where: { $0.state == .working }) else { return [] }
        var events: [RatchetEvent] = []

        optimization.spentUSD += 0.4
        optimization.secondsLeft = max((optimization.secondsLeft ?? 0) - 60, 0)
        if world.sandbox.isOn { world.sandbox.costSoFarUSD += 0.01 }

        let index = scriptIndex[id, default: 0]
        if let candidateID = pending[id], let at = detail.candidates.firstIndex(where: { $0.id == candidateID }) {
            // Give the evaluating candidate its verdict.
            let step = Self.script[(index - 1 + Self.script.count) % Self.script.count]
            var candidate = detail.candidates[at]
            candidate.verdict = step.verdict
            let current = optimization.best ?? optimization.baseline
            switch step.verdict {
            case .verified:
                let better = optimization.metric.lowerIsBetter ? current * (1 - step.gain) : current * (1 + step.gain)
                candidate.value = better
                optimization.best = better
                optimization.winsCount += 1
                optimization.valuePerYearUSD = (optimization.valuePerYearUSD ?? 0) + 450
                let move = RatchetMove(id: "\(id)-move-\(detail.moves.count + 1)", at: world.now, value: better, title: candidate.title)
                detail.moves.append(move)
                world.winSeries.append(MockWorld.WinStep(
                    date: world.now, valueUSD: world.totalValue() + 450, title: candidate.title, optimizationTitle: optimization.title))
                events.append(.moveAdded(optimizationID: id, move: move))
            case .noise:
                candidate.value = current * 1.01
            default:
                candidate.value = nil
            }
            optimization.evalsCount += 1
            detail.candidates[at] = candidate
            events.append(.candidateUpdated(optimizationID: id, candidate: candidate))
            pending[id] = nil
            bump(&detail, agentName: candidate.agent, world: &world, evalsBy: 1, winsBy: step.verdict == .verified ? 1 : 0, activity: nil, events: &events, id: id)
        } else {
            // A new candidate starts evaluating.
            let step = Self.script[index % Self.script.count]
            scriptIndex[id] = index + 1
            let candidate = Candidate(
                id: String(format: "%07lx", 0x5a31c00 + (index + 1) * 4099), title: step.title, agent: step.agent,
                verdict: .evaluating, value: nil, at: world.now)
            detail.candidates.insert(candidate, at: 0)
            pending[id] = candidate.id
            events.append(.candidateUpdated(optimizationID: id, candidate: candidate))
            bump(&detail, agentName: step.agent, world: &world, evalsBy: 0, winsBy: 0, activity: step.title, events: &events, id: id)
        }

        if (optimization.secondsLeft ?? 0) <= 0, pending[id] == nil {
            optimization.status = .done
            optimization.secondsLeft = nil
            optimization.prNumber = 215
            for at in detail.agents.indices {
                detail.agents[at].state = .idle
                detail.agents[at].activity = "Finished"
                events.append(.agentUpdated(optimizationID: id, agent: detail.agents[at]))
            }
            if world.sandbox.isOn {
                let receipt = TeardownReceipt(
                    id: "receipt-\(id)", account: world.sandbox.account ?? "acme-prod",
                    startedAt: optimization.startedAt ?? world.now, endedAt: world.now, costUSD: world.sandbox.costSoFarUSD,
                    destroyed: ["EC2 c7i.2xlarge × 1", "EC2 r7g.large × 1", "EBS 120 GB", "Security group", "Key pair"])
                detail.receipt = receipt
                world.receipt = receipt
                world.sandbox = .off
                events.append(.sandboxChanged(.off))
                events.append(.receiptIssued(receipt))
            }
        }

        detail.optimization = optimization
        world.details[id] = detail
        if let at = world.optimizations.firstIndex(where: { $0.id == id }) { world.optimizations[at] = optimization }
        events.append(.optimizationUpdated(optimization))
        if world.sandbox.isOn { events.append(.sandboxChanged(world.sandbox)) }
        if optimization.status == .done { events.append(.needsYouChanged(world.needsYou())) }
        return events
    }

    private func bump(_ detail: inout OptimizationDetail, agentName: String, world: inout MockWorld, evalsBy: Int, winsBy: Int,
                      activity: String?, events: inout [RatchetEvent], id: String) {
        guard let at = detail.agents.firstIndex(where: { $0.name == agentName }) ?? detail.agents.indices.first else { return }
        detail.agents[at].evals += evalsBy
        detail.agents[at].wins += winsBy
        if let activity { detail.agents[at].activity = activity }
        world.appendLog(agent: detail.agents[at], evalsBy: evalsBy, winsBy: winsBy, activity: activity, events: &events, id: id)
        events.append(.agentUpdated(optimizationID: id, agent: detail.agents[at]))
    }
}
