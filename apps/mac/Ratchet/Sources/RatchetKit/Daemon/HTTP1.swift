import Foundation

// A small HTTP/1.1 codec for the unix socket of `ratchetd`: it writes one request and parses one response
// (Content-Length, chunked, or until the connection closes). Pure bytes in, events out, so the tests cover it
// with every way of splitting the input. The socket itself is in UnixSocket.swift.

/// One header line.
struct HTTP1Field: Sendable, Equatable {
    var name: String
    var value: String
}

/// Status line and headers of a response.
struct HTTP1ResponseHead: Sendable, Equatable {
    var status: Int
    var reason: String
    var fields: [HTTP1Field]

    /// The first header with this name, compared without regard to case.
    func value(for name: String) -> String? {
        fields.first { $0.name.caseInsensitiveCompare(name) == .orderedSame }?.value
    }
}

enum HTTP1Request {
    /// "GET /v1/status HTTP/1.1", the headers, a blank line, the body. CR and LF inside a header are dropped.
    static func encode(method: String, target: String, fields: [HTTP1Field], body: [UInt8]) -> [UInt8] {
        func clean(_ text: String) -> String {
            text.replacingOccurrences(of: "\r", with: "").replacingOccurrences(of: "\n", with: "")
        }
        var head = "\(clean(method)) \(clean(target)) HTTP/1.1\r\n"
        for field in fields {
            head += clean(field.name) + ": " + clean(field.value) + "\r\n"
        }
        head += "\r\n"
        return Array(head.utf8) + body
    }
}

/// Incremental parser of one response. Feed it what the socket delivers; it returns what became complete.
struct HTTP1ResponseParser {
    enum Event: Sendable, Equatable {
        case head(HTTP1ResponseHead)
        case body([UInt8])
        case end
    }

    enum ParseError: Error, Equatable, LocalizedError {
        case malformed(String)
        /// The connection closed before the response was complete.
        case truncated

        var errorDescription: String? {
            switch self {
            case .malformed(let why): "The daemon sent a response that is not valid HTTP (\(why))."
            case .truncated: "The daemon closed the connection before it finished answering."
            }
        }
    }

    private enum State {
        case head
        case fixed(remaining: Int)
        case chunkSize
        case chunkData(remaining: Int)
        case chunkDataEnd
        case trailers
        case untilClose
        case done
    }

    private static let maxHeadBytes = 64 * 1024

    private var buffer: [UInt8] = []
    private var state: State = .head

    init() {}

    var isDone: Bool {
        if case .done = state { return true }
        return false
    }

    mutating func feed(_ bytes: [UInt8]) throws -> [Event] {
        buffer.append(contentsOf: bytes)
        return try advance()
    }

    /// The connection closed. Fine for a body that runs until close; otherwise the response was cut short.
    mutating func finish() throws -> [Event] {
        switch state {
        case .done:
            return []
        case .untilClose:
            state = .done
            return [.end]
        default:
            throw ParseError.truncated
        }
    }

    // MARK: Parsing

