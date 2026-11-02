import RatchetKit
import SwiftUI

/// The glass field under a running optimization where the person leaves a note for the agents (screen 2). Multi-line: Return
/// adds a line, Command-Return sends. A sent note shows in the log as "Queued · read at the next candidate" with Undo, then
/// "Read by Claude Code at 02:11". The glass, the edge and the focus ring are `KitSteeringField`'s; this fills it.
struct SteeringBar: View {
    let optimizationID: String
    @Environment(ChatHub.self) private var hub
    @Environment(\.daemonActionsEnabled) private var actionsEnabled
    @FocusState private var focused: Bool

    var body: some View {
        let model = hub.steering(for: optimizationID)
        SteeringField(model: model, focused: $focused, enabled: actionsEnabled)
            .frame(maxWidth: Layout.contentWidth)
            .padding(.horizontal, Space.pageX)
            .padding(.bottom, Space.lg)
            .frame(maxWidth: .infinity)
            // The note chat starts with the first note; from then on the stream keeps its cards (the notes) current.
            .task(id: model.chatID) {
                if let chatID = model.chatID { await hub.chats.follow(chatID) }
            }
            .gridProbe("steering", area: "detail")
    }
}

private struct SteeringField: View {
    @Bindable var model: SteeringModel
    var focused: FocusState<Bool>.Binding
    let enabled: Bool

    var body: some View {
        KitSteeringField(isFocused: focused.wrappedValue) {
            VStack(alignment: .leading, spacing: Space.sm) {
                ZStack(alignment: .topLeading) {
                    if model.draft.isEmpty {
                        Text(SteeringModel.placeholder)
                            .kitType(.body).foregroundStyle(Ink.textFaint)
                            .padding(.horizontal, Space.xs + 1)
                            .padding(.top, Space.sm)
                            .allowsHitTesting(false)
                            .accessibilityHidden(true)
                    }
                    TextEditor(text: $model.draft)
                        .kitType(.body)
                        .foregroundStyle(Ink.text)
                        .scrollContentBackground(.hidden)
                        .focused(focused)
                        .frame(minHeight: Space.section + Space.md, maxHeight: Space.section * 4)
                        .accessibilityLabel("Note for the agents")
                        .accessibilityHint(model.hint)
                }
                HStack(spacing: Space.md) {
                    Text(model.hint).kitType(.caption).foregroundStyle(Ink.textFaint)
                    if let failure = model.failure {
                        Text(failure).kitType(.caption).foregroundStyle(Ink.bad).lineLimit(2)
                    }
                    Spacer(minLength: 0)
                    KitIconButton(icon: .arrowUp, label: "Send note") { Task { await model.send() } }
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(!model.canSend || !enabled)
                }
            }
        }
    }
}
