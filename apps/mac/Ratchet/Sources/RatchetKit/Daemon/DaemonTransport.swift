import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

/// The wire of the daemon client: the generated OpenAPI client and the transport behind it, one place that turns a failed
/// call into the app's own errors, and the JSON calls the generated client does not hold. The generated client holds only the
/// operations listed in tools/api/openapi-generator-config.yaml; the other routes (pull request, report, repo search,
/// preflight, connector edits, detected harnesses, draft discard, advanced settings) go through the same transport and error
/// middleware as `jsonCall` and `rawCall`, so the committed client stays what the generator produces.
struct DaemonTransport: Sendable {
    let api: Client
    let transport: any ClientTransport

    init(transport: any ClientTransport) {
        self.transport = transport
        self.api = Client(
            serverURL: URL(string: "http://localhost") ?? URL(fileURLWithPath: "/"),
            transport: transport, middlewares: [APIErrorMiddleware()])
    }

    init(location: DaemonLocation) {
        self.init(transport: UnixSocketTransport(socketPath: location.socketPath, token: { location.readToken() }))
    }

    // MARK: Failures

    /// What the app sees of a failed call: the protocol's own errors, in plain words.
    func translate(_ error: any Error) -> any Error {
        if let error = daemonError(in: error) { return error.clientError }
        if let socket = underlyingSocketError(in: error) {
            return RatchetClientError.unavailable(socket.errorDescription ?? "Cannot reach ratchetd.")
        }
        if let wrapped = error as? ClientError { return RatchetClientError.unavailable(wrapped.causeDescription) }
        return error
    }

    private func underlyingSocketError(in error: any Error) -> UnixSocketError? {
        if let error = error as? UnixSocketError { return error }
        if let wrapped = error as? ClientError { return underlyingSocketError(in: wrapped.underlyingError) }
        return nil
    }

    /// Runs `real` and translates its failure.
    func calling<T: Sendable>(_ real: () async throws -> T) async throws -> T {
        do {
            return try await real()
        } catch {
            throw translate(error)
        }
    }

    // MARK: Calls outside the generated client

    func escaped(_ segment: String) -> String {
        segment.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? segment
    }

    /// One request without a body, through the error middleware; the response body on success.
    func rawCall(_ method: HTTPRequest.Method, _ path: String, operationID: String) async throws -> Data {
        let base = URL(string: "http://localhost") ?? URL(fileURLWithPath: "/")
        let request = HTTPRequest(method: method, scheme: nil, authority: nil, path: path)
        let transport = self.transport
        let (_, body) = try await APIErrorMiddleware().intercept(request, body: nil, baseURL: base, operationID: operationID) {
            request, body, url in
            try await transport.send(request, body: body, baseURL: url, operationID: operationID)
        }
        var bytes: [UInt8] = []
        if let body {
            for try await chunk in body { bytes.append(contentsOf: chunk) }
        }
        return Data(bytes)
    }

    /// One JSON request through the error middleware; the response body (empty for a 204).
    func jsonCall(_ method: HTTPRequest.Method, _ path: String, operationID: String, json: Data? = nil) async throws -> Data {
        let base = URL(string: "http://localhost") ?? URL(fileURLWithPath: "/")
        var request = HTTPRequest(method: method, scheme: nil, authority: nil, path: path)
        request.headerFields[.accept] = "application/json"
        var payload: HTTPBody?
        if let json {
            request.headerFields[.contentType] = "application/json"
            payload = HTTPBody(String(decoding: json, as: UTF8.self))
        }
        let transport = self.transport
        let (_, body) = try await APIErrorMiddleware().intercept(request, body: payload, baseURL: base, operationID: operationID) {
            request, body, url in
            try await transport.send(request, body: body, baseURL: url, operationID: operationID)
        }
        var bytes: [UInt8] = []
        if let body {
            for try await chunk in body { bytes.append(contentsOf: chunk) }
        }
        return Data(bytes)
    }

    /// Opens a server-sent event stream the generated client does not hold (a chat's): the body is read by the caller, and an
    /// error answer is turned into a `DaemonAPIError` like every other call.
    func openStream(_ path: String, lastEventID: String?, operationID: String) async throws -> HTTPBody {
        let base = URL(string: "http://localhost") ?? URL(fileURLWithPath: "/")
        var request = HTTPRequest(method: .get, scheme: nil, authority: nil, path: path)
        request.headerFields[.accept] = "text/event-stream"
        if let lastEventID, let name = HTTPField.Name("Last-Event-ID") { request.headerFields[name] = lastEventID }
        let transport = self.transport
        let (_, body) = try await APIErrorMiddleware().intercept(request, body: nil, baseURL: base, operationID: operationID) {
            request, body, url in
            try await transport.send(request, body: body, baseURL: url, operationID: operationID)
        }
        guard let body else { throw RatchetClientError.unavailable("ratchetd sent an empty stream.") }
        return body
    }


    /// A JSON object from a dictionary; `NSNull()` writes a null.
    static func jsonBody(_ object: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: object)
    }

    /// `?name=value`, with the characters that would end a value escaped.
    static func queryItem(_ name: String, _ value: String) -> String {
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "&=+#?/")
        return name + "=" + (value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value)
    }

    func decode<T: Decodable>(_ type: T.Type, _ data: Data) throws -> T {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw RatchetClientError.unavailable("ratchetd answered with something the app cannot read.")
        }
    }
}
