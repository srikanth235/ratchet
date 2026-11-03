import Foundation
import HTTPTypes
import Testing
@testable import RatchetKit

/// The pieces the daemon client is made of, each on its own.
@Suite("Daemon client parts")
struct DaemonPartsTests {
    // MARK: Bounded

    private actor Probe {
        private(set) var inFlight = 0
        private(set) var mostInFlight = 0

        func enter() {
            inFlight += 1
            mostInFlight = max(mostInFlight, inFlight)
        }

        func leave() { inFlight -= 1 }
    }

    @Test func boundedMapKeepsTheOrderAndTheCap() async {
        let probe = Probe()
        let out = await Bounded.map(Array(0..<30), limit: 4) { number -> Int in
            await probe.enter()
            try? await Task.sleep(for: .milliseconds(3))
            await probe.leave()
            return number * 2
        }
        #expect(out == (0..<30).map { $0 * 2 })
        #expect(await probe.mostInFlight <= 4)
        #expect(await probe.mostInFlight >= 1)
    }

    @Test func boundedMapOfNothingIsNothing() async {
        let out = await Bounded.map([Int](), limit: 3) { $0 }
        #expect(out.isEmpty)
    }

    // MARK: FollowTask

    @Test func aRatchetEventDatesTheMoveBeforeItUpdatesTheOptimization() {
        let tasks = FollowTask.tasks(for: .ratchet(optimizationID: "o1", from: 1, to: 2))
        #expect(tasks == [.moves("o1"), .optimization("o1")])
    }

