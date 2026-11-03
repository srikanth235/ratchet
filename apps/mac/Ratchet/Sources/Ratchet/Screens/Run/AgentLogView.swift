import RatchetKit
import SwiftUI

/// The lines of one agent's log in a quiet box that follows the newest line unless the reader scrolled away.
struct AgentLogView: View {
    let agentID: String
    let state: AgentState
    @Environment(AppStore.self) private var store
    /// True while the reader is at the bottom: a new line scrolls into view then, and never pulls a reader who scrolled up back down.
    @State private var followsNewest = true

    private var log: AgentLog { store.insight.logModel(agentID).log }

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
                        LogEntryRow(agent: nil, line: line).id(line.id)
                    }
                }
                .padding(Space.md)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(height: Space.section * 6)
            .background(shape.fill(Ink.raised.opacity(0.4)))
            .overlay(shape.strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
            // Where the reader is when scrolling stops decides whether new lines are followed.
            .onScrollPhaseChange { _, phase, context in
                guard phase == .idle else { return }
                let geometry = context.geometry
                followsNewest = geometry.contentOffset.y + geometry.containerSize.height >= geometry.contentSize.height - Space.lg
            }
            .onChange(of: log.lines.last?.id) { _, newest in
                if followsNewest, let newest { proxy.scrollTo(newest, anchor: .bottom) }
            }
            .task(id: agentID) {
                followsNewest = true
                await store.insight.loadAgentLog(agentID)
                if let newest = log.lines.last?.id { proxy.scrollTo(newest, anchor: .bottom) }
            }
        }
        .accessibilityLabel("Agent log")
    }
}
