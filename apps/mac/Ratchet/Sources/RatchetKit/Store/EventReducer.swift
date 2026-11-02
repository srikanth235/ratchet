import Foundation

/// The parts of the store that events change, as one value. The reducer reads and writes only this, so the rules are tested
/// without a store, a client or a main actor.
public struct LiveState: Sendable {
    public var overview: Overview?
    public var optimizations: [Optimization]
    public var details: [String: OptimizationDetail]
    public var connectors: [Connector]
    public var githubFlow: GithubSignInFlow?
    public var sandbox: SandboxStatus
    public var receipt: TeardownReceipt?
    /// The newest move of each optimization that the overview's staircase has not named yet: the next win on it is called by
    /// this move's title.
    public var pendingWins: [String: RatchetMove]

    public init(
        overview: Overview? = nil, optimizations: [Optimization] = [], details: [String: OptimizationDetail] = [:],
        connectors: [Connector] = [], githubFlow: GithubSignInFlow? = nil, sandbox: SandboxStatus = .off,
        receipt: TeardownReceipt? = nil, pendingWins: [String: RatchetMove] = [:]
    ) {
        self.overview = overview
        self.optimizations = optimizations
        self.details = details
        self.connectors = connectors
        self.githubFlow = githubFlow
        self.sandbox = sandbox
        self.receipt = receipt
        self.pendingWins = pendingWins
    }

    /// What notifications are decided from.
    public var observed: ObservedState {
        ObservedState(optimizations: optimizations, needsYou: overview?.needsYou ?? [], connectors: connectors)
    }
}

/// Which part of `LiveState` a reduction changed, so the store writes (and views re-render) only what moved.
public enum LivePart: Hashable, Sendable {
    case overview, optimizations, details, connectors, githubFlow, sandbox, receipt
}

/// What one event did: the new state, which parts changed, and what the store must do besides storing it.
public struct EventReduction: Sendable {
    public var state: LiveState
    public var touched: Set<LivePart> = []
    public var notifications: [NotificationRequest] = []
    /// An agent whose log has new lines: they are fetched if somebody has the log open.
    public var logAppended: String?
    /// The stream lost its place: reload what is shown.
    public var resync = false
    public var streamState: StreamState?

    public init(state: LiveState) {
        self.state = state
    }
}

/// (state, event) to (state, effects). Pure and total: every event has a case, and an event for something that is not loaded
/// changes nothing.
public enum EventReducer {
    /// - Parameters:
    ///   - isLoaded: false before the first good load: nothing is announced then (everything would look new).
    ///   - enabled: the notification categories the person has switched on.
    public static func reduce(
        _ state: LiveState, _ event: RatchetEvent, isLoaded: Bool = true,
        enabled: Set<NotificationCategory> = Set(NotificationCategory.allCases)
    ) -> EventReduction {
        var result = EventReduction(state: state)
        switch event {
        case .optimizationUpdated(let new):
            applyUpdate(new, to: &result)
        case .candidateUpdated(let id, let candidate):
            guard var detail = result.state.details[id] else { break }
            if let at = detail.candidates.firstIndex(where: { $0.id == candidate.id }) {
                detail.candidates[at] = candidate
            } else {
                detail.candidates.insert(candidate, at: 0)
            }
            result.state.details[id] = detail
            result.touched.insert(.details)
        case .moveAdded(let id, let move):
            // The daemon announces the whole ratchet after each win, so most of what arrives is a replay of moves the detail
            // already holds. Only a move that is new, and the newest of those, names the next win on the staircase.
            let loaded = result.state.details[id]
            let known = loaded?.moves.contains { $0.id == move.id } ?? false
            if !known, result.state.pendingWins[id].map({ $0.at <= move.at }) ?? true {
                result.state.pendingWins[id] = move
            }
            guard !known, var detail = loaded else { break }
            detail.moves.append(move)
            result.state.details[id] = detail
            result.touched.insert(.details)
        case .agentUpdated(let id, let agent):
            guard var detail = result.state.details[id] else { break }
            if let at = detail.agents.firstIndex(where: { $0.id == agent.id }) {
                detail.agents[at] = agent
            } else {
                detail.agents.append(agent)
            }
            result.state.details[id] = detail
            result.touched.insert(.details)
        case .agentLogAppended(_, let agentID):
            result.logAppended = agentID
        case .needsYouChanged(let items):
            if result.state.overview != nil {
                result.state.overview?.needsYou = items
                result.touched.insert(.overview)
            }
        case .connectorsChanged(let list):
            result.state.connectors = list
            result.touched.insert(.connectors)
        case .githubFlowChanged(let flow):
            result.state.githubFlow = flow
            result.touched.insert(.githubFlow)
        case .environmentDraftReady:
            break
        case .sandboxChanged(let status):
            result.state.sandbox = status
            result.touched.insert(.sandbox)
        case .receiptIssued(let issued):
            result.state.receipt = issued
            result.touched.insert(.receipt)
        case .resync:
            result.resync = true
        case .streamStateChanged(let streamState):
            result.streamState = streamState
        }
        if isLoaded { result.notifications = notifications(for: event, from: state, to: result.state, enabled: enabled) }
        return result
    }

