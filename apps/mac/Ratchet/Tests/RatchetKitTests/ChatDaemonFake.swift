import Foundation
import HTTPTypes
import OpenAPIRuntime
@testable import RatchetKit

/// A daemon for the chat routes that answers from a `MockChatClient`: requests are decoded, the mock does the work, and its
/// answers are written back as the daemon's JSON and server-sent events. Running the shared contract over `DaemonChatClient`
/// on this proves the wire (requests, decoding, `Last-Event-ID`, errors) says what the mock says.
final class ChatDaemonFake: ClientTransport, @unchecked Sendable {
    struct Seen {
        var method: String
        var path: String
        var headers: HTTPFields
        var body: String
    }

    let mock: MockChatClient
    private let lock = NSLock()
    private var log: [Seen] = []

    init(mock: MockChatClient) {
        self.mock = mock
    }

    var requests: [Seen] { lock.withLock { log } }

    // MARK: Encoding (app model to wire)

    static func ms(_ date: Date) -> Int64 { Int64((date.timeIntervalSince1970 * 1000).rounded()) }

    static func usage(_ usage: ChatUsage) -> ChatWire.Usage {
        ChatWire.Usage(inputTokens: usage.inputTokens, outputTokens: usage.outputTokens, costUsd: usage.costUSD, billing: usage.billing?.rawValue)
    }

    static func card(_ card: ChatCard) -> ChatWire.Card {
        ChatWire.Card(
            id: card.id, kind: card.kind.rawValue, status: card.status.rawValue, title: card.title, subtitle: card.subtitle,
            repo: card.repo,
            rows: card.rows.map { row in
                ChatWire.Row(
                    key: row.key, label: row.label, value: row.value, editable: row.editable, raw: row.raw,
                    options: row.options.map { ChatWire.Option(value: .string($0.value), label: $0.label) })
            },
            optimization: card.optimization,
            error: card.errorTitle.map { ChatWire.CardError(title: $0, message: card.errorMessage ?? "") }, readBy: card.readBy,
            readAt: card.readAt.map(ms))
    }

    static func message(_ message: ChatMessage) -> ChatWire.Message {
        ChatWire.Message(
            seq: message.seq, role: message.role.rawValue, text: message.text, harness: message.harness?.rawValue, model: message.model,
            card: message.card.map(card),
            tool: message.tool.map { ChatWire.Tool(id: $0.id, name: $0.name, state: $0.state.rawValue, summary: $0.summary) },
            usage: message.usage.map(usage), at: ms(message.at))
    }

    static func chat(_ chat: Chat) -> ChatWire.Chat {
        ChatWire.Chat(
            id: chat.id, repo: chat.repo, optimization: chat.optimization, harness: chat.harness.rawValue, model: chat.model,
            state: chat.state.rawValue, live: false, createdAt: ms(chat.updatedAt), updatedAt: ms(chat.updatedAt),
            dataNotice: chat.dataNotice.map { ChatWire.DataNotice(harness: chat.harness.rawValue, text: $0) },
            messages: chat.messages.map(message), hasMore: chat.hasMore, pending: chat.pending.map { ChatWire.Pending(text: $0) },
            lastEventId: chat.lastEventID)
    }

    static func failure(_ error: ChatError) -> ChatWire.Failure {
        ChatWire.Failure(
            code: error.code.rawValue, harness: error.harness.rawValue, title: error.title, message: error.message,
            retry: error.retry.map { ChatWire.Retry(resetsAt: $0.resetsAt.map(ms), canRetryNow: $0.canRetryNow) },
            fixes: error.fixes.map(\.rawValue), switchTo: error.switchTo?.rawValue)
    }

    /// One frame as the server-sent event the daemon writes.
    static func sse(_ frame: ChatFrame) -> String {
        var envelope = ChatWire.Envelope()
        let name: String
        switch frame.event {
        case .ready(let ready):
            name = "ready"
            envelope.state = ready.state.rawValue
            envelope.lastSeq = ready.lastSeq
            envelope.lastEventId = ready.lastEventID
            envelope.resync = ready.resync
            envelope.pending = ready.pending.map { ChatWire.Pending(text: $0) }
        case .message(let value):
            name = "message"
            envelope.message = message(value)
        case .delta(let text):
            name = "delta"
            envelope.text = text
        case .thinking(let text):
            name = "thinking"
            envelope.text = text
        case .tool(let step):
            name = "tool"
            envelope.tool = ChatWire.Tool(id: step.id, name: step.name, state: step.state.rawValue, summary: step.summary)
        case .card(let value, let replaces):
            name = "card"
            envelope.card = card(value)
            envelope.replaces = replaces
        case .note(let value):
            name = "note"
            envelope.card = card(value)
        case .usage(let value):
            name = "usage"
            envelope.usage = usage(value)
        case .done(let turn, let outcome, let value):
            name = "done"
            envelope.turn = turn
            envelope.outcome = outcome.rawValue
            envelope.usage = usage(value)
        case .failed(let error):
            name = "error"
            envelope.error = failure(error)
        }
        envelope.type = name
        let json = (try? JSONEncoder().encode(envelope)).map { String(decoding: $0, as: UTF8.self) } ?? "{}"
        return "event: \(name)\n" + (frame.id.map { "id: \($0)\n" } ?? "") + "data: \(json)\n\n"
    }

