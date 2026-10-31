import Foundation
import RatchetAPI
import Testing
@testable import RatchetKit

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// A tiny HTTP/1.1 server on a real unix socket, so the transport is tested end to end with the same system calls the
/// app makes. `handler` gets the request head ("GET /v1/x HTTP/1.1\r\nHost: …") and body and returns the pieces of the
/// response, written one by one with a short pause between them.
final class TestSocketServer: @unchecked Sendable {
    typealias Handler = @Sendable (_ head: String, _ body: [UInt8]) -> [[UInt8]]

    let path: String
    private let listener: Int32
    private let handler: Handler
    private let lock = NSLock()
    private var heads: [String] = []
    private var stopped = false

    init(handler: @escaping Handler) throws {
        // A client that hangs up early must not kill the test process.
        signal(SIGPIPE, SIG_IGN)
        path = "/tmp/rt-" + String(UUID().uuidString.prefix(8)) + ".sock"
        self.handler = handler
        #if os(Linux)
        listener = socket(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0)
        #else
        listener = socket(AF_UNIX, SOCK_STREAM, 0)
        #endif
        guard listener >= 0 else { throw UnixSocketError.io(operation: "socket", code: errno) }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        #if os(macOS)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        #endif
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        let bytes = Array(path.utf8CString)
        withUnsafeMutablePointer(to: &address.sun_path) { pointer in
            pointer.withMemoryRebound(to: CChar.self, capacity: capacity) { destination in
                for (index, byte) in bytes.enumerated() { destination[index] = byte }
            }
        }
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(listener, 16) == 0 else { throw UnixSocketError.io(operation: "bind", code: errno) }
        let descriptor = listener
        // The accepting thread owns the listening descriptor and closes it when it ends, so the number cannot be reused by
        // another server while this thread still waits on it.
        Thread.detachNewThread { [self] in
            while true {
                let connection = accept(descriptor, nil, nil)
                if connection < 0 || isStopped {
                    if connection >= 0 { close(connection) }
                    close(descriptor)
                    return
                }
                Thread.detachNewThread { [self] in serve(connection) }
            }
        }
    }

    /// The request heads received so far.
    var requestHeads: [String] {
        lock.withLock { heads }
    }

    private var isStopped: Bool {
        lock.withLock { stopped }
    }

    func stop() {
        lock.withLock { stopped = true }
        // A last connection wakes the thread that waits in accept (shutdown does that on Linux only).
        _ = UnixSocket.canConnect(path: path)
        unlink(path)
    }

    private func serve(_ connection: Int32) {
        defer { close(connection) }
        var received: [UInt8] = []
        var chunk = [UInt8](repeating: 0, count: 4096)
        var headEnd: Int?
        while headEnd == nil {
            let count = read(connection, &chunk, chunk.count)
            if count <= 0 { return }
            received += chunk[0..<count]
            headEnd = TestSocketServer.find(received)
        }
        guard let headEnd else { return }
        let head = String(decoding: received[0..<headEnd], as: UTF8.self)
        var body = Array(received[(headEnd + 4)...])
        let length = head.components(separatedBy: "\r\n")
            .first { $0.lowercased().hasPrefix("content-length:") }
            .flatMap { Int($0.dropFirst("content-length:".count).trimmingCharacters(in: .whitespaces)) } ?? 0
        while body.count < length {
            let count = read(connection, &chunk, chunk.count)
            if count <= 0 { return }
            body += chunk[0..<count]
        }
        lock.withLock { heads.append(head) }
        for piece in handler(head, body) {
            var offset = 0
            while offset < piece.count {
                let written = piece.withUnsafeBufferPointer { send(connection, $0.baseAddress.map { $0 + offset }, piece.count - offset, 0) }
                if written <= 0 { return }
                offset += written
            }
            usleep(20_000)
        }
    }

    private static func find(_ bytes: [UInt8]) -> Int? {
        guard bytes.count >= 4 else { return nil }
        for index in 0...(bytes.count - 4) where bytes[index] == 13 && bytes[index + 1] == 10 && bytes[index + 2] == 13 && bytes[index + 3] == 10 {
            return index
        }
        return nil
    }

