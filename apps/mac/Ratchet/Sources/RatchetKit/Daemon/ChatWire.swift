import Foundation

/// The JSON of the chat routes (`api/openapi.yaml`, `docs/ratchet-chat.md`), written by hand because the generated client does
/// not hold them, and kept thin: decoding and the mapping to the app's chat models, nothing else. When the generated client
/// gains the chat operations, only this file and `DaemonChatClient` change.
enum ChatWire {
    struct Usage: Codable {
        var inputTokens: Int?
        var outputTokens: Int?
        var costUsd: Double?
        var billing: String?
    }

    struct Tool: Codable {
        var id: String
        var name: String
        var state: String
        var summary: String
    }

    struct Option: Codable {
        var value: JSONValue
        var label: String
    }

    struct Row: Codable {
        var key: String
        var label: String
        var value: String
        var editable: Bool?
        var raw: JSONValue?
        var options: [Option]?
    }

    struct CardError: Codable {
        var title: String
        var message: String
    }

    struct Card: Codable {
        var id: String
        var kind: String?
        var status: String?
        var title: String
        var subtitle: String?
        var repo: String?
        var rows: [Row]?
        var optimization: String?
        var error: CardError?
        var readBy: String?
        var readAt: Int64?
    }

    struct Message: Codable {
        var seq: Int64
        var role: String
        var text: String
        var harness: String?
        var model: String?
        var card: Card?
        var tool: Tool?
        var usage: Usage?
        var at: Int64
    }

    struct Pending: Codable {
        var text: String
    }

    struct DataNotice: Codable {
        var harness: String?
        var text: String
    }

    struct Chat: Codable {
        var id: String
        var repo: String?
        var optimization: String?
        var harness: String
        var model: String?
        var state: String
        var live: Bool?
        var createdAt: Int64?
        var updatedAt: Int64?
        var dataNotice: DataNotice?
        var messages: [Message]?
        var hasMore: Bool?
        var pending: Pending?
        var lastEventId: Int64?
    }

    struct Retry: Codable {
        var resetsAt: Int64?
        var canRetryNow: Bool?
    }

    struct Failure: Codable {
        var code: String
        var harness: String
        var title: String
        var message: String
        var retry: Retry?
        var fixes: [String]?
        var switchTo: String?
    }

    struct Accepted: Codable {
        var seq: Int64
        var turn: Int
    }

    /// Every field any stream event can carry; `type` says which are there.
    struct Envelope: Codable {
        var type: String?
        var text: String?
        var message: Message?
        var tool: Tool?
        var card: Card?
        var replaces: String?
        var usage: Usage?
        var turn: Int?
        var outcome: String?
        var error: Failure?
        var state: String?
        var lastSeq: Int64?
        var lastEventId: Int64?
        var resync: Bool?
        var pending: Pending?
    }

    // MARK: Decoding

    private static let decoder = JSONDecoder()

