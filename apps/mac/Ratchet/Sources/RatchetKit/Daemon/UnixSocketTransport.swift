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
public struct UnixSocketTransport: ClientTransport {
    public let socketPath: String
    /// Called for every request, so a daemon that restarted (and wrote a new token) is picked up without relaunching.
    private let token: @Sendable () -> String?

    public init(socketPath: String, token: @escaping @Sendable () -> String?) {
        self.socketPath = socketPath
        self.token = token
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

        let socket = try UnixSocket.connect(path: socketPath)
        let (stream, continuation) = AsyncThrowingStream<ArraySlice<UInt8>, any Error>.makeStream()
        continuation.onTermination = { _ in socket.shutdown() }
        let head: HTTP1ResponseHead = try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (headContinuation: CheckedContinuation<HTTP1ResponseHead, any Error>) in
                Thread.detachNewThread {
                    pumpResponse(socket: socket, request: bytes, head: headContinuation, body: continuation)
                }
            }
        } onCancel: {
            socket.shutdown()
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

/// Runs on its own thread: writes the request, then reads until the response is complete. Resumes `head` once the head
/// has arrived (or the connection failed before that) and feeds the body into `body`.
private func pumpResponse(
    socket: UnixSocket,
    request: [UInt8],
    head: CheckedContinuation<HTTP1ResponseHead, any Error>,
    body: AsyncThrowingStream<ArraySlice<UInt8>, any Error>.Continuation
) {
    defer { socket.close() }
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