    // MARK: Responses

    static func response(status: String = "200 OK", type: String = "application/json", body: String) -> [[UInt8]] {
        [Array("HTTP/1.1 \(status)\r\nContent-Type: \(type)\r\nContent-Length: \(body.utf8.count)\r\n\r\n".utf8) + Array(body.utf8)]
    }

    /// The head and then the body in `parts`, each as its own chunk, so they arrive in separate reads.
    static func chunked(type: String = "application/json", parts: [String]) -> [[UInt8]] {
        var pieces = [Array("HTTP/1.1 200 OK\r\nContent-Type: \(type)\r\nTransfer-Encoding: chunked\r\n\r\n".utf8)]
        for part in parts { pieces.append(Array("\(String(part.utf8.count, radix: 16))\r\n\(part)\r\n".utf8)) }
        pieces.append(Array("0\r\n\r\n".utf8))
        return pieces
    }
}

@Suite("Unix socket transport")
struct UnixSocketTransportTests {
    private let now = Date(timeIntervalSince1970: 1_793_452_800)

    private func client(_ server: TestSocketServer, token: String? = "secret") -> DaemonRatchetClient {
        DaemonRatchetClient(
            transport: UnixSocketTransport(socketPath: server.path, token: { token }),
            fallback: MockRatchetClient(now: now, live: false))
    }

