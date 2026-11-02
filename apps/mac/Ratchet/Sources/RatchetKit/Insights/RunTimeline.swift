import Foundation

/// One entry of the run's Overview: what happened, once, in time order (screen 2). Pure values, so the rules (what is kept, what
/// is the same event seen by two agents, what becomes a verdict or a blocked card) are tested without a view.
public enum RunTimelineEntry: Identifiable, Hashable, Sendable {
    /// A sentence of an agent's: its status or its message. `agent` is set only when several agents work and the line is theirs alone.
    case line(id: String, at: Date, agent: String?, text: String, isMessage: Bool)
    /// The evaluator sealed a candidate: "7c02e9d at 351 ms".
    case verified(id: String, at: Date, agent: String?, text: String)
    /// The candidate did not count: it touched a protected path, or never got measured.
    case blocked(id: String, at: Date, agent: String?, text: String)
    /// A candidate that was measured and did not beat the noise: a quiet line, not a card.
    case noise(id: String, at: Date, agent: String?, text: String)
    /// A failure the agent reported.
    case failure(id: String, at: Date, agent: String?, text: String)
    /// The run's own harness changes: waiting, switching, pausing, resuming.
    case harness(id: String, at: Date, event: HarnessLogEvent)

    public var id: String {
        switch self {
        case .line(let id, _, _, _, _), .verified(let id, _, _, _), .blocked(let id, _, _, _), .noise(let id, _, _, _),
            .failure(let id, _, _, _), .harness(let id, _, _):
            id
        }
    }

    public var at: Date {
        switch self {
        case .line(_, let at, _, _, _), .verified(_, let at, _, _), .blocked(_, let at, _, _), .noise(_, let at, _, _),
            .failure(_, let at, _, _), .harness(_, let at, _):
            at
        }
    }
}

public enum RunTimeline {
    /// What the Overview keeps: statuses, messages, verdicts and errors. Tool calls and the rest are in Everything.
    public static let overviewKinds: Set<String> = ["status", "message", "verdict", "error"]

    /// Two lines of the same kind with the same words, a few seconds apart, are one event that more than one agent reported.
    public static let sameEventWindow: TimeInterval = 5

    /// The Overview: every agent's lines in time order, each event once. `agents` are (name, lines) pairs. The agent's name rides
    /// along only when several agents work and the event was reported by one of them.
    public static func overview(agents: [(name: String, lines: [AgentLogLine])]) -> [RunTimelineEntry] {
        struct Raw {
            var agent: String
            var line: AgentLogLine
        }
        var raws: [Raw] = []
        for agent in agents {
            for line in agent.lines where overviewKinds.contains(line.kind) { raws.append(Raw(agent: agent.name, line: line)) }
        }
        raws.sort { (a: Raw, b: Raw) -> Bool in
            if a.line.at != b.line.at { return a.line.at < b.line.at }
            if a.agent != b.agent { return a.agent < b.agent }
            return a.line.id < b.line.id
        }

        // The same event, read once: later copies of a line already seen within the window are dropped.
        var kept: [Raw] = []
        var seen: [String: [Date]] = [:]
        var sharedBy: [String: Int] = [:]
        for raw in raws {
            let key = raw.line.kind + "|" + raw.line.text
            if let dates = seen[key], dates.contains(where: { abs(raw.line.at.timeIntervalSince($0)) <= sameEventWindow }) {
                sharedBy[key, default: 1] += 1
                continue
            }
            seen[key, default: []].append(raw.line.at)
            kept.append(raw)
        }

        let several = agents.count > 1
        var entries: [RunTimelineEntry] = []
        for raw in kept {
            let line = raw.line
            let key = line.kind + "|" + line.text
            // An event every agent reported belongs to the run, not to one agent.
            var owner: String?
            if several {
                let reporters: Int = sharedBy[key] ?? 1
                if reporters < agents.count { owner = raw.agent }
            }
            let id: String = raw.agent + "-" + String(line.id)
            let text: String = line.text
            let lower: String = text.lowercased()
            let at: Date = line.at
            var entry: RunTimelineEntry
            if let event = line.harnessEvent {
                entry = RunTimelineEntry.harness(id: id, at: at, event: event)
            } else if line.kind == "verdict" {
                if lower.hasPrefix("verified") {
                    entry = RunTimelineEntry.verified(id: id, at: at, agent: owner, text: detail(of: text))
                } else if lower.hasPrefix("blocked") {
                    entry = RunTimelineEntry.blocked(id: id, at: at, agent: owner, text: detail(of: text))
                } else {
                    entry = RunTimelineEntry.noise(id: id, at: at, agent: owner, text: text)
                }
            } else if line.kind == "error" {
                if lower.hasPrefix("blocked") {
                    entry = RunTimelineEntry.blocked(id: id, at: at, agent: owner, text: detail(of: text))
                } else {
                    entry = RunTimelineEntry.failure(id: id, at: at, agent: owner, text: text)
                }
            } else {
                entry = RunTimelineEntry.line(id: id, at: at, agent: owner, text: text, isMessage: line.kind == "message")
            }
            entries.append(entry)
        }
        return entries
    }

    /// "verified: 7c02e9d at 351 ms" becomes "7c02e9d at 351 ms"; a line with no colon stays as it is.
    static func detail(of text: String) -> String {
        guard let colon = text.firstIndex(of: ":") else { return text }
        let rest = text[text.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        return rest.isEmpty ? text : rest.prefix(1).uppercased() + rest.dropFirst()
    }
}