    static func decode<T: Decodable>(_ type: T.Type, _ data: Data) throws -> T {
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw RatchetClientError.unavailable("ratchetd answered with something the app cannot read.")
        }
    }

    // MARK: Mapping (wire to app)

    static func date(ms: Int64?) -> Date {
        Date(timeIntervalSince1970: Double(ms ?? 0) / 1000)
    }

    static func billing(_ raw: String?) -> ChatBilling? {
        raw.flatMap { ChatBilling(rawValue: $0) }
    }

    static func usage(_ wire: Usage?) -> ChatUsage {
        ChatUsage(
            inputTokens: wire?.inputTokens ?? 0, outputTokens: wire?.outputTokens ?? 0, costUSD: wire?.costUsd,
            billing: billing(wire?.billing))
    }

    static func tool(_ wire: Tool) -> ChatToolStep {
        ChatToolStep(id: wire.id, name: wire.name, state: ChatToolState(rawValue: wire.state) ?? .running, summary: wire.summary)
    }

    /// An option's value as the text a change sends back: a string as it is, a number or a list written out.
    static func optionValue(_ value: JSONValue) -> String {
        switch value {
        case .string(let text): return text
        case .number(let number): return number == number.rounded() ? String(Int(number)) : String(number)
        case .bool(let flag): return flag ? "true" : "false"
        case .null: return ""
        default:
            guard let data = try? JSONEncoder().encode(value) else { return "" }
            return String(decoding: data, as: UTF8.self)
        }
    }

    static func card(_ wire: Card) -> ChatCard {
        ChatCard(
            id: wire.id, kind: ChatCardKind(rawValue: wire.kind ?? "") ?? .draft,
            status: ChatCardStatus(rawValue: wire.status ?? "") ?? .draft, title: wire.title, subtitle: wire.subtitle,
            repo: wire.repo, optimization: wire.optimization,
            rows: (wire.rows ?? []).map { row in
                ChatCardRow(
                    key: row.key, label: row.label, value: row.value, editable: row.editable ?? false,
                    options: (row.options ?? []).map { ChatCardOption(label: $0.label, value: optionValue($0.value)) },
                    raw: row.raw ?? .null)
            },
            errorTitle: wire.error?.title, errorMessage: wire.error?.message, readBy: wire.readBy,
            readAt: wire.readAt.map { date(ms: $0) })
    }

    /// nil for a role this app does not know: such a message is skipped, not fatal.
    static func message(_ wire: Message) -> ChatMessage? {
        guard let role = ChatRole(rawValue: wire.role) else { return nil }
        return ChatMessage(
            seq: wire.seq, role: role, text: wire.text, at: date(ms: wire.at), harness: wire.harness.flatMap { Harness(rawValue: $0) },
            model: wire.model, card: wire.card.map(card), tool: wire.tool.map(tool), usage: wire.usage.map(usage))
    }

    static func chat(_ wire: Chat) throws -> RatchetKit.Chat {
        guard let harness = Harness(rawValue: wire.harness) else {
            throw RatchetClientError.unavailable("ratchetd named an agent this app does not know: \(wire.harness).")
        }
        return RatchetKit.Chat(
            id: wire.id, repo: wire.repo, optimization: wire.optimization, harness: harness, model: wire.model,
            state: ChatState(rawValue: wire.state) ?? .idle, messages: (wire.messages ?? []).compactMap(message),
            pending: wire.pending?.text, lastEventID: wire.lastEventId ?? 0, hasMore: wire.hasMore ?? false,
            dataNotice: wire.dataNotice?.text, updatedAt: date(ms: wire.updatedAt))
    }

    static func failure(_ wire: Failure) -> ChatError {
        ChatError(
            code: ChatErrorCode(rawValue: wire.code) ?? .failed, harness: Harness(rawValue: wire.harness) ?? .claudeCode,
            title: wire.title, message: wire.message, fixes: (wire.fixes ?? []).compactMap { ChatFix(rawValue: $0) },
            retry: wire.retry.map { ChatRetry(canRetryNow: $0.canRetryNow ?? false, resetsAt: $0.resetsAt.map { date(ms: $0) }) },
            switchTo: wire.switchTo.flatMap { Harness(rawValue: $0) })
    }

    /// One server-sent message as a frame of the chat stream; nil for an event this app does not know or cannot read.
    static func frame(_ message: SSEMessage) -> ChatFrame? {
        guard let envelope = try? decoder.decode(Envelope.self, from: Data(message.data.utf8)) else { return nil }
        let name = message.event == "message" ? (envelope.type ?? "message") : message.event
        let id = message.id.flatMap { Int64($0) }
        guard let event = event(named: name, envelope) else { return nil }
        return ChatFrame(id: id, event: event)
    }

    private static func event(named name: String, _ wire: Envelope) -> ChatEvent? {
        switch name {
        case "ready":
            return .ready(ChatReady(
                state: ChatState(rawValue: wire.state ?? "") ?? .idle, lastSeq: wire.lastSeq ?? 0, lastEventID: wire.lastEventId ?? 0,
                resync: wire.resync ?? false, pending: wire.pending?.text))
        case "message":
            return wire.message.flatMap(Self.message).map { .message($0) }
        case "delta":
            return .delta(wire.text ?? "")
        case "thinking":
            return .thinking(wire.text ?? "")
        case "tool":
            return wire.tool.map { .tool(tool($0)) }
        case "card":
            return wire.card.map { .card(card($0), replaces: wire.replaces) }
        case "note":
            return wire.card.map { .note(card($0)) }
        case "usage":
            return .usage(usage(wire.usage))
        case "done":
            return .done(turn: wire.turn ?? 0, outcome: ChatOutcome(rawValue: wire.outcome ?? "") ?? .completed, usage: usage(wire.usage))
        case "error":
            return wire.error.map { .failed(failure($0)) }
        default:
            return nil
        }
    }
}
