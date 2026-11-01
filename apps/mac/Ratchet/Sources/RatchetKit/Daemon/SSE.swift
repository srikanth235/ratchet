import Foundation

// Server-sent events from `GET /v1/events`: the wire parser, and the decoding of one message into what the daemon says.

/// One dispatched message of an event stream.
public struct SSEMessage: Sendable, Equatable {
    /// The `event:` field; "message" when the block had none.
    public var event: String
    /// The `id:` field of this block, if it had one (the daemon's `hello` has none).
    public var id: String?
    /// The `data:` lines joined with a newline.
    public var data: String
    public var retryMilliseconds: Int?

    public init(event: String = "message", id: String? = nil, data: String, retryMilliseconds: Int? = nil) {
        self.event = event
        self.id = id
        self.data = data
        self.retryMilliseconds = retryMilliseconds
    }
}

/// An incremental parser of `text/event-stream`. Feed it bytes in any split; it returns the messages that became
/// complete. Handles LF, CRLF and CR line ends (also split across two reads), comment lines (keep-alives), a leading
/// byte order mark, multi-line data, and remembers the last `id:` it saw for reconnecting with `Last-Event-ID`.
public struct SSEParser: Sendable {
    /// The id of the newest block that carried one. Persists across blocks, as the SSE specification says.
    public private(set) var lastEventID: String?

    private var line: [UInt8] = []
    private var previousWasCR = false
    private var atStart = true

    private var eventName: String?
    private var eventID: String?
    private var dataLines: [String] = []
    private var retry: Int?

    public init(lastEventID: String? = nil) {
        self.lastEventID = lastEventID
    }

    public mutating func feed(_ text: String) -> [SSEMessage] {
        feed(Array(text.utf8))
    }

    public mutating func feed(_ bytes: [UInt8]) -> [SSEMessage] {
        var messages: [SSEMessage] = []
        for byte in bytes {
            if byte == 10 && previousWasCR {
                previousWasCR = false
                continue
            }
            previousWasCR = false
            if byte == 13 || byte == 10 {
                previousWasCR = byte == 13
                if let message = endOfLine() { messages.append(message) }
            } else {
                line.append(byte)
            }
        }
        return messages
    }

    // MARK: Lines

    private mutating func endOfLine() -> SSEMessage? {
        var bytes = line
        line.removeAll(keepingCapacity: true)
        if atStart {
            atStart = false
            if bytes.starts(with: [0xEF, 0xBB, 0xBF]) { bytes.removeFirst(3) }
        }
        if bytes.isEmpty { return dispatch() }
        if bytes[0] == UInt8(ascii: ":") { return nil }  // a comment

        let text = String(decoding: bytes, as: UTF8.self)
        let field: String
        var value: String
        if let colon = text.firstIndex(of: ":") {
            field = String(text[text.startIndex..<colon])
            value = String(text[text.index(after: colon)...])
            if value.hasPrefix(" ") { value.removeFirst() }
        } else {
            field = text
            value = ""
        }
        switch field {
        case "event": eventName = value
        case "data": dataLines.append(value)
        case "id":
            // An id with a NUL is ignored by the specification.
            if !value.contains("\u{0}") { eventID = value; lastEventID = value }
        case "retry":
            if !value.isEmpty, value.allSatisfy(\.isASCII), let number = Int(value) { retry = number }
        default: break
        }
        return nil
    }

    private mutating func dispatch() -> SSEMessage? {
        defer {
            eventName = nil
            eventID = nil
            dataLines = []
            retry = nil
        }
        guard !dataLines.isEmpty else { return nil }
        return SSEMessage(
            event: (eventName?.isEmpty ?? true) ? "message" : (eventName ?? "message"),
            id: eventID, data: dataLines.joined(separator: "\n"), retryMilliseconds: retry)
    }
}

/// What one message of the daemon's stream says (`ApiEvent` in the OpenAPI document). Kept free of the generated types so
/// that an event this app does not know yet (`unknown`) never breaks the stream.
enum DaemonEvent: Sendable, Equatable {
    case hello(lastEventID: Int64, version: String)
    /// The daemon cannot replay what was missed: refetch everything shown.
    case resync(reason: String)
    case optimization(id: String, status: String)
    case agentEvent(optimizationID: String, agentID: String)
    case candidate(optimizationID: String, candidateID: String, verdict: String)
    case ratchet(optimizationID: String, from: Double, to: Double)
    case spend(optimizationID: String, totalUSD: Double)
    case notice(optimizationID: String, level: String, text: String)
    case runActive(active: Bool, optimizationIDs: [String])
    case sandbox(sandboxID: String, phase: String, text: String)
    case environmentDraft(draftID: String, status: String)
    case connector(connectorID: String)
    case github(signedIn: Bool)
    case unknown(type: String)

    private struct Payload: Decodable {
        var type: String?
        var lastEventId: Int64?
        var version: String?
        var reason: String?
        var optimizationId: String?
        var status: String?
        var agentId: String?
        var candidateId: String?
        var verdict: String?
        var from: Double?
        var to: Double?
        var totalUsd: Double?
        var level: String?
        var text: String?
        var active: Bool?
        var optimizationIds: [String]?
        var sandboxId: String?
        var phase: String?
        var draftId: String?
        var connectorId: String?
        var signedIn: Bool?
    }

    /// nil when the message is not JSON at all.
    static func decode(_ message: SSEMessage) -> DaemonEvent? {
        guard let payload = try? JSONDecoder().decode(Payload.self, from: Data(message.data.utf8)) else { return nil }
        let type = message.event == "message" ? (payload.type ?? message.event) : message.event
        switch type {
        case "hello":
            return .hello(lastEventID: payload.lastEventId ?? 0, version: payload.version ?? "")
        case "resync":
            return .resync(reason: payload.reason ?? "")
        case "optimization":
            guard let id = payload.optimizationId else { return .unknown(type: type) }
            return .optimization(id: id, status: payload.status ?? "")
        case "agent-event":
            guard let id = payload.optimizationId, let agent = payload.agentId else { return .unknown(type: type) }
            return .agentEvent(optimizationID: id, agentID: agent)
        case "candidate":
            guard let id = payload.optimizationId, let candidate = payload.candidateId else { return .unknown(type: type) }
            return .candidate(optimizationID: id, candidateID: candidate, verdict: payload.verdict ?? "")
        case "ratchet":
            guard let id = payload.optimizationId else { return .unknown(type: type) }
            return .ratchet(optimizationID: id, from: payload.from ?? 0, to: payload.to ?? 0)
        case "spend":
            guard let id = payload.optimizationId else { return .unknown(type: type) }
            return .spend(optimizationID: id, totalUSD: payload.totalUsd ?? 0)
        case "notice":
            guard let id = payload.optimizationId else { return .unknown(type: type) }
            return .notice(optimizationID: id, level: payload.level ?? "info", text: payload.text ?? "")
        case "run-active":
            return .runActive(active: payload.active ?? false, optimizationIDs: payload.optimizationIds ?? [])
        case "sandbox":
            guard let id = payload.sandboxId else { return .unknown(type: type) }
            return .sandbox(sandboxID: id, phase: payload.phase ?? "", text: payload.text ?? "")
        case "environment-draft":
            guard let id = payload.draftId else { return .unknown(type: type) }
            return .environmentDraft(draftID: id, status: payload.status ?? "")
        case "connector":
            guard let id = payload.connectorId else { return .unknown(type: type) }
            return .connector(connectorID: id)
        case "github":
            return .github(signedIn: payload.signedIn ?? false)
        default:
            return .unknown(type: type)
        }
    }
}
