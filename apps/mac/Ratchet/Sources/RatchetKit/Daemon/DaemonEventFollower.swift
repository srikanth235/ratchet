import Foundation
import RatchetAPI

/// Follows `GET /v1/events` and turns what the daemon announces into the full models the app applies.
///
/// The announcements carry ids only, so each one is a refetch of the thing that changed. Announcements that need the same
/// refetch are merged: everything that arrives within `window` of the first one is answered together, each thing fetched
/// once (a burst of log lines is one detail fetch, not one per line). The connection itself is an `SSEConnection`.
///
/// A refetch that fails, or a message that is not JSON, is not dropped silently: the follower asks the store to reload
/// everything (`.resync`) once the batch is done, and reports the connection's state as `.streamStateChanged`.
struct DaemonEventFollower: Sendable {
    let client: DaemonRatchetClient
    let window: Duration

    func stream() -> AsyncStream<RatchetEvent> {
        let (stream, continuation) = AsyncStream<RatchetEvent>.makeStream(bufferingPolicy: .unbounded)
        let task = Task { await self.follow(into: continuation) }
        continuation.onTermination = { _ in task.cancel() }
        return stream
    }

    // MARK: Following

    private func follow(into continuation: AsyncStream<RatchetEvent>.Continuation) async {
        continuation.yield(.streamStateChanged(.connecting))
        let api = client.api
        let connection = SSEConnection { lastID in
            let response = try await api.streamEvents(headers: .init(lastEventID: lastID.flatMap { Int64($0) }))
            return try response.ok.body.textEventStream
        }
        let batches = EventBatcher(window: window) { tasks in
            await self.deliver(tasks, into: continuation)
        }
        await connection.run { item, position in
            switch item {
            case .connected(let isReconnect, let resumedFrom):
                continuation.yield(.streamStateChanged(.live))
                // After a dropped connection with nothing to resume from, whatever happened meanwhile is lost.
                if isReconnect && resumedFrom == nil { continuation.yield(.resync) }
            case .messages(let messages):
                var tasks: [FollowTask] = []
                var unreadable = false
                for message in messages {
                    guard let event = DaemonEvent.decode(message) else {
                        unreadable = true
                        continue
                    }
                    switch event {
                    case .hello(let newest, _):
                        if position == nil { position = String(newest) }
                    case .resync:
                        position = nil
                        continuation.yield(.resync)
                    case .runActive(_, let ids, let awake):
                        continuation.yield(.powerChanged(PowerState(keepingAwake: awake, localOptimizationIDs: ids)))
                    default:
                        tasks += FollowTask.tasks(for: event)
                    }
                }
                if unreadable { continuation.yield(.resync) }
                if !tasks.isEmpty { await batches.add(tasks) }
            case .disconnected(let error, let attempt):
                continuation.yield(.streamStateChanged(.reconnecting(attempt: attempt, reason: error)))
            }
        }
        await batches.finish()
        continuation.finish()
    }

    // MARK: Answering

    /// Fetches what the merged announcements asked for and yields the models. The detail of an optimization is read once
    /// per batch however many tasks need it.
    private func deliver(_ tasks: [FollowTask], into continuation: AsyncStream<RatchetEvent>.Continuation) async {
        var details: [String: VM.OptimizationDetail] = [:]
        var failed = false
        for task in FollowTask.ordered(tasks) {
            let answered = await run(task, details: &details, into: continuation)
            if !answered { failed = true }
        }
        // Something could not be fetched: the next batch may repair it, but a reload makes sure.
        if failed { continuation.yield(.resync) }
    }

    private func loadDetail(_ id: String, _ cache: inout [String: VM.OptimizationDetail]) async -> VM.OptimizationDetail? {
        if let kept = cache[id] { return kept }
        guard let fetched = try? await client.optimizationDetail(id) else { return nil }
        cache[id] = fetched
        return fetched
    }

