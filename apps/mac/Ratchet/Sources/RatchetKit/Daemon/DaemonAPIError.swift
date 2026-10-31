import Foundation
import HTTPTypes
import OpenAPIRuntime

/// An error answer from `ratchetd`: the status and the `ApiError` body (`code`, one plain sentence, field problems).
public struct DaemonAPIError: Error, Equatable, Sendable, LocalizedError {
    public var status: Int
    /// `unauthorized`, `not_found`, `invalid_request`, `conflict`, `not_ready`, `unsupported`, `upstream`, `internal`.
    public var code: String
    public var message: String
    /// "path: problem" for each field of an `invalid_request`.
    public var fields: [String]

    public init(status: Int, code: String, message: String, fields: [String] = []) {
        self.status = status
        self.code = code
        self.message = message
        self.fields = fields
    }

    /// The engine does not do this yet (the provisional groups). The app shows sample data instead.
    public var isUnsupported: Bool { status == 501 || code == "unsupported" }

    public var errorDescription: String? {
        fields.isEmpty ? message : message + " (" + fields.joined(separator: "; ") + ")"
    }

    /// The same failure in the words of the `RatchetClient` protocol.
    public var clientError: RatchetClientError {
        switch code {
        case "not_found": .notFound(message)
        case "invalid_request", "conflict", "not_ready": .invalid(errorDescription ?? message)
        default: .unavailable(errorDescription ?? message)
        }
    }

    /// Reads an `ApiError` body. A body that is not one still gives a useful error from the status.
    static func parse(status: Int, body: [UInt8]) -> DaemonAPIError {
        struct Body: Decodable {
            struct Field: Decodable {
                var path: String?
                var message: String?
            }
            var code: String?
            var message: String?
            var fields: [Field]?
        }
        if let decoded = try? JSONDecoder().decode(Body.self, from: Data(body)) {
            let fields = (decoded.fields ?? []).map { ($0.path.map { $0 + ": " } ?? "") + ($0.message ?? "") }
            return DaemonAPIError(
                status: status, code: decoded.code ?? Self.code(forStatus: status),
                message: decoded.message ?? "ratchetd answered \(status)", fields: fields)
        }
        return DaemonAPIError(status: status, code: Self.code(forStatus: status), message: "ratchetd answered \(status)")
    }

    private static func code(forStatus status: Int) -> String {
        switch status {
        case 400: "invalid_request"
        case 401: "unauthorized"
        case 404: "not_found"
        case 409: "conflict"
        case 501: "unsupported"
        case 502: "upstream"
        default: "internal"
        }
    }
}

/// Turns every error answer (status 400 and up) into a `DaemonAPIError`, so the callers get one typed error instead of
/// a generated "undocumented response" for each operation.
struct APIErrorMiddleware: ClientMiddleware {
    func intercept(
        _ request: HTTPRequest,
        body: HTTPBody?,
        baseURL: URL,
        operationID: String,
        next: @Sendable (HTTPRequest, HTTPBody?, URL) async throws -> (HTTPResponse, HTTPBody?)
    ) async throws -> (HTTPResponse, HTTPBody?) {
        let (response, responseBody) = try await next(request, body, baseURL)
        guard response.status.code >= 400 else { return (response, responseBody) }
        var bytes: [UInt8] = []
        if let responseBody {
            for try await chunk in responseBody {
                bytes.append(contentsOf: chunk)
                if bytes.count > 1_000_000 { break }
            }
        }
        throw DaemonAPIError.parse(status: Int(response.status.code), body: bytes)
    }
}

/// Finds a `DaemonAPIError` inside whatever the generated client threw (it wraps errors in `ClientError`).
func daemonError(in error: any Error) -> DaemonAPIError? {
    if let error = error as? DaemonAPIError { return error }
    if let wrapped = error as? ClientError { return daemonError(in: wrapped.underlyingError) }
    return nil
}
