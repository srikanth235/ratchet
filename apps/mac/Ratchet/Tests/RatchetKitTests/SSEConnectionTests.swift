import Foundation
import OpenAPIRuntime
import Testing
@testable import RatchetKit

@Suite("SSE connection")
struct SSEConnectionTests {
    private struct Broken: Error, LocalizedError {
        var errorDescription: String? { "connection reset" }
    }

    /// What the tests observe: every item the handler saw, the `Last-Event-ID` of every open, every pause.
    private actor Journal {
        var items: [SSEItem] = []
        var opened: [String?] = []
        var pauses: [Duration] = []
        private var opens = 0

        func add(_ item: SSEItem) { items.append(item) }
        func pause(_ duration: Duration) { pauses.append(duration) }
        func open(_ id: String?) -> Int {
            opened.append(id)
            opens += 1
            return opens
        }

        var messages: [SSEMessage] {
            items.flatMap { item -> [SSEMessage] in
                if case .messages(let list) = item { return list }
                return []
            }
        }
    }

    private func body(_ chunks: [String], failing: Bool = false) -> HTTPBody {
        let stream = AsyncThrowingStream<ArraySlice<UInt8>, any Error> { continuation in
            for chunk in chunks { continuation.yield(ArraySlice(Array(chunk.utf8))) }
            if failing { continuation.finish(throwing: Broken()) } else { continuation.finish() }
        }
        return HTTPBody(stream, length: .unknown)
    }

    private func policy(_ journal: Journal, initial: Duration = .milliseconds(500), max: Duration = .seconds(10), failures: Int? = nil)
        -> SSEConnection.Policy
    {
        SSEConnection.Policy(initialPause: initial, maxPause: max, maxFailures: failures, sleep: { await journal.pause($0) })
    }

    private func record(_ journal: Journal) -> @Sendable (SSEItem, inout String?) async -> Void {
        { item, _ in await journal.add(item) }
    }

    // MARK: Parsing and resuming

    @Test func messagesSplitAcrossReadsArriveWhole() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal)) { id in
            let number = await journal.open(id)
            if number > 1 { throw CancellationError() }
            return self.body(["id: 1\nda", "ta: {\"a\":1}\n", "\nid: 2\ndata: two\n\n"])
        }
        await connection.run(handle: record(journal))
        let messages = await journal.messages
        #expect(messages.map(\.id) == ["1", "2"])
        #expect(messages.map(\.data) == ["{\"a\":1}", "two"])
    }

    @Test func aReconnectResumesFromTheLastID() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal)) { id in
            let number = await journal.open(id)
            switch number {
            case 1: return self.body(["id: 1\ndata: a\n\nid: 7\ndata: b\n\n"])
            case 2: return self.body(["id: 8\ndata: c\n\n"])
            default: throw CancellationError()
            }
        }
        await connection.run(handle: record(journal))
        #expect(await journal.opened == [nil, "7", "8"])
        let items = await journal.items
        #expect(items.contains(.connected(isReconnect: false, resumedFrom: nil)))
        #expect(items.contains(.connected(isReconnect: true, resumedFrom: "7")))
        #expect(await journal.messages.map(\.data) == ["a", "b", "c"])
    }

    @Test func aGivenPositionIsUsedForTheFirstConnection() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal)) { id in
            _ = await journal.open(id)
            throw CancellationError()
        }
        await connection.run(from: "41", handle: record(journal))
        #expect(await journal.opened == ["41"])
    }

    @Test func theHandlerCanForgetThePositionWhenTheServerSaysItIsGone() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal)) { id in
            let number = await journal.open(id)
            if number == 1 { return self.body(["id: 5\ndata: resync\n\n"]) }
            throw CancellationError()
        }
        await connection.run { item, position in
            await journal.add(item)
            if case .messages(let list) = item, list.contains(where: { $0.data == "resync" }) { position = nil }
        }
        #expect(await journal.opened == [nil, nil])
    }

    // MARK: Failing and backing off

    @Test func pausesGrowAndStopAtTheCap() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal, max: .seconds(3))) { id in
            let number = await journal.open(id)
            if number > 5 { throw CancellationError() }
            throw Broken()
        }
        await connection.run(handle: record(journal))
        #expect(await journal.pauses == [.milliseconds(500), .seconds(1), .seconds(2), .seconds(3), .seconds(3)])
        let attempts = await journal.items.compactMap { item -> Int? in
            if case .disconnected(_, let attempt) = item { return attempt }
            return nil
        }
        #expect(attempts == [1, 2, 3, 4, 5])
    }

    @Test func aGoodConnectionResetsThePauseAndTheFailureCount() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal)) { id in
            let number = await journal.open(id)
            switch number {
            case 1, 2: throw Broken()
            case 3: return self.body(["id: 1\ndata: ok\n\n"])
            case 4: throw Broken()
            default: throw CancellationError()
            }
        }
        await connection.run(handle: record(journal))
        // Two failures (0.5 s, 1 s), a good connection that ended cleanly (back to 0.5 s), then one more failure.
        #expect(await journal.pauses == [.milliseconds(500), .seconds(1), .milliseconds(500), .seconds(1)])
        let attempts = await journal.items.compactMap { item -> Int? in
            if case .disconnected(_, let attempt) = item { return attempt }
            return nil
        }
        #expect(attempts == [1, 2, 0, 1])
    }

    @Test func aStreamThatBreaksMidwayIsReportedAndResumed() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal)) { id in
            let number = await journal.open(id)
            if number == 1 { return self.body(["id: 3\ndata: a\n\n"], failing: true) }
            throw CancellationError()
        }
        await connection.run(handle: record(journal))
        #expect(await journal.opened == [nil, "3"])
        let items = await journal.items
        #expect(items.contains(.disconnected(error: "connection reset", attempt: 1)))
    }

    @Test func itGivesUpAfterTheFailureLimit() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal, failures: 3)) { id in
            _ = await journal.open(id)
            throw Broken()
        }
        await connection.run(handle: record(journal))
        #expect(await journal.opened.count == 3)
        #expect(await journal.pauses.count == 2)
    }

    // MARK: Cancelling

    @Test func cancellingTheTaskEndsTheLoop() async {
        let journal = Journal()
        let connection = SSEConnection(policy: policy(journal)) { id in
            _ = await journal.open(id)
            try await Task.sleep(for: .seconds(60))
            return self.body([])
        }
        let task = Task { await connection.run(handle: self.record(journal)) }
        while await journal.opened.isEmpty { await Task.yield() }
        task.cancel()
        await task.value
        #expect(await journal.opened.count == 1)
    }
}