    /// True when it was answered (an answer of "nothing to say" counts).
    private func run(
        _ task: FollowTask, details: inout [String: VM.OptimizationDetail], into continuation: AsyncStream<RatchetEvent>.Continuation
    ) async -> Bool {
        switch task {
        case .moves(let id):
            guard let detail = await loadDetail(id, &details) else { return false }
            let start = DaemonMapping.optimization(detail.summary, detail: detail).startedAt
            guard let moves = try? await client.ratchetMoves(optimizationID: id, start: start) else { return false }
            for move in moves { continuation.yield(.moveAdded(optimizationID: id, move: move)) }
            return true
        case .candidate(let id, let candidateID):
            guard let list = try? await client.api.listCandidates(path: .init(id: id)).ok.body.json else { return false }
            if let changed = list.first(where: { $0.id == candidateID }) {
                continuation.yield(.candidateUpdated(optimizationID: id, candidate: DaemonMapping.candidate(changed)))
            }
            return true
        case .logAppended(let id, let agentID):
            continuation.yield(.agentLogAppended(optimizationID: id, agentID: agentID))
            return true
        case .agent(let id, let agentID):
            guard let detail = await loadDetail(id, &details) else { return false }
            if let agent = detail.agents.first(where: { $0.id == agentID }) {
                continuation.yield(.agentUpdated(optimizationID: id, agent: DaemonMapping.agent(agent)))
            }
            return true
        case .optimization(let id):
            guard let detail = await loadDetail(id, &details) else { return false }
            continuation.yield(.optimizationUpdated(DaemonMapping.optimization(detail.summary, detail: detail)))
            for agent in detail.agents {
                continuation.yield(.agentUpdated(optimizationID: id, agent: DaemonMapping.agent(agent)))
            }
            return true
        case .sandbox:
            guard let environments = try? await client.environments() else { return false }
            continuation.yield(.sandboxChanged(DaemonMapping.sandbox(environments)))
            if let receipt = environments.lastReceipt {
                continuation.yield(.receiptIssued(await client.mappedReceipt(receipt)))
            }
            return true
        case .connectors:
            guard let list = try? await client.connectors() else { return false }
            continuation.yield(.connectorsChanged(list))
            let flow = try? await client.githubSignInFlow()
            continuation.yield(.githubFlowChanged(flow))
            return true
        case .draft(let draftID, let status):
            guard status == "ready" else { return true }
            guard let draft = try? await client.api.getEnvironmentDraft(path: .init(id: draftID)).ok.body.json else { return false }
            continuation.yield(.environmentDraftReady(draftID: draftID, repoID: draft.repo))
            return true
        case .needsYou:
            guard let overview = try? await client.api.getOverview().ok.body.json else { return false }
            continuation.yield(.needsYouChanged(overview.needsYou.map(DaemonMapping.needsYou)))
            return true
        }
    }
}

/// One thing to refetch. Announcements that map to the same task are answered once.
enum FollowTask: Hashable, Sendable {
    case moves(String)
    case candidate(String, String)
    case logAppended(String, String)
    case agent(String, String)
    case optimization(String)
    case sandbox
    case connectors
    case draft(String, String)
    case needsYou

    static func tasks(for event: DaemonEvent) -> [FollowTask] {
        switch event {
        case .optimization(let id, _): [.optimization(id), .needsYou]
        case .spend(let id, _): [.optimization(id)]
        case .ratchet(let id, _, _): [.moves(id), .optimization(id)]
        case .candidate(let id, let candidateID, _): [.candidate(id, candidateID)]
        case .agentEvent(let id, let agentID): [.logAppended(id, agentID), .agent(id, agentID)]
        case .sandbox: [.sandbox]
        case .connector, .github: [.connectors, .needsYou]
        case .environmentDraft(let draftID, let status): [.draft(draftID, status), .needsYou]
        case .hello, .resync, .notice, .runActive, .unknown: []
        }
    }

    /// Unique, in the order that makes the app's state right: moves before the optimization they dated, the optimization
    /// before the overview. An agent update that the whole optimization already carries is left out.
    static func ordered(_ tasks: [FollowTask]) -> [FollowTask] {
        var unique: [FollowTask] = []
        for task in tasks where !unique.contains(task) { unique.append(task) }
        let whole = Set(unique.compactMap { task -> String? in
            if case .optimization(let id) = task { id } else { nil }
        })
        unique.removeAll { task in
            if case .agent(let id, _) = task { whole.contains(id) } else { false }
        }
        return unique.enumerated().sorted { a, b in
            a.element.rank == b.element.rank ? a.offset < b.offset : a.element.rank < b.element.rank
        }.map(\.element)
    }

    private var rank: Int {
        switch self {
        case .moves: 0
        case .candidate: 1
        case .logAppended: 2
        case .agent: 3
        case .optimization: 4
        case .sandbox: 5
        case .connectors: 6
        case .draft: 7
        case .needsYou: 8
        }
    }
}

/// Collects tasks for `window` after the first one arrives, then hands them over together. Lives for one connection run.
actor EventBatcher {
    private var pending: [FollowTask] = []
    private var timer: Task<Void, Never>?
    private let window: Duration
    private let flush: @Sendable ([FollowTask]) async -> Void

    init(window: Duration, flush: @escaping @Sendable ([FollowTask]) async -> Void) {
        self.window = window
        self.flush = flush
    }

    func add(_ tasks: [FollowTask]) {
        pending += tasks
        guard timer == nil else { return }
        timer = Task { [window] in
            try? await Task.sleep(for: window)
            await self.drain()
        }
    }

    private func drain() async {
        let batch = pending
        pending = []
        timer = nil
        guard !batch.isEmpty else { return }
        await flush(batch)
    }

    /// The connection run is over (its task was cancelled): nobody is waiting for what is pending.
    func finish() {
        timer?.cancel()
        timer = nil
        pending = []
    }
}