    @Test func mergedTasksAreUniqueAndOrdered() {
        let burst: [FollowTask] =
            FollowTask.tasks(for: .agentEvent(optimizationID: "o1", agentID: "a1"))
            + FollowTask.tasks(for: .agentEvent(optimizationID: "o1", agentID: "a1"))
            + FollowTask.tasks(for: .optimization(id: "o2", status: "running"))
            + FollowTask.tasks(for: .ratchet(optimizationID: "o1", from: 1, to: 2))
            + FollowTask.tasks(for: .connector(connectorID: "c"))
        let ordered = FollowTask.ordered(burst)
        // The agent update is part of the whole optimization's, so it is left out.
        #expect(ordered == [
            .moves("o1"), .logAppended("o1", "a1"), .optimization("o2"), .optimization("o1"), .connectors, .needsYou,
        ])
    }

    @Test func eventsThatSayNothingNeedNoFetch() {
        #expect(FollowTask.tasks(for: .notice(optimizationID: "o", level: "info", text: "x")).isEmpty)
        #expect(FollowTask.tasks(for: .runActive(active: true, optimizationIDs: [])).isEmpty)
        #expect(FollowTask.tasks(for: .unknown(type: "future")).isEmpty)
    }

    // MARK: EventBatcher

    private actor Flushes {
        var batches: [[FollowTask]] = []
        func add(_ batch: [FollowTask]) { batches.append(batch) }
    }

    @Test func tasksThatArriveWithinTheWindowAreAnsweredTogether() async throws {
        let flushes = Flushes()
        let batcher = EventBatcher(window: .milliseconds(40)) { await flushes.add($0) }
        await batcher.add([.optimization("o1")])
        await batcher.add([.optimization("o1"), .needsYou])
        try await Task.sleep(for: .milliseconds(300))
        let batches = await flushes.batches
        #expect(batches.count == 1)
        #expect(batches.first == [.optimization("o1"), .optimization("o1"), .needsYou])
    }

    @Test func aTaskAfterTheWindowStartsANewBatch() async throws {
        let flushes = Flushes()
        let batcher = EventBatcher(window: .milliseconds(20)) { await flushes.add($0) }
        await batcher.add([.sandbox])
        try await Task.sleep(for: .milliseconds(250))
        await batcher.add([.connectors])
        try await Task.sleep(for: .milliseconds(250))
        #expect(await flushes.batches == [[.sandbox], [.connectors]])
    }

    @Test func finishingDropsWhatIsPending() async throws {
        let flushes = Flushes()
        let batcher = EventBatcher(window: .milliseconds(30)) { await flushes.add($0) }
        await batcher.add([.sandbox])
        await batcher.finish()
        try await Task.sleep(for: .milliseconds(200))
        #expect(await flushes.batches.isEmpty)
    }

    // MARK: OptimizationRowsCache

    private actor Loads {
        var count = 0
        func next() -> Int {
            count += 1
            return count
        }
    }

    @Test func rowsRequestedTogetherShareOneLoad() async throws {
        let cache = OptimizationRowsCache(lifetime: 5)
        let loads = Loads()
        let load: @Sendable () async throws -> [OptimizationRow] = {
            _ = await loads.next()
            try await Task.sleep(for: .milliseconds(20))
            return []
        }
        async let first = cache.rows(load: load)
        async let second = cache.rows(load: load)
        _ = try await (first, second)
        #expect(await loads.count == 1)
        await cache.forget()
        _ = try await cache.rows(load: load)
        #expect(await loads.count == 2)
    }

    @Test func aFailedLoadIsNotRemembered() async throws {
        let cache = OptimizationRowsCache(lifetime: 5)
        let loads = Loads()
        let load: @Sendable () async throws -> [OptimizationRow] = {
            let number = await loads.next()
            if number == 1 { throw RatchetClientError.unavailable("down") }
            return []
        }
        await #expect(throws: RatchetClientError.self) { try await cache.rows(load: load) }
        _ = try await cache.rows(load: load)
        #expect(await loads.count == 2)
    }

    @Test func anAgentIsFoundThroughItsOptimization() async {
        let cache = OptimizationRowsCache()
        await cache.remember(agents: ["a1", "a2"], of: "o1")
        #expect(await cache.owner(ofAgent: "a2") == "o1")
        #expect(await cache.owner(ofAgent: "zz") == nil)
    }

    // MARK: The follower, against the fake daemon

    private let hello = #"event: hello\#ndata: {"type":"hello","lastEventId":0,"serverTime":1,"version":"0"}\#n\#n"#

    @Test(.timeLimit(.minutes(1))) func aBurstOfLogLinesIsOneFetchAndOneNotice() async throws {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/optimizations/opt_demo_search", 200, Fixtures.detailSearch)
        var body = hello
        for number in 1...25 {
            body += #"event: agent-event\#nid: \#(number)\#ndata: {"type":"agent-event","optimizationId":"opt_demo_search","agentId":"opt_demo_search_agt_0"}\#n\#n"#
        }
        daemon.streams([body])
        var logNotices = 0
        var updates = 0
        for await event in await DaemonRatchetClient(transport: daemon, coalescing: .milliseconds(60)).events() {
            if case .agentLogAppended = event { logNotices += 1 }
            if case .agentUpdated = event {
                updates += 1
                break
            }
        }
        #expect(logNotices == 1)
        #expect(updates == 1)
        #expect(daemon.count("GET /v1/optimizations/opt_demo_search") == 1)
    }

    @Test(.timeLimit(.minutes(1))) func aRefetchThatFailsAsksForAReload() async throws {
        let daemon = FakeDaemon()
        // No route for the optimization: the refetch is a 404.
        daemon.streams([hello + #"event: spend\#nid: 1\#ndata: {"type":"spend","optimizationId":"opt_gone","totalUsd":1}\#n\#n"#])
        var states: [StreamState] = []
        for await event in await DaemonRatchetClient(transport: daemon, coalescing: .milliseconds(20)).events() {
            if case .streamStateChanged(let state) = event { states.append(state) }
            if event == .resync { break }
        }
        #expect(states.first == StreamState.connecting)
        #expect(states.contains(.live))
    }

    @Test(.timeLimit(.minutes(1))) func aDaemonThatIsAwayIsReportedAsReconnecting() async throws {
        let daemon = FakeDaemon()
        daemon.failure = RatchetClientError.unavailable("no daemon")
        var states: [StreamState] = []
        for await event in await DaemonRatchetClient(transport: daemon, coalescing: .milliseconds(20)).events() {
            if case .streamStateChanged(let state) = event {
                states.append(state)
                if case .reconnecting = state { break }
            }
        }
        #expect(states.first == StreamState.connecting)
        guard case .reconnecting(let attempt, _)? = states.last else {
            Issue.record("never reported reconnecting")
            return
        }
        #expect(attempt == 1)
    }

    // MARK: ConnectionGate

    private actor Order {
        var events: [String] = []
        func add(_ name: String) { events.append(name) }
    }

    @Test func theGateAdmitsUpToItsLimitThenQueuesInOrder() async throws {
        let gate = ConnectionGate(limit: 2)
        try await gate.acquire()
        try await gate.acquire()
        #expect(await gate.openCount == 2)
        let order = Order()
        let first = Task {
            try await gate.acquire()
            await order.add("first")
        }
        while await gate.waitingCount < 1 { await Task.yield() }
        let second = Task {
            try await gate.acquire()
            await order.add("second")
        }
        while await gate.waitingCount < 2 { await Task.yield() }
        await gate.release()
        try await first.value
        #expect(await order.events == ["first"])
        await gate.release()
        try await second.value
        #expect(await order.events == ["first", "second"])
        // The slots were passed on, not freed.
        #expect(await gate.openCount == 2)
        await gate.release()
        await gate.release()
        #expect(await gate.openCount == 0)
    }

    @Test func aWaitingRequestThatIsCancelledLeavesTheQueue() async throws {
        let gate = ConnectionGate(limit: 1)
        try await gate.acquire()
        let waiting = Task { try await gate.acquire() }
        while await gate.waitingCount < 1 { await Task.yield() }
        waiting.cancel()
        await #expect(throws: CancellationError.self) { try await waiting.value }
        #expect(await gate.waitingCount == 0)
        await gate.release()
        #expect(await gate.openCount == 0)
    }

    // MARK: HTTP/1 parser

    @Test func manyChunksInOneReadParseToTheWholeBody() throws {
        var wire = Array("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".utf8)
        var expected: [UInt8] = []
        for number in 0..<2000 {
            let piece = Array("line \(number)\n".utf8)
            wire += Array(String(piece.count, radix: 16).utf8) + [13, 10] + piece + [13, 10]
            expected += piece
        }
        wire += Array("0\r\n\r\n".utf8)
        var parser = HTTP1ResponseParser()
        var body: [UInt8] = []
        var ended = false
        for event in try parser.feed(wire) {
            switch event {
            case .head(let head): #expect(head.status == 200)
            case .body(let chunk): body += chunk
            case .end: ended = true
            }
        }
        #expect(body == expected)
        #expect(ended && parser.isDone)
    }

    @Test func aResponseSplitAtEveryByteParsesTheSame() throws {
        let wire = Array("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".utf8)
        var parser = HTTP1ResponseParser()
        var body: [UInt8] = []
        var statuses: [Int] = []
        for byte in wire {
            for event in try parser.feed([byte]) {
                if case .head(let head) = event { statuses.append(head.status) }
                if case .body(let chunk) = event { body += chunk }
            }
        }
        #expect(statuses == [200])
        #expect(String(decoding: body, as: UTF8.self) == "hello world")
        #expect(parser.isDone)
    }
}
