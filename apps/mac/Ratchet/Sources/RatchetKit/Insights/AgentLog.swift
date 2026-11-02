import Foundation

/// One line of an agent's log (`GET /v1/agents/{id}/events`).
public struct AgentLogLine: Identifiable, Codable, Hashable, Sendable {
    public var id: Int64
    public var at: Date
    /// "status", "message", "verdict", "tool"...: how the line is tinted.
    public var kind: String
    public var text: String

    public init(id: Int64, at: Date, kind: String, text: String) {
        self.id = id
        self.at = at
        self.kind = kind
        self.text = text
    }
}

/// A page of the log, oldest first. `cursor` goes back as the `cursor` of the next request to read only newer lines.
public struct AgentLogPage: Hashable, Sendable {
    public var lines: [AgentLogLine]
    public var cursor: Int64

    public init(lines: [AgentLogLine], cursor: Int64) {
        self.lines = lines
        self.cursor = cursor
    }
}

/// What the app keeps of one agent's log: the newest page first, then only what came after the cursor, the newest 300 lines.
public struct AgentLog: Hashable, Sendable {
    public static let capacity = 300

    public var lines: [AgentLogLine] = []
    public var cursor: Int64?
    /// True once the first page arrived (an empty log is "No events yet", not "loading").
    public var isLoaded = false
    public var error: String?

    public init() {}

    /// Appends a page. An empty page keeps the cursor; lines already shown are skipped, so a page read twice adds nothing.
    public mutating func apply(_ page: AgentLogPage) {
        isLoaded = true
        error = nil
        guard !page.lines.isEmpty else { return }
        let known = lines.last?.id ?? Int64.min
        lines.append(contentsOf: page.lines.filter { $0.id > known })
        if lines.count > Self.capacity { lines.removeFirst(lines.count - Self.capacity) }
        cursor = page.cursor
    }

    public mutating func fail(_ message: String) {
        isLoaded = true
        error = message
    }
}