    @Test func requestsCarryTheTokenAndAnswersComeBackWhole() async throws {
        let server = try TestSocketServer { head, _ in
            head.hasPrefix("GET /v1/templates ") ? TestSocketServer.response(body: Fixtures.templates) : TestSocketServer.response(status: "404 Not Found", body: #"{"code":"not_found","message":"nope"}"#)
        }
        defer { server.stop() }
        let templates = try await client(server).templates()
        #expect(templates.count > 3)
        let head = try #require(server.requestHeads.first)
        #expect(head.contains("Authorization: Bearer secret"))
        #expect(head.contains("Host: localhost\r\n"))
        #expect(head.contains("Connection: close\r\n"))
    }

    @Test func chunkedAndUntilCloseBodiesAreAssembled() async throws {
        let list = Fixtures.optimizations
        let half = list.index(list.startIndex, offsetBy: list.count / 2)
        let server = try TestSocketServer { head, _ in
            if head.hasPrefix("GET /v1/optimizations ") {
                return TestSocketServer.chunked(parts: [String(list[..<half]), String(list[half...])])
            }
            if head.hasPrefix("GET /v1/optimizations/opt_demo_search ") {
                // no length and no chunking: the body ends when the server closes
                return [Array("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n".utf8), Array(Fixtures.detailSearch.utf8)]
            }
            return TestSocketServer.response(status: "404 Not Found", body: #"{"code":"not_found","message":"nope"}"#)
        }
        defer { server.stop() }
        let optimizations = try await client(server).optimizations()
        #expect(optimizations.count == 4)
        #expect(optimizations.first?.startedAt != nil)  // the detail arrived too
    }

    @Test func requestBodiesAndNoContentAnswers() async throws {
        let server = try TestSocketServer { head, body in
            if head.hasPrefix("POST /v1/agents/a1/messages ") {
                return String(decoding: body, as: UTF8.self).contains("hello agent")
                    ? [Array("HTTP/1.1 204 No Content\r\n\r\n".utf8)]
                    : TestSocketServer.response(status: "400 Bad Request", body: #"{"code":"invalid_request","message":"no text"}"#)
            }
            return TestSocketServer.response(status: "404 Not Found", body: #"{"code":"not_found","message":"nope"}"#)
        }
        defer { server.stop() }
        try await client(server).messageAgent(id: "a1", text: "hello agent")
        let head = try #require(server.requestHeads.first)
        #expect(head.contains("Content-Type: application/json"))
        #expect(head.lowercased().contains("content-length: "))
    }

    @Test func errorsKeepTheirMeaning() async throws {
        let server = try TestSocketServer { _, _ in
            TestSocketServer.response(status: "404 Not Found", body: #"{"code":"not_found","message":"optimization x not found"}"#)
        }
        defer { server.stop() }
        await #expect(throws: RatchetClientError.notFound("optimization x not found")) {
            _ = try await client(server).optimization(id: "x")
        }
    }

    @Test func aStreamArrivesPieceByPiece() async throws {
        let server = try TestSocketServer { _, _ in
            TestSocketServer.chunked(type: "text/event-stream", parts: [
                "event: hello\ndata: {\"type\":\"hello\",\"lastEventId\":0,\"serverTime\":1,\"version\":\"0\"}\n\n",
                ": keep-alive\n\n",
                "event: spend\nid: 1\ndata: {\"type\":\"spend\",\"optimizationId\":\"o1\",\"totalUsd\":2}\n\n",
            ])
        }
        defer { server.stop() }
        let api = Client(
            serverURL: URL(string: "http://localhost") ?? URL(fileURLWithPath: "/"),
            transport: UnixSocketTransport(socketPath: server.path, token: { "secret" }), middlewares: [APIErrorMiddleware()])
        let body = try await api.streamEvents().ok.body.textEventStream
        var parser = SSEParser()
        var messages: [SSEMessage] = []
        var reads = 0
        for try await chunk in body {
            reads += 1
            messages += parser.feed(Array(chunk))
        }
        #expect(messages.map(\.event) == ["hello", "spend"])
        #expect(messages.last?.id == "1")
        #expect(reads >= 2)
    }

    @Test func aDaemonThatIsNotThereIsReportedPlainly() async {
        let transport = UnixSocketTransport(socketPath: "/tmp/rt-nobody-home.sock", token: { nil })
        let client = DaemonRatchetClient(transport: transport, fallback: MockRatchetClient(now: now, live: false))
        do {
            _ = try await client.optimizations()
            Issue.record("expected a failure")
        } catch let error as RatchetClientError {
            guard case .unavailable(let why) = error else { Issue.record("wrong error \(error)"); return }
            #expect(why.contains("/tmp/rt-nobody-home.sock"))
        } catch {
            Issue.record("wrong error \(error)")
        }
        #expect(!UnixSocket.canConnect(path: "/tmp/rt-nobody-home.sock"))
    }

    @Test func aSocketCanBeProbed() throws {
        let server = try TestSocketServer { _, _ in [] }
        #expect(UnixSocket.canConnect(path: server.path))
        server.stop()
        #expect(!UnixSocket.canConnect(path: server.path))
        #expect(throws: UnixSocketError.pathTooLong(String(repeating: "x", count: 300))) {
            _ = try UnixSocket.connect(path: String(repeating: "x", count: 300))
        }
    }

    @Test func theLocationFollowsTheEnvironment() {
        let home = DaemonLocation.standard(environment: ["RATCHET_HOME": "/tmp/rh"])
        #expect(home.socketPath == "/tmp/rh/ratchetd.sock")
        #expect(home.tokenPath == "/tmp/rh/ratchetd.token")
        let socket = DaemonLocation.standard(environment: ["RATCHET_SOCKET": "/run/x/d.sock"])
        #expect(socket.socketPath == "/run/x/d.sock")
        #expect(socket.tokenPath == "/run/x/ratchetd.token")
        let standard = DaemonLocation.standard(environment: [:])
        #expect(standard.socketPath.hasSuffix("/Library/Application Support/Ratchet/ratchetd.sock"))
    }

    @Test func theTokenIsReadFromItsFileEachTime() throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: "rt-token-" + String(UUID().uuidString.prefix(8)))
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let location = DaemonLocation.standard(environment: ["RATCHET_HOME": directory.path])
        #expect(location.readToken() == nil)
        try "first\n".write(to: URL(fileURLWithPath: location.tokenPath), atomically: true, encoding: .utf8)
        #expect(location.readToken() == "first")
        try "second".write(to: URL(fileURLWithPath: location.tokenPath), atomically: true, encoding: .utf8)
        #expect(location.readToken() == "second")
    }
}