    // MARK: Routes

    private func json<T: Encodable>(_ value: T, status: Int = 200) -> (HTTPResponse, HTTPBody?) {
        var response = HTTPResponse(status: .init(code: status))
        response.headerFields[.contentType] = "application/json"
        let data = (try? JSONEncoder().encode(value)) ?? Data()
        return (response, HTTPBody(String(decoding: data, as: UTF8.self)))
    }

    private func empty(_ status: Int) -> (HTTPResponse, HTTPBody?) {
        (HTTPResponse(status: .init(code: status)), nil)
    }

    private func failure(_ error: any Error, on conflict: Bool = false) -> (HTTPResponse, HTTPBody?) {
        var response = HTTPResponse(status: .init(code: 500))
        var code = "internal"
        var message = error.localizedDescription
        if let error = error as? RatchetClientError {
            switch error {
            case .notFound(let what):
                response = HTTPResponse(status: .init(code: 404))
                code = "not_found"
                message = "Not found: \(what)"
            case .invalid(let why):
                response = HTTPResponse(status: .init(code: 409))
                code = "conflict"
                message = why
            case .unavailable(let why):
                response = HTTPResponse(status: .init(code: 409))
                code = "conflict"
                message = why
            }
        }
        response.headerFields[.contentType] = "application/json"
        let body = (try? JSONSerialization.data(withJSONObject: ["code": code, "message": message])) ?? Data()
        return (response, HTTPBody(String(decoding: body, as: UTF8.self)))
    }

    func send(_ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String) async throws -> (HTTPResponse, HTTPBody?) {
        var text = ""
        if let body {
            for try await chunk in body { text += String(decoding: chunk, as: UTF8.self) }
        }
        let full = request.path ?? ""
        let path = full.split(separator: "?", maxSplits: 1).first.map(String.init) ?? full
        let query = full.contains("?") ? String(full.split(separator: "?", maxSplits: 1)[1]) : ""
        lock.withLock { log.append(Seen(method: request.method.rawValue, path: full, headers: request.headerFields, body: text)) }
        let parts = path.split(separator: "/").map(String.init)  // v1, chats, id?, action?
        let fields = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]

        do {
            switch (request.method, parts.count) {
            case (.post, 2):
                let new = NewChat(
                    repo: fields["repo"] as? String, optimization: fields["optimization"] as? String,
                    harness: (fields["harness"] as? String).flatMap { Harness(rawValue: $0) }, model: fields["model"] as? String)
                return json(Self.chat(try await mock.startChat(new)), status: 201)
            case (.get, 3):
                var after: Int64?
                var limit = 200
                for pair in query.split(separator: "&") {
                    let kv = pair.split(separator: "=")
                    if kv.count == 2, kv[0] == "after" { after = Int64(kv[1]) }
                    if kv.count == 2, kv[0] == "limit" { limit = Int(kv[1]) ?? 200 }
                }
                return json(Self.chat(try await mock.chat(id: parts[2], after: after, limit: limit)))
            case (.delete, 3):
                try await mock.end(chatID: parts[2])
                return empty(204)
            case (.post, 4) where parts[3] == "messages":
                let receipt = try await mock.send(chatID: parts[2], text: fields["text"] as? String ?? "")
                return json(ChatWire.Accepted(seq: receipt.seq, turn: receipt.turn), status: 202)
            case (.post, 4) where parts[3] == "interrupt":
                try await mock.interrupt(chatID: parts[2])
                return empty(204)
            case (.get, 4) where parts[3] == "stream":
                _ = try await mock.chat(id: parts[2], after: nil, limit: 1)  // 404 for a chat that is gone
                let last = HTTPField.Name("Last-Event-ID").flatMap { request.headerFields[$0] }.flatMap { Int64($0) }
                let frames = await mock.stream(chatID: parts[2], lastEventID: last)
                let (stream, continuation) = AsyncThrowingStream<ArraySlice<UInt8>, any Error>.makeStream()
                let pump = Task {
                    do {
                        for try await frame in frames { continuation.yield(Array(Self.sse(frame).utf8)[...]) }
                        continuation.finish()
                    } catch {
                        continuation.finish(throwing: error)
                    }
                }
                continuation.onTermination = { _ in pump.cancel() }
                var response = HTTPResponse(status: .ok)
                response.headerFields[.contentType] = "text/event-stream"
                return (response, HTTPBody(stream, length: .unknown))
            default:
                return failure(RatchetClientError.notFound("route \(path)"))
            }
        } catch {
            return failure(error)
        }
    }
}
