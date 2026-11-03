import RatchetKit
import SwiftUI

/// "Ask…" on a run: a conversation about this optimization with the default agent, in a sheet. The chat is bound to the
/// optimization, so the daemon gives the agent the run's context. It is the same conversation view as the creation chat, with
/// no draft card; Done, or Escape, closes it and the focus goes back to the button that opened it.
struct AskRunSheet: View {
    let optimizationID: String
    let title: String
    @Environment(AppRouter.self) private var router
    @Environment(ChatHub.self) private var hub
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: Space.md) {
                Text("Ask about " + title).kitType(.section).foregroundStyle(Ink.text).lineLimit(1)
                    .accessibilityAddTraits(.isHeader)
                Spacer(minLength: Space.md)
                Button("Done") { dismiss() }
                    .buttonStyle(.kit(.quiet, small: true))
                    .keyboardShortcut(.cancelAction)
                    .help("Close (Escape)")
            }
            .padding(.horizontal, Space.lg)
            .padding(.vertical, Space.md)
            Hairline()
            if let model = hub.askConversation(optimizationID, router: router) {
                ConversationBody(
                    model: model, card: nil,
                    placeholder: "Ask about this run, e.g. “why was the last candidate blocked?”", probeArea: nil)
                    .task(id: model.chatID) { await hub.chats.follow(model.chatID) }
            }
        }
        .frame(width: Layout.sheetWidth, height: Layout.conversationSheetHeight)
    }
}
