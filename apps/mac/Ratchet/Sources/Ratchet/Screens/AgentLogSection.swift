import RatchetKit
import SwiftUI

/// The live log of the agents of a running optimization: one agent at a time, newest lines at the bottom, appended as the
/// daemon announces `agent-event`. The store pages it (`GET /v1/agents/{id}/events`); this view only shows it.
struct AgentLogSection: View {
    let agents: [Agent]
    @Environment(AppStore.self) private var store
    @State private var chosen: String?

    private var current: Agent? {
        agents.first { $0.id == chosen } ?? agents.first
    }

    var body: some View {
        KitSection("Agent log") {
            if let agent = current {
                VStack(alignment: .leading, spacing: Space.md) {
                    if agents.count > 1 {
                        KitSwitch(items: agents, selection: agent, title: { $0.name }, onSelect: { chosen = $0.id })
                    }
                    AgentLogView(agentID: agent.id, state: agent.state)
                        .id(agent.id)
                }
            } else {
                Text("No agents yet.").kitType(.body).foregroundStyle(Ink.textMuted)
            }
        }
        .id("log")
    }
}

/// The lines of one agent's log in a quiet box that follows the newest line unless the reader scrolled away.
struct AgentLogView: View {
    let agentID: String
    let state: AgentState
    @Environment(AppStore.self) private var store

    private var log: AgentLog { store.insight.logs[agentID] ?? AgentLog() }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: Space.xs) {
                    if let error = log.error {
                        Text(error).kitType(.mono).foregroundStyle(Ink.bad)
                    }
                    if log.lines.isEmpty && log.error == nil {
                        Text(log.isLoaded ? "No events yet." : "Loading…").kitType(.meta).foregroundStyle(Ink.textFaint)
                    }
                    ForEach(log.lines) { line in
                        AgentLogRow(line: line).id(line.id)
                    }
                }
                .padding(Space.md)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(height: Space.section * 6)
            .background(shape.fill(Ink.raised.opacity(0.4)))
            .overlay(shape.strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
            .onChange(of: log.lines.last?.id) { _, newest in
                if let newest { proxy.scrollTo(newest, anchor: .bottom) }
            }
            .task(id: agentID) {
                await store.loadAgentLog(agentID)
                if let newest = log.lines.last?.id { proxy.scrollTo(newest, anchor: .bottom) }
            }
        }
        .accessibilityLabel("Agent log")
    }
}

private struct AgentLogRow: View {
    let line: AgentLogLine

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.md) {
            Text(Elapsed.timeOfDay(line.at)).kitType(.mono).foregroundStyle(Ink.textFaint)
                .frame(width: Space.section * 2, alignment: .leading)
            Text(line.kind).kitType(.mono).foregroundStyle(Ink.textFaint)
                .frame(width: Space.section * 2, alignment: .leading)
            Text(line.text).kitType(.mono).foregroundStyle(tone).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var tone: Color {
        let text = line.text.lowercased()
        if line.kind == "error" || text.hasPrefix("blocked") || text.hasPrefix("failed") { return Ink.bad }
        if text.hasPrefix("verified") { return Ink.ok }
        return line.kind == "message" ? Ink.text : Ink.textMuted
    }
}
