import Foundation
import HTTPTypes
import OpenAPIRuntime
import Testing
@testable import RatchetKit

/// Streams of `GET /v1/chats/c1/stream` played in order, recording how each was asked for.
private final class ScriptedStreams: ClientTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var bodies: [String]
    private var seen: [HTTPFields] = []
    var status = 200

    init(_ bodies: [String]) { self.bodies = bodies }

    var requests: [HTTPFields] { lock.withLock { seen } }

    func send(_ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String) async throws -> (HTTPResponse, HTTPBody?) {
        let next: String? = lock.withLock {
            seen.append(request.headerFields)
            return bodies.isEmpty ? nil : (bodies.count > 1 ? bodies.removeFirst() : bodies[0])
        }
        if status != 200 {
            var response = HTTPResponse(status: .init(code: status))
            response.headerFields[.contentType] = "application/json"
            return (response, HTTPBody(#"{"code":"not_found","message":"no such chat"}"#))
        }
        var response = HTTPResponse(status: .ok)
        response.headerFields[.contentType] = "text/event-stream"
        return (response, HTTPBody(next ?? ""))
    }
}

@Suite("Daemon chat client")
struct DaemonChatClientTests {
    private let fast = SSEConnection.Policy(initialPause: .milliseconds(1), maxPause: .milliseconds(2), maxFailures: 5, sleep: { _ in })

    // MARK: Decoding what the daemon writes

    private func frame(_ event: String, id: String? = nil, _ json: String) -> ChatFrame? {
        ChatWire.frame(SSEMessage(event: event, id: id, data: json))
    }

    @Test func everyEventOfTheStreamDecodes() throws {
        let ready = try #require(frame("ready", #"{"type":"ready","chatId":"c1","harness":"claude-code","model":null,"state":"replying","lastSeq":4,"lastEventId":12,"resync":true,"pending":{"text":"Looking"}}"#))
        #expect(ready.id == nil)
        #expect(ready.event == .ready(ChatReady(state: .replying, lastSeq: 4, lastEventID: 12, resync: true, pending: "Looking")))

        #expect(frame("delta", id: "13", #"{"type":"delta","text":"Hel"}"#) == ChatFrame(id: 13, event: .delta("Hel")))
        #expect(frame("thinking", id: "14", #"{"type":"thinking","text":"hm"}"#)?.event == .thinking("hm"))

        let tool = try #require(frame("tool", id: "15", #"{"type":"tool","tool":{"id":"t1","name":"checkout","state":"running","summary":"Reading acme/shop"}}"#))
        #expect(tool.event == .tool(ChatToolStep(id: "t1", name: "checkout", state: .running, summary: "Reading acme/shop")))

        let message = try #require(frame("message", id: "16", #"{"type":"message","message":{"seq":2,"role":"assistant","text":"Done.","harness":"codex","model":"gpt-5","at":1793452800000,"usage":{"inputTokens":10,"outputTokens":5,"billing":"api-key","costUsd":0.01}}}"#))
        guard case .message(let value) = message.event else { Issue.record("not a message"); return }
        #expect(value.harness == .codex && value.model == "gpt-5" && value.at == Date(timeIntervalSince1970: 1_793_452_800))
        #expect(value.usage == ChatUsage(inputTokens: 10, outputTokens: 5, costUSD: 0.01, billing: .apiKey))

        let usage = try #require(frame("usage", id: "17", #"{"type":"usage","usage":{"inputTokens":3,"outputTokens":4}}"#))
        #expect(usage.event == .usage(ChatUsage(inputTokens: 3, outputTokens: 4)))

        let done = try #require(frame("done", id: "18", #"{"type":"done","turn":2,"outcome":"interrupted","lastSeq":3,"usage":{"inputTokens":1,"outputTokens":2}}"#))
        #expect(done.event == .done(turn: 2, outcome: .interrupted, usage: ChatUsage(inputTokens: 1, outputTokens: 2)))
    }

    @Test func aDraftCardKeepsItsRowsOptionsAndRawValues() throws {
        let json = #"""
        {"type":"card","seq":3,"replaces":"draft_1","card":{"id":"draft_2","kind":"draft","status":"draft","title":"Checkout p95","subtitle":"up to $50 per run","repo":"acme/shop",
         "rows":[{"key":"agents","label":"Agents","value":"Claude Code → Codex · on this Mac","editable":true,"raw":["claude-code","codex"],
                  "options":[{"value":"claude-code","label":"Claude Code"},{"value":"codex","label":"Codex"}]},
                 {"key":"limits","label":"Each run","value":"up to $50","editable":true,"raw":{"budgetUsd":50.0}},
                 {"key":"output","label":"Output","value":"a pull request with verified wins","editable":false,"raw":null}]}}
        """#
        let result = try #require(frame("card", id: "20", json))
        guard case .card(let card, let replaces) = result.event else { Issue.record("not a card"); return }
        #expect(replaces == "draft_1" && card.id == "draft_2" && card.subtitle == "up to $50 per run")
        #expect(card.rows.map(\.key) == ["agents", "limits", "output"])
        #expect(card.rows[0].raw.strings == ["claude-code", "codex"] && card.rows[0].options.map(\.label) == ["Claude Code", "Codex"])
        #expect(card.rows[1].raw["budgetUsd"]?.number == 50 && card.rows[1].editable)
        #expect(card.rows[2].raw == .null && !card.rows[2].editable)
    }

    @Test func aSteeringNoteCarriesItsReadReceipt() throws {
        let queued = try #require(frame("note", id: "30", #"{"type":"note","seq":5,"card":{"id":"note_1","kind":"note","status":"queued","title":"focus on database queries","optimization":"opt_1"}}"#))
        guard case .note(let card) = queued.event else { Issue.record("not a note"); return }
        #expect(card.kind == .note && card.status == .queued && card.optimization == "opt_1" && card.readAt == nil)
        let read = try #require(frame("note", id: "31", #"{"type":"note","seq":5,"card":{"id":"note_1","kind":"note","status":"read","title":"focus on database queries","readBy":"Claude Code","readAt":1793452800000}}"#))
        guard case .note(let after) = read.event else { Issue.record("not a note"); return }
        #expect(after.status == .read && after.readBy == "Claude Code" && after.readAt == Date(timeIntervalSince1970: 1_793_452_800))
    }

    @Test func aFailedTurnIsATypedErrorWithItsFixes() throws {
        let json = #"{"type":"error","error":{"code":"rate_limited","harness":"claude-code","title":"Claude Code reached its usage limit","message":"It resets at 15:40.","retry":{"resetsAt":1793457000000,"canRetryNow":false},"fixes":["retry","switch-harness"],"switchTo":"codex"}}"#
        let result = try #require(frame("error", id: "40", json))
        guard case .failed(let error) = result.event else { Issue.record("not an error"); return }
        #expect(error.code == .rateLimited && error.harness == .claudeCode && error.switchTo == .codex)
        #expect(error.fixes == [.retry, .switchHarness] && error.retry?.canRetryNow == false)
        #expect(error.retry?.resetsAt == Date(timeIntervalSince1970: 1_793_457_000))
    }

    @Test func anUnknownOrUnreadableEventIsSkipped() {
        #expect(frame("hologram", id: "9", #"{"type":"hologram"}"#) == nil)
        #expect(ChatWire.frame(SSEMessage(event: "delta", id: "1", data: "not json")) == nil)
        #expect(frame("message", #"{"type":"message","message":{"seq":1,"role":"hologram","text":"?","at":0}}"#) == nil)
    }

    @Test func theChatHistoryMapsEverything() throws {
        let json = #"""
        {"id":"c1","repo":"acme/shop","optimization":null,"harness":"grok","model":"grok-4","state":"replying","live":true,"createdAt":1793452000000,"updatedAt":1793452800000,
         "dataNotice":{"harness":"grok","text":"First time: your messages go to Grok."},"hasMore":true,"lastEventId":42,"pending":{"text":"Wri"},
         "messages":[{"seq":1,"role":"user","text":"hi","at":1793452000000},{"seq":2,"role":"tool","text":"Read x","at":1793452001000,"tool":{"id":"t","name":"Read","state":"done","summary":"Read x"}}]}
        """#
        let chat = try ChatWire.chat(JSONDecoder().decode(ChatWire.Chat.self, from: Data(json.utf8)))
        #expect(chat.harness == .grok && chat.repo == "acme/shop" && chat.optimization == nil && chat.model == "grok-4")
        #expect(chat.state == .replying && chat.pending == "Wri" && chat.lastEventID == 42 && chat.hasMore)
        #expect(chat.dataNotice == "First time: your messages go to Grok.")
        #expect(chat.messages.map(\.role) == [.user, .tool] && chat.messages[1].tool?.state == .done)
    }

    @Test func aStoredHistoryRebuildsItsCardsAndStepsPerTurn() {
        let card = ChatCard(id: "d1", title: "Checkout p95")
        let chat = Chat(
            id: "c1", harness: .claudeCode,
            messages: [
                ChatMessage(seq: 1, role: .user, text: "make it faster", at: .distantPast),
                ChatMessage(seq: 2, role: .tool, text: "Read x", at: .distantPast, tool: ChatToolStep(id: "t", name: "Read", state: .done, summary: "Read x")),
                ChatMessage(seq: 3, role: .assistant, text: "Here is a draft.", at: .distantPast, harness: .claudeCode),
                ChatMessage(seq: 4, role: .card, text: "", at: .distantPast, card: card),
            ])
        var conversation = ChatConversation(chat)
        conversation.replace(with: chat)
        #expect(conversation.cards.map(\.id) == ["d1"] && conversation.cardAnchors["d1"] == 1)
        #expect(conversation.turnSteps[1]?.map(\.summary) == ["Read x"])
        let items = ChatTimeline.items(conversation).map(\.id)
        #expect(items == ["user-1", "step-t", "reply-3", "card-d1"])
    }

    // MARK: Errors and the connection

    @Test func noAgentSignedInIsUnavailableNotInvalid() async {
        let fake = ChatDaemonFake(mock: MockChatClient(readyHarnesses: []))
        let client = DaemonChatClient(transport: fake, policy: fast)
        do {
            _ = try await client.startChat(NewChat())
            Issue.record("started")
        } catch let error as RatchetClientError {
            #expect(error == .unavailable("No agent is signed in. Sign in to Claude Code, Codex or Grok first."))
        } catch {
            Issue.record("threw \(error)")
        }
    }

    @Test(.timeLimit(.minutes(1))) func aDroppedStreamReconnectsFromTheLastIdItSaw() async throws {
        let first = "event: ready\ndata: {\"type\":\"ready\",\"state\":\"replying\",\"lastSeq\":1,\"lastEventId\":0,\"resync\":false}\n\n"
            + "event: delta\nid: 1\ndata: {\"type\":\"delta\",\"text\":\"a\"}\n\n"
            + "event: delta\nid: 2\ndata: {\"type\":\"delta\",\"text\":\"b\"}\n\n"
        let second = "event: ready\ndata: {\"type\":\"ready\",\"state\":\"replying\",\"lastSeq\":1,\"lastEventId\":3,\"resync\":false}\n\n"
            + "event: delta\nid: 3\ndata: {\"type\":\"delta\",\"text\":\"c\"}\n\n"
        let transport = ScriptedStreams([first, second])
        let client = DaemonChatClient(transport: transport, policy: fast)
        var text = ""
        for try await frame in await client.stream(chatID: "c1", lastEventID: nil) {
            if case .delta(let part) = frame.event { text += part }
            if text == "abc" { break }
        }
        #expect(text == "abc")
        let name = try #require(HTTPField.Name("Last-Event-ID"))
        #expect(transport.requests.count >= 2)
        #expect(transport.requests[0][name] == nil)
        #expect(transport.requests[1][name] == "2")
    }

    @Test(.timeLimit(.minutes(1))) func aStreamRefusedByTheDaemonEndsWithThatError() async {
        let transport = ScriptedStreams([""])
        transport.status = 404
        let client = DaemonChatClient(transport: transport, policy: fast)
        do {
            for try await _ in await client.stream(chatID: "gone", lastEventID: nil) {}
            Issue.record("the stream did not fail")
        } catch let error as RatchetClientError {
            #expect(error == .notFound("no such chat"))
        } catch {
            Issue.record("threw \(error)")
        }
        #expect(transport.requests.count == 1)
    }
}
