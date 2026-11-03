import AppKit
import RatchetKit
import SwiftUI

/// The creation conversation (screens 8, 18, 22): what the person said, the first-use notice, the steps of the setup agent, its
/// streamed reply labelled with harness and model (Retry, Copy), typed errors with their fix buttons, the draft card, and the
/// composer under it (Stop while it drafts). Everything the rules say is in `ConversationModel` and `DraftCardModel`; this view
/// draws them. The live text is its own small view (`LiveReplyView`), so a streamed word redraws nothing else.
struct CreationView: View {
    let chatID: String
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ChatHub.self) private var hub

    var body: some View {
        let conversation = hub.conversation(chatID, router: router)
        let card = hub.draftCard(chatID)
        ConversationBody(model: conversation, card: card)
            .gridArea("conversation", inset: Space.pageX)
            .task(id: chatID) { await hub.chats.follow(chatID) }
            .onChange(of: card.createdOptimizationID) { _, created in
                guard let created else { return }
                // The run started at once: show it, live.
                Task {
                    await store.refresh()
                    router.open(optimization: created)
                    hub.forget(chatID)
                }
            }
    }
}

/// The conversation itself, shared by the creation view (with its draft card) and the "Ask about a run" sheet (no card).
struct ConversationBody: View {
    @Bindable var model: ConversationModel
    var card: DraftCardModel?
    var placeholder = "Change anything in plain words, e.g. “use Grok too” or “only weekdays”."
    /// The layout area the composer is measured in; nil where there is none (a sheet).
    var probeArea: String? = "conversation"
    @Environment(AppRouter.self) private var router
    @Environment(ChatHub.self) private var hub
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    var body: some View {
        let items = model.items
        let latestReply = items.compactMap { item -> Int64? in
            if case .reply(let message) = item { message.seq } else { nil }
        }.last
        let firstUser = items.first { if case .user = $0 { true } else { false } }?.id
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: Space.lg) {
                    ForEach(items) { item in
                        row(item, latestReply: latestReply)
                        if item.id == firstUser, let notice = model.dataNotice {
                            NoticeLine(text: notice, closeLabel: "Dismiss the first-use notice") { model.dismissDataNotice() }
                        }
                    }
                    LiveReplyView(model: model, showsWorking: !items.contains { if case .step = $0 { true } else { false } })
                    if let error = model.error {
                        ChatErrorBlock(error: error) { action in Task { await model.perform(action) } }
                    }
                    if let note = model.actionError {
                        Text(note).kitType(.meta).foregroundStyle(Ink.bad)
                    }
                    if let latestReply, !model.isReplying,
                       let message = items.compactMap({ item -> ChatMessage? in
                           if case .reply(let message) = item, message.seq == latestReply { message } else { nil }
                       }).first {
                        ReplyFooter(
                            label: model.label(for: message), canRetry: model.canRetry(message),
                            retry: { Task { await model.retry() } },
                            copy: {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(model.copyText(message), forType: .string)
                            })
                    }
                    Color.clear.frame(height: Space.xs).id("bottom")
                }
                .frame(maxWidth: Layout.contentWidth, alignment: .leading)
                .padding(.horizontal, Space.pageX)
                .padding(.top, Space.xl)
                .padding(.bottom, Space.lg)
                .frame(maxWidth: .infinity, alignment: .center)
            }
            .background(Ink.canvas)
            .onChange(of: items.count) { _, _ in
                withAnimation(Motion.step) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onAppear { proxy.scrollTo("bottom", anchor: .bottom) }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            KitComposer(
                text: $model.draft, placeholder: placeholder,
                isSending: model.isReplying, canSend: model.canSend && actionsEnabled,
                onSend: { Task { await model.pressButton() } }
            ) {
                Text(ComposerModel.hint).kitType(.caption).foregroundStyle(Ink.textFaint)
            }
            .frame(maxWidth: Layout.contentWidth)
            // The floating bar spans its own width, centred: it is measured on its own, not against the column's leading edge.
            .probed("composer", area: probeArea)
            .padding(.horizontal, Space.pageX)
            .padding(.bottom, Space.lg)
            .frame(maxWidth: .infinity)
        }
    }

    @ViewBuilder
    private func row(_ item: ChatItem, latestReply: Int64?) -> some View {
        switch item {
        case .user(let message):
            UserBubble(text: message.text)
        case .step(let step):
            StepLine(step: step)
        case .reply(let message):
            VStack(alignment: .leading, spacing: Space.xs) {
                Text(message.text).kitType(.body).foregroundStyle(Ink.text)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if message.seq != latestReply, let label = model.label(for: message) {
                    Text(label.text).kitType(.caption).foregroundStyle(Ink.textFaint)
                }
            }
        case .card(let drawn):
            // The draft being talked about; a replaced draft is not drawn (its newer one is).
            if let card, drawn.id == card.card?.id {
                DraftCardView(model: card)
            }
        }
    }
}

/// The reply being written. It reads only `LiveReply`, so a streamed word invalidates this view and nothing else.
private struct LiveReplyView: View {
    let model: ConversationModel
    /// "Claude Code is working…" says it only while there are no steps to show: the steps are the progress.
    var showsWorking = true

    var body: some View {
        if let live = model.live, !live.text.isEmpty {
            Text(live.text).kitType(.body).foregroundStyle(Ink.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityLabel("Reply so far: " + live.text)
        } else if model.isReplying && showsWorking {
            HStack(spacing: Space.sm) {
                StatusShape(tone: .run).frame(height: Dim.textLine)
                Text(model.harnessLabel + " is working…").kitType(.meta).foregroundStyle(Ink.textMuted)
            }
            .accessibilityElement(children: .combine)
        }
    }
}

/// A failed turn: what happened, when it can be tried again, and its buttons ("Try Again", "Use Codex", "Sign In").
struct ChatErrorBlock: View {
    let error: ChatErrorPresentation
    let act: (ChatErrorAction) -> Void

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        HStack(alignment: .top, spacing: Space.md) {
            KitIconView(icon: .alert).foregroundStyle(Ink.bad).padding(.top, Space.xs)
            VStack(alignment: .leading, spacing: Space.xs) {
                Text(error.title).kitType(.bodyStrong).foregroundStyle(Ink.text)
                if !error.message.isEmpty {
                    Text(error.message).kitType(.meta).foregroundStyle(Ink.textMuted).fixedSize(horizontal: false, vertical: true)
                }
                if let note = error.note {
                    Text(note).kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: Space.sm) {
                ForEach(error.actions) { action in
                    Button(action.title) { act(action) }
                        .buttonStyle(.kit(.quiet, small: true))
                        .disabled(!action.isEnabled)
                }
            }
        }
        .padding(Space.lg)
        .background(shape.fill(Ink.badWash))
        .overlay(shape.strokeBorder(Ink.bad.opacity(0.5), lineWidth: Dim.hairline))
        .accessibilityElement(children: .contain)
        .onAppear { AccessibilityNotification.Announcement(error.title).post() }
    }
}

extension View {
    /// A layout probe in `area`, or none when the view is not in a measured area (a sheet).
    @ViewBuilder
    func probed(_ name: String, area: String?) -> some View {
        if let area { gridProbe(name, area: area, edge: nil) } else { self }
    }
}
