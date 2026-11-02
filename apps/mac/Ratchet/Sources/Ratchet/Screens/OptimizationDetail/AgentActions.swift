import AppKit
import RatchetKit
import SwiftUI

/// Pause or resume one agent, and send it a message.
struct AgentActions: View {
    let agent: Agent
    @Environment(AppStore.self) private var store
    @State private var composing = false
    @State private var text = ""

    var body: some View {
        HStack(spacing: Space.sm) {
            Button("Message") { composing = true }
                .buttonStyle(.kit(.link, small: true))
                .popover(isPresented: $composing) { composer }
            Button(agent.state == .paused ? "Resume" : "Pause") {
                Task {
                    if agent.state == .paused { await store.optimizationStore.resumeAgent(agent.id) } else { await store.optimizationStore.pauseAgent(agent.id) }
                }
            }
            .buttonStyle(.kit(.quiet, small: true))
            .disabled(agent.state == .idle)
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            Text("Message to " + agent.name).kitType(.section).foregroundStyle(Ink.textMuted)
            TextField("Try caching the tax table first", text: $text)
                .kitField()
                .frame(width: 280)
                .onSubmit(send)
            HStack {
                Spacer()
                Button("Send", action: send)
                    .buttonStyle(.kit(.primary, small: true))
                    .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(Space.lg)
        .background(Ink.canvas)
    }

    private func send() {
        let note = text
        guard !note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        text = ""
        composing = false
        Task { await store.optimizationStore.messageAgent(agent.id, text: note) }
    }
}
