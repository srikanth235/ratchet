import Foundation
import Testing
@testable import RatchetKit

@Suite("Server-sent events")
struct SSETests {
    private func parse(_ text: String, pieces size: Int = 0) -> [SSEMessage] {
        var parser = SSEParser()
        let bytes = Array(text.utf8)
        if size <= 0 { return parser.feed(bytes) }
        var messages: [SSEMessage] = []
        var index = 0
        while index < bytes.count {
            messages += parser.feed(Array(bytes[index..<min(index + size, bytes.count)]))
            index += size
        }
        return messages
    }

    private let stream =
        "event: hello\ndata: {\"type\":\"hello\",\"lastEventId\":0,\"serverTime\":1,\"version\":\"0.0.0\"}\n\n"
        + ": keep-alive\n\n"
        + "event: spend\nid: 7\ndata: {\"type\":\"spend\",\"optimizationId\":\"o1\",\"totalUsd\":1.5}\n\n"

    @Test("Messages, comments and ids, in any split", arguments: [0, 1, 2, 3, 10, 50])
    func messages(size: Int) {
        let messages = parse(stream, pieces: size)
        #expect(messages.count == 2)
        #expect(messages[0].event == "hello")
        #expect(messages[0].id == nil)
        #expect(messages[1].event == "spend")
        #expect(messages[1].id == "7")
        #expect(messages[1].data.contains("\"totalUsd\":1.5"))
    }

    @Test func lineEndsMayBeLFCRLFOrCR() {
        for end in ["\n", "\r\n", "\r"] {
            let messages = parse("event: a\(end)id: 3\(end)data: x\(end)\(end)event: b\(end)data: y\(end)\(end)", pieces: 1)
            #expect(messages.map(\.event) == ["a", "b"])
            #expect(messages.map(\.data) == ["x", "y"])
        }
    }

    @Test func dataLinesAreJoinedAndOneSpaceIsDropped() {
        let messages = parse("data: one\ndata:  two\ndata\n\n")
        #expect(messages.count == 1)
        #expect(messages[0].event == "message")
        #expect(messages[0].data == "one\n two\n")
    }

    @Test func unknownFieldsAndEmptyBlocksAreIgnored() {
        let messages = parse("foo: bar\n\nevent: only-an-event\n\nretry: 2500\ndata: x\n\n")
        #expect(messages.count == 1)
        #expect(messages[0].retryMilliseconds == 2500)
    }

    @Test func theLastIDIsRemembered() {
        var parser = SSEParser()
        _ = parser.feed("id: 4\ndata: a\n\nid: 5\ndata: b\n\ndata: c\n\n")
        #expect(parser.lastEventID == "5")
        let more = parser.feed("data: d\n\n")
        #expect(more.first?.id == nil)
        #expect(parser.lastEventID == "5")
    }

    @Test func aByteOrderMarkAtTheStartIsSkipped() {
        var parser = SSEParser()
        let messages = parser.feed([0xEF, 0xBB, 0xBF] + Array("data: x\n\n".utf8))
        #expect(messages.first?.data == "x")
    }

    @Test func multibyteTextSurvivesAnySplit() {
        let messages = parse("data: Zürich → 値\n\n", pieces: 1)
        #expect(messages.first?.data == "Zürich → 値")
    }

    // MARK: What the daemon says

    private func event(_ name: String, _ json: String) -> DaemonEvent? {
        DaemonEvent.decode(SSEMessage(event: name, id: "1", data: json))
    }

    @Test func theDaemonsEventsAreDecoded() {
        #expect(event("hello", #"{"type":"hello","lastEventId":12,"serverTime":5,"version":"0.1"}"#) == .hello(lastEventID: 12, version: "0.1"))
        #expect(event("resync", #"{"type":"resync","reason":"restarted"}"#) == .resync(reason: "restarted"))
        #expect(event("optimization", #"{"type":"optimization","optimizationId":"o1","status":"running"}"#) == .optimization(id: "o1", status: "running"))
        #expect(event("agent-event", #"{"type":"agent-event","optimizationId":"o1","agentId":"a1","event":null}"#) == .agentEvent(optimizationID: "o1", agentID: "a1"))
        #expect(event("candidate", #"{"type":"candidate","optimizationId":"o1","candidateId":"c1","verdict":"verified"}"#) == .candidate(optimizationID: "o1", candidateID: "c1", verdict: "verified"))
        #expect(event("ratchet", #"{"type":"ratchet","optimizationId":"o1","from":182,"to":150.5}"#) == .ratchet(optimizationID: "o1", from: 182, to: 150.5))
        #expect(event("spend", #"{"type":"spend","optimizationId":"o1","totalUsd":3.25}"#) == .spend(optimizationID: "o1", totalUSD: 3.25))
        #expect(event("run-active", #"{"type":"run-active","active":true,"optimizationIds":["o1"]}"#) == .runActive(active: true, optimizationIDs: ["o1"]))
        #expect(event("sandbox", #"{"type":"sandbox","sandboxId":"s1","phase":"ready","text":"up","region":null}"#) == .sandbox(sandboxID: "s1", phase: "ready", text: "up"))
        #expect(event("environment-draft", #"{"type":"environment-draft","draftId":"d1","status":"ready"}"#) == .environmentDraft(draftID: "d1", status: "ready"))
        #expect(event("connector", #"{"type":"connector","connectorId":"c1"}"#) == .connector(connectorID: "c1"))
        #expect(event("github", #"{"type":"github","signedIn":true}"#) == .github(signedIn: true))
    }

    @Test func anEventFromTheFutureIsUnknownNotAnError() {
        #expect(event("something-new", #"{"type":"something-new","x":1}"#) == .unknown(type: "something-new"))
        #expect(event("optimization", #"{"type":"optimization"}"#) == .unknown(type: "optimization"))
        #expect(event("spend", "not json") == nil)
    }

    @Test func theTypeInThePayloadServesWhenThereIsNoEventName() {
        let message = SSEMessage(event: "message", id: "2", data: #"{"type":"github","signedIn":false}"#)
        #expect(DaemonEvent.decode(message) == .github(signedIn: false))
    }
}
