import Foundation

/// Why a run moved to another harness.
public enum HarnessSwitchCause: String, Hashable, Sendable {
    case rateLimited = "rate-limited"
    case authExpired = "auth-expired"
    case crashing, unavailable
}

/// An entry of the run log about harnesses: the fallback rules at work (`AGENT_HARNESS_PLAN.md` 4.6). Every one is a `status`
/// line of the agent's log; the structured form (`data.harness`) is not on the REST log yet, so these are read from the
/// sentences the runner writes. A line that does not match is just a line.
public enum HarnessLogEvent: Hashable, Sendable {
    /// A short rate limit: waiting for it to lift on the same harness.
    case wait(harness: Harness, detail: String)
    /// Moved on to the next harness at the next turn.
    case switched(from: Harness, to: Harness, cause: HarnessSwitchCause?, reason: String)
    /// Every harness is unavailable: paused, looking again by itself.
    case paused(reason: String)
    /// A harness is back.
    case resumed(harness: Harness)

    /// "Waiting for Claude Code", "Switched to Codex", "Paused", "Resumed with Claude Code".
    public var title: String {
        switch self {
        case .wait(let harness, _): "Waiting for " + harness.title
        case .switched(_, let to, _, _): "Switched to " + to.title
        case .paused: "Paused"
        case .resumed(let harness): "Resumed with " + harness.title
        }
    }

    /// The line under the title: "Claude Code is rate limited", "Waiting 5 min for the limit to lift".
    public var detail: String {
        switch self {
        case .wait(let harness, let detail): harness.title + " is rate limited. Waiting " + detail + " for it to lift."
        case .switched(_, _, _, let reason): reason
        case .paused(let reason): reason + " It resumes by itself when an agent is back."
        case .resumed: "An agent is available again."
        }
    }

    /// A harness change a person might want to know about (the others are routine).
    public var needsAttention: Bool {
        switch self {
        case .paused: true
        case .switched(_, _, let cause, _): cause == .authExpired
        default: false
        }
    }

    private static func harness(named name: String) -> Harness? {
        Harness.allCases.first { $0.title == name.trimmingCharacters(in: .whitespaces) }
    }

    /// Reads one log line; nil when it is not about harnesses.
    public init?(text: String) {
        if text.hasPrefix("switching from "), let colon = text.firstIndex(of: ":") {
            let head = text[text.index(text.startIndex, offsetBy: "switching from ".count)..<colon]
            let pair = head.components(separatedBy: " to ")
            guard pair.count == 2, let from = Self.harness(named: pair[0]), let to = Self.harness(named: pair[1]) else { return nil }
            let reason = String(text[text.index(after: colon)...]).trimmingCharacters(in: .whitespaces)
            self = .switched(from: from, to: to, cause: Self.cause(of: reason), reason: reason.prefix(1).uppercased() + reason.dropFirst())
        } else if text.hasPrefix("resumed with ") {
            guard let found = Self.harness(named: String(text.dropFirst("resumed with ".count))) else { return nil }
            self = .resumed(harness: found)
        } else if text.hasPrefix("paused: ") {
            var reason = String(text.dropFirst("paused: ".count))
            if let again = reason.range(of: "; checking again") { reason = String(reason[..<again.lowerBound]) }
            self = .paused(reason: reason.prefix(1).uppercased() + reason.dropFirst())
        } else if let limited = text.range(of: " is rate limited; waiting "), let tail = text.range(of: " for it to lift") {
            guard let found = Self.harness(named: String(text[..<limited.lowerBound])) else { return nil }
            self = .wait(harness: found, detail: String(text[limited.upperBound..<tail.lowerBound]))
        } else {
            return nil
        }
    }

    private static func cause(of reason: String) -> HarnessSwitchCause? {
        let lower = reason.lowercased()
        if lower.contains("rate limited") { return .rateLimited }
        if lower.contains("sign-in expired") { return .authExpired }
        if lower.contains("kept failing") { return .crashing }
        if lower.contains("not available") { return .unavailable }
        return nil
    }
}

/// A harness entry with the line it came from.
public struct HarnessLogEntry: Identifiable, Hashable, Sendable {
    public var id: Int64
    public var at: Date
    public var event: HarnessLogEvent
}

extension AgentLogLine {
    /// The harness event this line is, if any.
    public var harnessEvent: HarnessLogEvent? {
        kind == "status" ? HarnessLogEvent(text: text) : nil
    }
}

extension AgentLog {
    /// The harness entries of the log, oldest first: what the run log shows with its own marker.
    public var harnessEntries: [HarnessLogEntry] {
        lines.compactMap { line in line.harnessEvent.map { HarnessLogEntry(id: line.id, at: line.at, event: $0) } }
    }

    /// Why the run is paused, while it is: its newest harness entry is a pause.
    public var pausedReason: String? {
        if case .paused(let reason)? = harnessEntries.last?.event { return reason }
        return nil
    }

    /// The harness the run is on now, from the newest switch or resume; nil when the log says nothing.
    public var activeHarness: Harness? {
        for entry in harnessEntries.reversed() {
            switch entry.event {
            case .switched(_, let to, _, _): return to
            case .resumed(let harness): return harness
            default: continue
            }
        }
        return nil
    }
}
