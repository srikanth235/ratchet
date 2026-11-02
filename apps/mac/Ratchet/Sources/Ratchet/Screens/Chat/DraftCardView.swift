import RatchetKit
import SwiftUI

/// The draft card of the creation conversation (screens 8 and 18): Measures, Agents, Each run, Schedule, Output, Config. Each
/// editable row has its own button ("Change agents") that opens a popover; a change is said to the chat, the card updates, and the
/// row shows "Updated" with Undo. Start Over, Create and Run Once · ≈$38 and Create Optimization are at the foot; nothing waits
/// on a human after them.
struct DraftCardView: View {
    let model: DraftCardModel
    @Environment(ChatHub.self) private var hub
    @Environment(AppRouter.self) private var router
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        let failed = model.failure != nil
        VStack(alignment: .leading, spacing: 0) {
            header
            if let failure = model.failure {
                failureBlock(failure)
            }
            ForEach(model.rows) { state in
                DraftRowView(state: state, model: model)
            }
            footer
        }
        .background(shape.fill(Ink.raised.opacity(0.5)))
        .overlay(shape.strokeBorder(failed ? Ink.bad.opacity(0.6) : Ink.hairlineStrong, lineWidth: Dim.hairline))
        .clipShape(shape)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Draft optimization: " + model.title)
        .gridProbe("draft-card", area: "conversation")
    }

    private var header: some View {
        HStack(spacing: Space.md) {
            RatchetMarkView(height: Dim.mark)
            Text(model.title).kitType(.bodyStrong).foregroundStyle(Ink.text)
            Spacer(minLength: Space.md)
            if let subtitle = model.subtitle {
                Text(subtitle).kitType(.meta).foregroundStyle(Ink.textFaint)
            }
        }
        .padding(.horizontal, Space.lg)
        .padding(.vertical, Space.md)
        .overlay(alignment: .bottom) { Hairline() }
    }

    private func failureBlock(_ failure: (title: String, message: String)) -> some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
        return HStack(alignment: .top, spacing: Space.md) {
            KitIconView(icon: .alert).foregroundStyle(Ink.bad).padding(.top, Space.xs)
            VStack(alignment: .leading, spacing: Space.xs) {
                Text(failure.title).kitType(.bodyStrong).foregroundStyle(Ink.text)
                Text(failure.message).kitType(.meta).foregroundStyle(Ink.textMuted).fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: Space.sm) {
                ForEach(model.fixes) { fix in
                    Button(fix.title) { Task { await model.apply(fix) } }
                        .buttonStyle(.kit(.quiet, small: true))
                        .disabled(model.isDrafting)
                }
            }
        }
        .padding(Space.md)
        .background(shape.fill(Ink.badWash))
        .overlay(shape.strokeBorder(Ink.bad.opacity(0.5), lineWidth: Dim.hairline))
        .padding(Space.md)
        .accessibilityElement(children: .contain)
    }

    private var footer: some View {
        HStack(alignment: .center, spacing: Space.md) {
            Text("Runs at once from \(Text("ratchet/config").font(KitType.monoStrong.font)). A PR adopts it into main later.")
                .kitType(.meta).foregroundStyle(Ink.textFaint)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            if model.status == .created {
                Text("Created. Opening it…").kitType(.meta).foregroundStyle(Ink.ok)
            }
            Button {
                Task { await hub.startOver(chatID: model.chatID, router: router) }
            } label: {
                HStack(spacing: Space.xs) {
                    KitIconView(icon: .undo, size: Dim.icon - Space.xs)
                    Text("Start Over")
                }
            }
            .buttonStyle(.kit(.ghost))
            .help("End this conversation and go back to Home with your first words")
            Button(model.createAndRunOnceTitle) { Task { await model.createAndRunOnce() } }
                .buttonStyle(.kit(.quiet))
                .disabled(!model.canCreate || !actionsEnabled)
                .help("Create it and run it once now. No schedule is added.")
            Button(model.createTitle) { Task { await model.create() } }
                .buttonStyle(.kit(.primary))
                .disabled(!model.canCreate || !actionsEnabled)
                .help("Create it. The first run starts at once; nothing waits on a pull request.")
        }
        .padding(.horizontal, Space.lg)
        .padding(.vertical, Space.md)
    }
}

