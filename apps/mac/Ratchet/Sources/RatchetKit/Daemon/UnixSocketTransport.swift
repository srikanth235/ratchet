import Foundation
import HTTPTypes
import OpenAPIRuntime

/// The transport of the generated OpenAPI client: HTTP/1.1 over the unix socket of `ratchetd`, one connection per
/// request (`Connection: close`), the bearer token added to every request.
///
/// Why not Network.framework or swift-nio: this is about 250 lines of plain POSIX that behave the same on macOS and
/// Linux, so the unit tests and a real `ratchetd` can exercise it without a Mac, and the app gains no dependency.
/// A blocking read lives on its own thread per connection; the async side only sees a stream of byte chunks, so a
/// slow server or an event stream never holds a cooperative thread.
///
/// Each open request holds one thread, so the number open at once is capped (`maxConnections`, the event stream being one of
/// them): a request over the cap waits for a slot instead of starting another thread, and a refresh that asks for many things
/// at once cannot flood the daemon. The connect, which can block when the daemon's queue is full, happens on that thread too.
public struct UnixSocketTransport: ClientTransport {
    public let socketPath: String
    /// Called for every request, so a daemon that restarted (and wrote a new token) is picked up without relaunching.
    private let token: @Sendable () -> String?
    private let gate: ConnectionGate

    public init(socketPath: String, token: @escaping @Sendable () -> String?, maxConnections: Int = 8) {
        self.socketPath = socketPath
        self.token = token
        self.gate = ConnectionGate(limit: maxConnections)
    }

    public func send(
        _ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String
    ) async throws -> (HTTPResponse, HTTPBody?) {
        var payload: [UInt8] = []
        if let body {
            for try await chunk in body { payload.append(contentsOf: chunk) }
        }
        guard let target = request.path else { throw UnixSocketTransportError.noPath }

        var fields: [HTTP1Field] = [
            HTTP1Field(name: "Host", value: "localhost"),
            HTTP1Field(name: "Connection", value: "close"),
        ]
        var hasAuthorization = false
        for field in request.headerFields {
            if field.name == .authorization { hasAuthorization = true }
            fields.append(HTTP1Field(name: field.name.rawName, value: field.value))
        }
        if !hasAuthorization, let token = token() {
            fields.append(HTTP1Field(name: "Authorization", value: "Bearer " + token))
        }
        if !payload.isEmpty || request.method == .post || request.method == .put || request.method == .patch {
            fields.append(HTTP1Field(name: "Content-Length", value: String(payload.count)))
        }
        let bytes = HTTP1Request.encode(method: request.method.rawValue, target: target, fields: fields, body: payload)

        try await gate.acquire()
        let (stream, continuation) = AsyncThrowingStream<ArraySlice<UInt8>, any Error>.makeStream()
        let box = SocketBox()
        continuation.onTermination = { _ in box.shutdown() }
        let path = socketPath
        let gate = self.gate
        // The thread frees the slot when the response is over (the end of the body), or when the connect or the head failed.
        let head: HTTP1ResponseHead = try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (headContinuation: CheckedContinuation<HTTP1ResponseHead, any Error>) in
                Thread.detachNewThread {
                    pumpResponse(path: path, box: box, request: bytes, head: headContinuation, body: continuation)
                    Task { await gate.release() }
                }
            }
        } onCancel: {
            box.shutdown()
        }

        var response = HTTPResponse(status: HTTPResponse.Status(code: head.status))
        for field in head.fields {
            if let name = HTTPField.Name(field.name) {
                response.headerFields.append(HTTPField(name: name, value: field.value))
            }
        }
        let length = head.value(for: "Content-Length").flatMap { Int64($0) }
        if head.status == 204 || head.status == 304 || length == 0 {
            return (response, nil)
        }
        return (response, HTTPBody(stream, length: length.map { .known($0) } ?? .unknown))
    }
}

enum UnixSocketTransportError: Error, LocalizedError {
    case noPath

    var errorDescription: String? { "The request has no path." }
}

/// The socket of one request, once it is connected, so a cancelled request can wake the thread that is reading it. Cancelling
/// before the socket exists makes the thread give up as soon as it has connected.
private final class SocketBox: @unchecked Sendable {
    private let lock = NSLock()
    private var socket: UnixSocket?
    private var cancelled = false

    /// False when the request was cancelled already: the caller closes the socket.
    func store(_ socket: UnixSocket) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if cancelled { return false }
        self.socket = socket
        return true
    }

    func shutdown() {
        lock.lock()
        cancelled = true
        let current = socket
        lock.unlock()
        current?.shutdown()
    }
}

/// Runs on its own thread: connects, writes the request, then reads until the response is complete. Resumes `head` once the
/// head has arrived (or the connection failed before that) and feeds the body into `body`.
private func pumpResponse(
    path: String,
    box: SocketBox,
    request: [UInt8],
    head: CheckedContinuation<HTTP1ResponseHead, any Error>,
    body: AsyncThrowingStream<ArraySlice<UInt8>, any Error>.Continuation
) {
    let socket: UnixSocket
    do {
        socket = try UnixSocket.connect(path: path)
    } catch {
        head.resume(throwing: error)
        body.finish(throwing: error)
        return
    }
    defer { socket.close() }
    guard box.store(socket) else {
        head.resume(throwing: CancellationError())
        body.finish()
        return
    }
    var parser = HTTP1ResponseParser()
    var headSent = false
    do {
        try socket.writeAll(request)
        while true {
            let bytes = try socket.read()
            let events: [HTTP1ResponseParser.Event]
            if bytes.isEmpty {
                events = try parser.finish()
            } else {
                events = try parser.feed(bytes)
            }
            for event in events {
                switch event {
                case .head(let value):
                    headSent = true
                    head.resume(returning: value)
                case .body(let chunk):
                    body.yield(ArraySlice(chunk))
                case .end:
                    body.finish()
                }
            }
            if bytes.isEmpty || parser.isDone { break }
        }
    } catch {
        if headSent {
            body.finish(throwing: error)
        } else {
            head.resume(throwing: error)
        }
    }
}

/// Lets at most `limit` requests hold a connection at once; the others wait their turn, first come first served. A waiting
/// request that is cancelled leaves the queue.
actor ConnectionGate {
    private let limit: Int
    private var inUse = 0
    private var waiters: [(id: UUID, continuation: CheckedContinuation<Void, any Error>)] = []

    init(limit: Int) {
        self.limit = max(limit, 1)
    }

    var openCount: Int { inUse }
    var waitingCount: Int { waiters.count }

    func acquire() async throws {
        if inUse < limit {
            inUse += 1
            return
        }
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
                waiters.append((id, continuation))
                // Cancelled before it was in the queue: the handler had nothing to remove.
                if Task.isCancelled { leave(id) }
            }
        } onCancel: {
            Task { await self.leave(id) }
        }
    }

    func release() {
        if waiters.isEmpty {
            inUse = max(inUse - 1, 0)
        } else {
            // The slot passes to the first in line; the count stays.
            waiters.removeFirst().continuation.resume()
        }
    }

    private func leave(_ id: UUID) {
        guard let at = waiters.firstIndex(where: { $0.id == id }) else { return }
        waiters.remove(at: at).continuation.resume(throwing: CancellationError())
    }
}