    /// An optimization the person just started: on screen without waiting for its event, and announced to nobody.
    public static func register(_ optimization: Optimization, in state: LiveState) -> EventReduction {
        var result = EventReduction(state: state)
        applyUpdate(optimization, to: &result)
        return result
    }

    // MARK: Notifications

    private static func notifications(
        for event: RatchetEvent, from old: LiveState, to new: LiveState, enabled: Set<NotificationCategory>
    ) -> [NotificationRequest] {
        if case .environmentDraftReady(let draftID, let repoID) = event {
            return NotificationRouting.environmentDrafted(draftID: draftID, repoID: repoID, enabled: enabled).map { [$0] } ?? []
        }
        return NotificationRouting.changes(from: old.observed, to: new.observed, enabled: enabled)
    }

    // MARK: An optimization changed

    /// Puts the optimization in the list and its detail, and moves the overview's totals by what changed: money spent, value
    /// locked in, wins. A new win is a step on the staircase, named after the move that made it when one is waiting.
    private static func applyUpdate(_ new: Optimization, to result: inout EventReduction) {
        let old = result.state.optimizations.first { $0.id == new.id }
        if let at = result.state.optimizations.firstIndex(where: { $0.id == new.id }) {
            result.state.optimizations[at] = new
        } else {
            result.state.optimizations.insert(new, at: 0)
        }
        result.touched.insert(.optimizations)
        if var detail = result.state.details[new.id] {
            detail.optimization = new
            result.state.details[new.id] = detail
            result.touched.insert(.details)
        }
        guard var current = result.state.overview else { return }
        let spentDelta = new.spentUSD - (old?.spentUSD ?? 0)
        let valueDelta = (new.valuePerYearUSD ?? 0) - (old?.valuePerYearUSD ?? 0)
        let winsDelta = new.winsCount - (old?.winsCount ?? 0)
        current.spentUSD += spentDelta
        if valueDelta != 0 {
            current.valueLockedPerYearUSD += valueDelta
            if var last = current.series.last, last.win == nil {
                last.valuePerYearUSD = current.valueLockedPerYearUSD
                current.series.removeLast()
                if valueDelta > 0 {
                    let move = result.state.pendingWins.removeValue(forKey: new.id)
                    let date = move?.at ?? max(last.date, Date())
                    current.series.append(ValuePoint(
                        id: "win-\(new.id)-\(new.winsCount)", date: date, valuePerYearUSD: current.valueLockedPerYearUSD,
                        win: move?.title ?? "Verified win", optimizationTitle: new.title))
                    last.date = max(last.date, date)
                }
                current.series.append(last)
            }
        }
        if winsDelta > 0 { current.winsCount += winsDelta }
        result.state.overview = current
        result.touched.insert(.overview)
    }
}