/// One row of the card: its label, its value, "Updated" with Undo after a change, and its own button.
private struct DraftRowView: View {
    let state: DraftRowState
    let model: DraftCardModel
    @State private var open = false
    @State private var edit: DraftRowEdit?
    @State private var typed = ""
    @State private var focus = SnapshotFocus.shared

    private var row: ChatCardRow { state.row }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.md) {
            Text(row.label).kitType(.meta).foregroundStyle(Ink.textFaint)
                .frame(width: Layout.labelColumnCompact, alignment: .leading)
            Text(row.value)
                .kitType(row.key == "config" ? .mono : .body)
                .foregroundStyle(Ink.text)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            if state.isUpdated {
                HStack(spacing: Space.sm) {
                    StatusShape(tone: .ok, size: Dim.icon - Space.xs)
                    Text("Updated").kitType(.caption).foregroundStyle(Ink.ok)
                    Button("Undo") { Task { await model.undo(row.key) } }
                        .buttonStyle(.kit(.link, small: true))
                        .help("Put \(row.label) back as it was")
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(row.label) updated")
            }
            if state.isEditable {
                Button(state.changeTitle) {
                    edit = model.editor(for: row.key)
                    typed = ""
                    open = true
                }
                .buttonStyle(.kit(.quiet, small: true))
                .disabled(!model.canCreate && model.status != .failed)
                .popover(isPresented: $open, arrowEdge: .trailing) { popover }
                .task(id: focus.popover) {
                    await SnapshotFocus.opens(state.changeTitle, focus: focus) {
                        edit = model.editor(for: row.key)
                        open = true
                    }
                }
            }
        }
        .padding(.horizontal, Space.lg)
        .padding(.vertical, Space.sm)
        .frame(minHeight: Dim.buttonHeight + Space.sm * 2)
        .overlay(alignment: .bottom) { Hairline() }
        .accessibilityElement(children: .contain)
    }

    /// Done closes the popover and says the change, only when it is one.
    private func finish() {
        open = false
        guard let edit, edit != model.editor(for: row.key) else { return }
        Task { await model.edit(row.key, edit) }
    }

    @ViewBuilder
    private var popover: some View {
        switch edit {
        case .agents?:
            AgentOrderPopover(
                editor: Binding(
                    get: { if case .agents(let editor)? = edit { editor } else { AgentOrderEditor(order: [.claudeCode]) } },
                    set: { edit = .agents($0) }),
                done: finish)
        case .limits?:
            LimitsPopover(
                editor: Binding(
                    get: { if case .limits(let editor)? = edit { editor } else { LimitsEditor() } },
                    set: { edit = .limits($0) }),
                done: finish)
        case .schedule?:
            SchedulePopover(
                kind: Binding(
                    get: { if case .schedule(let kind)? = edit { kind } else { nil } },
                    set: { if let kind = $0 { edit = .schedule(kind) } }),
                done: finish)
        case .option?:
            ChoicePopover(
                title: state.changeTitle, options: row.options.map(\.label), selected: selectedOption, noneTitle: nil,
                onSelect: { label in
                    if let label, let option = row.options.first(where: { $0.label == label }) {
                        edit = .option(rowLabel: row.label, option: option)
                    }
                },
                done: finish)
        case .text?, nil:
            PopoverFrame(state.changeTitle, doneTitle: "Send", done: {
                edit = typed.isEmpty ? nil : .text(rowLabel: row.label, typed)
                finish()
            }) {
                TextField("Say what to change, in plain words", text: $typed, axis: .vertical)
                    .kitField()
                    .onSubmit {
                        edit = typed.isEmpty ? nil : .text(rowLabel: row.label, typed)
                        finish()
                    }
            }
        }
    }

    private var selectedOption: String? {
        if case .option(_, let option)? = edit { return option.label }
        return nil
    }
}
