import Foundation
import Testing
@testable import RatchetKit

@Suite("HTTP/1.1 codec")
struct HTTP1Tests {
    /// Parses `bytes` fed in pieces of `size` (the whole thing when 0), then closes the connection.
    private func parse(_ bytes: [UInt8], pieces size: Int = 0) throws -> (head: HTTP1ResponseHead?, body: [UInt8], ended: Bool) {
        var parser = HTTP1ResponseParser()
        var events: [HTTP1ResponseParser.Event] = []
        if size <= 0 {
            events += try parser.feed(bytes)
        } else {
            var index = 0
            while index < bytes.count {
                events += try parser.feed(Array(bytes[index..<min(index + size, bytes.count)]))
                index += size
            }
        }
        events += try parser.finish()
        var head: HTTP1ResponseHead?
        var body: [UInt8] = []
        var ended = false
        for event in events {
            switch event {
            case .head(let value): head = value
            case .body(let chunk): body += chunk
            case .end: ended = true
            }
        }
        return (head, body, ended)
    }

    private func bytes(_ text: String) -> [UInt8] { Array(text.utf8) }

    @Test func requestsAreEncoded() {
        let request = HTTP1Request.encode(
            method: "POST", target: "/v1/agents/a1/messages",
            fields: [HTTP1Field(name: "Host", value: "localhost"), HTTP1Field(name: "X-Bad", value: "a\r\nInjected: 1")],
            body: Array("{}".utf8))
        let text = String(decoding: request, as: UTF8.self)
        #expect(text.hasPrefix("POST /v1/agents/a1/messages HTTP/1.1\r\nHost: localhost\r\n"))
        #expect(text.contains("X-Bad: aInjected: 1\r\n"))
        #expect(text.hasSuffix("\r\n\r\n{}"))
    }

    @Test("Content-Length bodies, in any split", arguments: [0, 1, 2, 3, 7, 64])
    func contentLength(size: Int) throws {
        let response = bytes("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}")
        let result = try parse(response, pieces: size)
        #expect(result.head?.status == 200)
        #expect(result.head?.reason == "OK")
        #expect(result.head?.value(for: "Content-Type") == "application/json")
        #expect(String(decoding: result.body, as: UTF8.self) == "{\"ok\":true}")
        #expect(result.ended)
    }

    @Test("Chunked bodies with extensions and trailers, in any split", arguments: [0, 1, 2, 5, 13])
    func chunked(size: Int) throws {
        let response = bytes(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
                + "5\r\nhello\r\n" + "7;ext=1\r\n, world\r\n" + "0\r\nX-Trailer: yes\r\n\r\n")
        let result = try parse(response, pieces: size)
        #expect(String(decoding: result.body, as: UTF8.self) == "hello, world")
        #expect(result.ended)
    }

    @Test func chunkSizesAreHex() throws {
        let payload = String(repeating: "a", count: 26)
        let response = bytes("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1A\r\n\(payload)\r\n0\r\n\r\n")
        let result = try parse(response)
        #expect(result.body.count == 26)
    }

    @Test func noContentHasNoBody() throws {
        let result = try parse(bytes("HTTP/1.1 204 No Content\r\n\r\n"))
        #expect(result.head?.status == 204)
        #expect(result.body.isEmpty)
        #expect(result.ended)
    }

    @Test func aBodyWithoutLengthRunsUntilTheConnectionCloses() throws {
        let result = try parse(bytes("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nto the end"), pieces: 4)
        #expect(String(decoding: result.body, as: UTF8.self) == "to the end")
        #expect(result.ended)
    }

    @Test func interimResponsesAreSkipped() throws {
        let result = try parse(bytes("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nhi"), pieces: 3)
        #expect(result.head?.status == 201)
        #expect(String(decoding: result.body, as: UTF8.self) == "hi")
    }

    @Test func aResponseCutShortIsAnError() {
        #expect(throws: HTTP1ResponseParser.ParseError.truncated) {
            _ = try parse(bytes("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort"))
        }
        #expect(throws: HTTP1ResponseParser.ParseError.truncated) {
            _ = try parse(bytes("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel"))
        }
        #expect(throws: HTTP1ResponseParser.ParseError.truncated) {
            _ = try parse([])
        }
    }

    @Test func garbageIsAnError() {
        #expect(throws: HTTP1ResponseParser.ParseError.self) { _ = try parse(bytes("nonsense\r\n\r\n")) }
        #expect(throws: HTTP1ResponseParser.ParseError.self) {
            _ = try parse(bytes("HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n"))
        }
        #expect(throws: HTTP1ResponseParser.ParseError.self) {
            _ = try parse(bytes("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"))
        }
    }

    @Test func errorBodiesBecomeTypedErrors() {
        let body = Array(#"{"code":"not_found","message":"optimization x not found"}"#.utf8)
        let error = DaemonAPIError.parse(status: 404, body: body)
        #expect(error.code == "not_found")
        #expect(error.clientError == .notFound("optimization x not found"))
        let unsupported = DaemonAPIError.parse(status: 501, body: Array("not json".utf8))
        #expect(unsupported.isUnsupported)
        let fields = DaemonAPIError.parse(
            status: 400, body: Array(#"{"code":"invalid_request","message":"bad","fields":[{"path":"budgetUsd","message":"must be positive"}]}"#.utf8))
        #expect(fields.fields == ["budgetUsd: must be positive"])
        #expect(fields.clientError == .invalid("bad (budgetUsd: must be positive)"))
    }
}