    private mutating func advance() throws -> [Event] {
        var events: [Event] = []
        loop: while true {
            switch state {
            case .head:
                guard let end = Self.find([13, 10, 13, 10], in: buffer) else {
                    if buffer.count > Self.maxHeadBytes { throw ParseError.malformed("the head is too large") }
                    break loop
                }
                let head = try Self.parseHead(Array(buffer[0..<end]))
                buffer.removeSubrange(0..<(end + 4))
                // 1xx answers ("100 Continue") are not the response.
                if head.status >= 100 && head.status < 200 && head.status != 101 { continue loop }
                events.append(.head(head))
                state = try Self.framing(of: head)

            case .fixed(let remaining):
                if remaining == 0 {
                    state = .done
                    events.append(.end)
                    break loop
                }
                if buffer.isEmpty { break loop }
                let count = min(remaining, buffer.count)
                events.append(.body(Array(buffer[0..<count])))
                buffer.removeSubrange(0..<count)
                state = .fixed(remaining: remaining - count)

            case .untilClose:
                if buffer.isEmpty { break loop }
                events.append(.body(buffer))
                buffer.removeAll(keepingCapacity: true)
                break loop

            case .chunkSize:
                guard let end = Self.find([13, 10], in: buffer) else {
                    if buffer.count > 1024 { throw ParseError.malformed("a chunk size line is too long") }
                    break loop
                }
                let line = String(decoding: buffer[0..<end], as: UTF8.self)
                buffer.removeSubrange(0..<(end + 2))
                // "1a;name=value": the size is the hex number before any extension.
                let digits = line.split(separator: ";", maxSplits: 1, omittingEmptySubsequences: false).first.map {
                    $0.trimmingCharacters(in: .whitespaces)
                } ?? ""
                guard let size = Int(digits, radix: 16), size >= 0 else {
                    throw ParseError.malformed("bad chunk size \"\(digits)\"")
                }
                state = size == 0 ? .trailers : .chunkData(remaining: size)

            case .chunkData(let remaining):
                if buffer.isEmpty { break loop }
                let count = min(remaining, buffer.count)
                events.append(.body(Array(buffer[0..<count])))
                buffer.removeSubrange(0..<count)
                state = count == remaining ? .chunkDataEnd : .chunkData(remaining: remaining - count)

            case .chunkDataEnd:
                if buffer.count < 2 { break loop }
                guard buffer[0] == 13, buffer[1] == 10 else { throw ParseError.malformed("a chunk does not end with CRLF") }
                buffer.removeSubrange(0..<2)
                state = .chunkSize

            case .trailers:
                // Trailer lines are ignored; a blank line ends the message.
                guard let end = Self.find([13, 10], in: buffer) else { break loop }
                buffer.removeSubrange(0..<(end + 2))
                if end == 0 {
                    state = .done
                    events.append(.end)
                    break loop
                }

            case .done:
                buffer.removeAll()
                break loop
            }
        }
        return events
    }

    private static func framing(of head: HTTP1ResponseHead) throws -> State {
        if head.status == 204 || head.status == 304 || head.status == 101 { return .fixed(remaining: 0) }
        if let encoding = head.value(for: "Transfer-Encoding"), encoding.lowercased().contains("chunked") {
            return .chunkSize
        }
        if let length = head.value(for: "Content-Length") {
            guard let count = Int(length.trimmingCharacters(in: .whitespaces)), count >= 0 else {
                throw ParseError.malformed("bad Content-Length \"\(length)\"")
            }
            return .fixed(remaining: count)
        }
        return .untilClose
    }

    private static func parseHead(_ bytes: [UInt8]) throws -> HTTP1ResponseHead {
        let text = String(decoding: bytes, as: UTF8.self)
        let lines = text.components(separatedBy: "\r\n")
        guard let statusLine = lines.first else { throw ParseError.malformed("no status line") }
        // "HTTP/1.1 200 OK"
        let parts = statusLine.split(separator: " ", maxSplits: 2, omittingEmptySubsequences: true)
        guard parts.count >= 2, parts[0].hasPrefix("HTTP/"), let status = Int(parts[1]) else {
            throw ParseError.malformed("bad status line \"\(statusLine)\"")
        }
        let reason = parts.count > 2 ? String(parts[2]) : ""
        var fields: [HTTP1Field] = []
        for line in lines.dropFirst() where !line.isEmpty {
            guard let colon = line.firstIndex(of: ":") else { throw ParseError.malformed("bad header \"\(line)\"") }
            let name = String(line[line.startIndex..<colon]).trimmingCharacters(in: .whitespaces)
            let value = String(line[line.index(after: colon)...]).trimmingCharacters(in: .whitespaces)
            fields.append(HTTP1Field(name: name, value: value))
        }
        return HTTP1ResponseHead(status: status, reason: reason, fields: fields)
    }

    /// The index of the first occurrence of `pattern`.
    private static func find(_ pattern: [UInt8], in bytes: [UInt8]) -> Int? {
        guard !pattern.isEmpty, bytes.count >= pattern.count else { return nil }
        let last = bytes.count - pattern.count
        var i = 0
        while i <= last {
            if bytes[i] == pattern[0] {
                var match = true
                var j = 1
                while j < pattern.count {
                    if bytes[i + j] != pattern[j] { match = false; break }
                    j += 1
                }
                if match { return i }
            }
            i += 1
        }
        return nil
    }
}
