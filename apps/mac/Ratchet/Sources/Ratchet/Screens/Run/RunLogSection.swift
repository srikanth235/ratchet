import RatchetKit
import SwiftUI

/// The three views of a run's log (screens 2 and 20): Overview (what the run did, step by step), By Agent (one lane each) and
/// Everything (every line of every agent, in time order). Harness waits, switches, pauses and resumes are marked in all three.
enum LogTab: String, CaseIterable, Identifiable, Hashable {
    case overview, byAgent, everything

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .byAgent: "By Agent"
        case .everything: "Everything"
        }
    }

    /// The tooltip of the segment.
    var help: String {
        switch self {
        case .overview: "What the run did, step by step"
        case .byAgent: "Agents, one lane each"
        case .everything: "Every line from every agent, in time order"
        }
    }

    /// The line beside the switch.
    var caption: String { help }
}

struct RunLogSection: View {
    let optimizationID: String
    let agents: [Agent]
    @Environment(AppStore.self) private var store
    @Environment(ChatHub.self) private var hub
    @State private var tab: LogTab = .overview
    @State private var focus = SnapshotFocus.shared

    var body: some View {
        KitSection("Log") {
            VStack(alignment: .leading, spacing: Space.md) {
                HStack(spacing: Space.md) {
                    KitSegmented(items: LogTab.allCases, selection: $tab, title: { $0.title }, help: { $0.help })
                        .fixedSize()
                    Text(tab.caption).kitType(.meta).foregroundStyle(Ink.textFaint)
                }
                .dimsWhenInactive()
                PausedNote(agentIDs: agents.map(\.id))
                switch tab {
                case .overview:
                    OverviewTimeline(agents: agents, notes: hub.steering(for: optimizationID))
                case .byAgent:
                    lanes
                case .everything:
                    MergedLog(agents: agents)
                }
            }
        }
        .id("log")
        .task(id: focus.logTab) {
            guard Store.isOverridden, let name = focus.logTab else { return }
            tab = name == "by-agent" ? .byAgent : name == "everything" ? .everything : .overview
        }
        // Every lane and the merged views read the logs of all the agents, so they are all loaded and then kept current by events.
        .task(id: agents.map(\.id)) {
            for agent in agents { await store.insight.loadAgentLog(agent.id) }
        }
    }

    /// One lane per agent: side by side when there is room, stacked when there is not.
    private var lanes: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: Space.lg) {
                ForEach(agents) { agent in AgentLane(agent: agent).frame(maxWidth: .infinity, alignment: .topLeading) }
            }
            VStack(alignment: .leading, spacing: Space.lg) {
                ForEach(agents) { agent in AgentLane(agent: agent) }
            }
        }
        .frame(minHeight: Space.section * 6)
    }
}

/// "Paused: every agent is signed out. It resumes by itself when an agent is back." while the newest harness entry says so.
private struct PausedNote: View {
    let agentIDs: [String]
    @Environment(AppStore.self) private var store

    var body: some View {
        if let reason = agentIDs.lazy.compactMap({ store.insight.logModel($0).log.pausedReason }).first {
            StatusLabel.shaped(.wait, "Paused", caption: reason + " It resumes by itself when an agent is back.")
        }
    }
}

/// One agent's lane: its name and state, and its own log, or why there is nothing yet.
private struct AgentLane: View {
    let agent: Agent
    @Environment(AppStore.self) private var store

    var body: some View {
        let log = store.insight.logModel(agent.id).log
        VStack(alignment: .leading, spacing: Space.sm) {
            HStack(spacing: Space.sm) {
                StatusShape(tone: agent.state.tone, size: Dim.icon - Space.xs)
                Text(agent.name).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
                Text(agent.state.title).kitType(.caption).foregroundStyle(Ink.textFaint)
            }
            .accessibilityElement(children: .combine)
            if agent.state == .idle && log.lines.isEmpty && log.error == nil {
                let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
                VStack(spacing: Space.xs) {
                    Text("Hasn't started yet").kitType(.body).foregroundStyle(Ink.textMuted)
                    Text("Its lane fills in when it picks up a candidate.").kitType(.meta).foregroundStyle(Ink.textFaint)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity, minHeight: Space.section * 6)
                .background(shape.fill(Ink.raised.opacity(0.4)))
                .overlay(shape.strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
            } else {
                AgentLogView(agentID: agent.id, state: agent.state)
            }
        }
    }
}

/// Everything: the lines of all agents in time order, every one, in one quiet box that follows the newest line.
private struct MergedLog: View {
    let agents: [Agent]
    @Environment(AppStore.self) private var store

