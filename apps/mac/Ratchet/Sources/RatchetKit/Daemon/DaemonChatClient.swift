import Foundation
import HTTPTypes
import OpenAPIRuntime
import Synchronization

/// `ChatClient` on the real daemon (`/v1/chats`): JSON calls through the shared transport and its error middleware, and one
/// `SSEConnection` per followed chat. The connection reconnects with `Last-Event-ID` by itself, so a stream returned here
/// ends only when the chat is gone, the caller stops reading, or the daemon refuses it.
public final class DaemonChatClient: ChatClient {
    let wire: DaemonTransport
    let policy: SSEConnection.Policy

    init(wire: DaemonTransport, policy: SSEConnection.Policy) {
        self.wire = wire
        self.policy = policy
    }

    public convenience init(transport: any ClientTransport, policy: SSEConnection.Policy = SSEConnection.Policy()) {
        self.init(wire: DaemonTransport(transport: transport), policy: policy)
    }

    public convenience init(location: DaemonLocation) {
        self.init(wire: DaemonTransport(location: location), policy: SSEConnection.Policy())
    }

    // MARK: Calls

    public func startChat(_ new: NewChat) async throws -> Chat {
        var fields: [String: Any] = [:]
        if let repo = new.repo { fields["repo"] = repo }
        if let optimization = new.optimization { fields["optimization"] = optimization }
        if let harness = new.harness { fields["harness"] = harness.rawValue }
        if let model = new.model { fields["model"] = model }
        let body = try DaemonTransport.jsonBody(fields)
        let data: Data
        do {
            data = try await wire.jsonCall(.post, "/v1/chats", operationID: "createChat", json: body)
        } catch {
            // 409: no agent is installed and signed in, so no chat can start.
            if let api = daemonError(in: error), api.status == 409 { throw RatchetClientError.unavailable(api.message) }
            throw wire.translate(error)
        }
        return try ChatWire.chat(ChatWire.decode(ChatWire.Chat.self, data))
    }

    public func chat(id: String, after seq: Int64?, limit: Int) async throws -> Chat {
        var query: [String] = []
        if let seq { query.append("after=\(seq)") }
        if limit > 0 { query.append("limit=\(min(limit, 1000))") }
        let path = "/v1/chats/\(wire.escaped(id))" + (query.isEmpty ? "" : "?" + query.joined(separator: "&"))
        return try await wire.calling {
            let data = try await wire.jsonCall(.get, path, operationID: "getChat")
            return try ChatWire.chat(ChatWire.decode(ChatWire.Chat.self, data))
        }
    }

    public func send(chatID: String, text: String) async throws -> ChatReceipt {
        try await wire.calling {
            let body = try DaemonTransport.jsonBody(["text": text])
            let data = try await wire.jsonCall(
                .post, "/v1/chats/\(wire.escaped(chatID))/messages", operationID: "sendChatMessage", json: body)
            let accepted = try ChatWire.decode(ChatWire.Accepted.self, data)
            return ChatReceipt(seq: accepted.seq, turn: accepted.turn)
        }
    }

    public func interrupt(chatID: String) async throws {
        try await wire.calling {
            _ = try await wire.jsonCall(.post, "/v1/chats/\(wire.escaped(chatID))/interrupt", operationID: "interruptChat")
        }
    }

    public func end(chatID: String) async throws {
        try await wire.calling {
            _ = try await wire.jsonCall(.delete, "/v1/chats/\(wire.escaped(chatID))", operationID: "deleteChat")
        }
    }

    // MARK: The stream

    /// What ended a stream for good: the daemon refused it (the chat is gone, the token is wrong).
    private final class Refusal: Sendable {
        let error = Mutex<(any Error)?>(nil)
    }

    public func stream(chatID: String, lastEventID: Int64?) async -> AsyncThrowingStream<ChatFrame, any Error> {
        let (stream, continuation) = AsyncThrowingStream<ChatFrame, any Error>.makeStream()
        let wire = self.wire
        let policy = self.policy
        let refusal = Refusal()
        let path = "/v1/chats/\(wire.escaped(chatID))/stream"
        let task = Task {
            let connection = SSEConnection(policy: policy) { lastID in
                do {
                    return try await wire.openStream(path, lastEventID: lastID, operationID: "streamChat")
                } catch {
                    // A refusal will not get better by asking again: record it and stop the connection (a cancellation ends it).
                    if let api = daemonError(in: error), api.status == 404 || api.status == 401 {
                        refusal.error.withLock { $0 = wire.translate(api) }
                        throw CancellationError()
                    }
                    throw error
                }
            }
            await connection.run(from: lastEventID.map { String($0) }) { item, _ in
                guard case .messages(let messages) = item else { return }
                for message in messages {
                    if let frame = ChatWire.frame(message) { continuation.yield(frame) }
                }
            }
            if let failure = refusal.error.withLock({ $0 }) {
                continuation.finish(throwing: failure)
            } else {
                continuation.finish()
            }
        }
        continuation.onTermination = { _ in task.cancel() }
        return stream
    }
}