    private struct Entry: Identifiable {
        let agent: String
        let line: AgentLogLine
        var id: String { agent + "-" + String(line.id) }
    }

    private var entries: [Entry] {
        agents.flatMap { agent in
            store.insight.logModel(agent.id).log.lines.map { Entry(agent: agent.name, line: $0) }
        }
        .sorted { $0.line.at < $1.line.at }
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
        let rows = entries
        VStack(alignment: .leading, spacing: Space.sm) {
            if rows.isEmpty {
                Text(agents.isEmpty ? "No agents yet." : "No events yet.").kitType(.meta).foregroundStyle(Ink.textFaint)
            }
            ForEach(rows) { entry in
                LogEntryRow(agent: agents.count > 1 ? entry.agent : nil, line: entry.line)
            }
        }
        .padding(Space.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(shape.fill(Ink.raised.opacity(0.4)))
        .overlay(shape.strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Everything")
    }
}

/// The Overview (screen 2): one timeline of what the run did, each event once, however many agents saw it. Prose in the reading
/// face, not log lines; a sealed verdict and a blocked candidate are cards; the person's own notes close it. The per-agent
/// view is By Agent, every line is Everything.
private struct OverviewTimeline: View {
    let agents: [Agent]
    let notes: SteeringModel?
    @Environment(AppStore.self) private var store

    private var entries: [RunTimelineEntry] {
        RunTimeline.overview(agents: agents.map { (name: $0.name, lines: store.insight.logModel($0.id).log.lines) })
    }

    var body: some View {
        let rows = entries
        VStack(alignment: .leading, spacing: Space.md) {
            if rows.isEmpty {
                Text(agents.isEmpty ? "No agents yet." : "No events yet.").kitType(.meta).foregroundStyle(Ink.textFaint)
            }
            ForEach(rows) { entry in TimelineRow(entry: entry) }
            if let notes {
                ForEach(notes.entries) { note in
                    SteeringNoteRow(entry: note) { Task { await notes.undo(note.id) } }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Overview")
    }
}

/// One entry of the Overview: the time in the margin, then a sentence, a quiet line, or a card.
private struct TimelineRow: View {
    let entry: RunTimelineEntry

    var body: some View {
        switch entry {
        case .harness(_, let at, let event):
            HarnessEventRow(at: at, event: event)
        case .line(_, let at, let agent, let text, let isMessage):
            row(at) { sentence(agent: agent, text: text, tone: isMessage ? Ink.text : Ink.textMuted) }
        case .noise(_, let at, let agent, let text):
            row(at) { sentence(agent: agent, text: text, tone: Ink.textFaint) }
        case .failure(_, let at, let agent, let text):
            row(at) {
                HStack(alignment: .top, spacing: Space.sm) {
                    StatusShape(tone: .bad).frame(height: Dim.lineBody)
                    sentence(agent: agent, text: text, tone: Ink.bad)
                }
            }
        case .verified(_, let at, let agent, let text):
            row(at) { card(tone: .ok, title: "Verified by the evaluator", agent: agent, text: text) }
        case .blocked(_, let at, let agent, let text):
            row(at) { card(tone: .bad, title: "Blocked before measuring", agent: agent, text: text) }
        }
    }

    private func row<Content: View>(_ at: Date, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .top, spacing: Space.md) {
            Text(Elapsed.timeOfDay(at)).kitType(.mono).foregroundStyle(Ink.textFaint)
                .frame(width: Space.section * 2, alignment: .leading)
                .frame(minHeight: Dim.lineBody, alignment: .leading)
            content().frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// "Claude Code · team key  proposes Pool size 20 → 48": the agent in the stronger face when one of several said it.
    private func sentence(agent: String?, text: String, tone: Color) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
            if let agent { Text(agent).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1).fixedSize() }
            Text(text).kitType(.body).foregroundStyle(tone).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func card(tone: StatusTone, title: String, agent: String?, text: String) -> some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        let ok = tone == .ok
        return VStack(alignment: .leading, spacing: Space.xs) {
            HStack(spacing: Space.sm) {
                StatusShape(tone: tone, size: Dim.icon - Space.xs)
                Text(title).kitType(.microStrong).foregroundStyle(ok ? Ink.ok : Ink.bad)
                if let agent { Text(agent).kitType(.micro).foregroundStyle(Ink.textFaint) }
            }
            Text(text).kitType(.body).foregroundStyle(Ink.text).textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(Space.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(shape.fill(ok ? Ink.okWash : Ink.badWash))
        .overlay(shape.strokeBorder(ok ? Ink.ok.opacity(0.5) : Ink.bad.opacity(0.5), lineWidth: Dim.hairline))
        .accessibilityElement(children: .combine)
        .accessibilityLabel(title + ". " + text)
    }
}

/// One line: the time, the agent when there are several, and the text; a harness event is marked with its own row.
struct LogEntryRow: View {
    let agent: String?
    let line: AgentLogLine

    var body: some View {
        if let event = line.harnessEvent {
            HarnessEventRow(at: line.at, event: event)
        } else {
            HStack(alignment: .firstTextBaseline, spacing: Space.md) {
                Text(Elapsed.timeOfDay(line.at)).kitType(.mono).foregroundStyle(Ink.textFaint)
                    .frame(width: Space.section * 2, alignment: .leading)
                if let agent {
                    Text(agent).kitType(.microStrong).foregroundStyle(Ink.textMuted)
                        .frame(width: Space.section * 4, alignment: .leading).lineLimit(1)
                }
                Text(line.text).kitType(.mono).foregroundStyle(Self.tone(line)).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    static func tone(_ line: AgentLogLine) -> Color {
        let text = line.text.lowercased()
        if line.kind == "error" || text.hasPrefix("blocked") || text.hasPrefix("failed") { return Ink.bad }
        if text.hasPrefix("verified") { return Ink.ok }
        return line.kind == "message" ? Ink.text : Ink.textMuted
    }
}

/// "Waiting for Claude Code", "Switched to Codex", "Paused", "Resumed with Claude Code": the run's own agent changes, marked
/// with a shape and the title, the reason under it.
struct HarnessEventRow: View {
    let at: Date
    let event: HarnessLogEvent

    private var tone: StatusTone {
        switch event {
        case .wait, .paused: .wait
        case .switched: event.needsAttention ? .bad : .run
        case .resumed: .ok
        }
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
        HStack(alignment: .top, spacing: Space.md) {
            Text(Elapsed.timeOfDay(at)).kitType(.mono).foregroundStyle(Ink.textFaint)
                .frame(width: Space.section * 2, alignment: .leading)
            StatusShape(tone: tone, size: Dim.icon - Space.xs).frame(height: Dim.textLine)
            VStack(alignment: .leading, spacing: 0) {
                Text(event.title).kitType(.bodyStrong).foregroundStyle(Ink.text)
                Text(event.detail).kitType(.meta).foregroundStyle(Ink.textMuted).fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, Space.sm)
        .padding(.vertical, Space.xs)
        .background(shape.fill(Ink.waitWash))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(event.title). \(event.detail)")
    }
}

/// "You · “focus on database queries” Queued · read at the next candidate  Undo", then "Read by Claude Code at 02:11".
struct SteeringNoteRow: View {
    let entry: SteeringEntry
    let undo: () -> Void

    private var tone: StatusTone {
        switch entry.status {
        case .queued: .wait
        case .read: .ok
        case .withdrawn: .off
        }
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.md) {
            Text("You").kitType(.bodyStrong).foregroundStyle(Ink.text)
            Text("“" + entry.text + "”").kitType(.body).foregroundStyle(Ink.textMuted).lineLimit(3)
            StatusLabel.shaped(tone, entry.statusLine)
            if entry.canUndo {
                Button(action: undo) {
                    HStack(spacing: Space.xs) {
                        KitIconView(icon: .undo, size: Dim.icon - Space.xs)
                        Text("Undo")
                    }
                }
                .buttonStyle(.kit(.link, small: true))
                .help("Take this note back. It works until an agent reads it.")
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .contain)
    }
}
