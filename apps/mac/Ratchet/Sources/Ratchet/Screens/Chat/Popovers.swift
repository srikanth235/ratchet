import RatchetKit
import SwiftUI

// The popovers behind the Home chips, the draft card's rows and the optimization settings rows. One view each, one editor each
// (RatchetKit/Agents/Editors.swift), so "Change agents" looks and reads the same everywhere (S-102). No custom background: the
// system paints a popover.

/// One option of a small segmented control.
struct SegmentOption: Identifiable, Hashable {
    let id: String
    let title: String
}

/// A bordered segmented control over a few options chosen by id ("This Mac | Cloud").
struct ChoiceSegmented: View {
    let options: [SegmentOption]
    @Binding var selection: String

    var body: some View {
        KitSegmented(
            items: options,
            selection: Binding(
                get: { options.first { $0.id == selection } ?? options[0] },
                set: { selection = $0.id }),
            title: { $0.title })
            .fixedSize()
    }
}

/// The frame of every popover: a title, the content, and Done at the trailing edge.
struct PopoverFrame<Content: View>: View {
    let title: String
    var doneTitle = "Done"
    let done: () -> Void
    let content: Content

    init(_ title: String, doneTitle: String = "Done", done: @escaping () -> Void, @ViewBuilder content: () -> Content) {
        self.title = title
        self.doneTitle = doneTitle
        self.done = done
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            Text(title).kitType(.section).foregroundStyle(Ink.text)
                .accessibilityAddTraits(.isHeader)
            content
            HStack {
                Spacer()
                Button(doneTitle, action: done)
                    .buttonStyle(.kit(.quiet, small: true))
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(Space.lg)
        .frame(width: Layout.popoverWidth, alignment: .leading)
    }
}

// MARK: - Change agents

/// "Change agents": the agents in order with Move Up and Move Down (28 pt), the Where switch (This Mac, Cloud) and Done.
struct AgentOrderPopover: View {
    @Binding var editor: AgentOrderEditor
    /// The Where switch is part of the card and the composer; the optimization page and the Agents page order only.
    var showsWhere = true
    let done: () -> Void

    var body: some View {
        PopoverFrame("Change agents", done: done) {
            VStack(alignment: .leading, spacing: 0) {
                Hairline()
                ForEach(Array(editor.order.enumerated()), id: \.element.id) { index, harness in
                    HairlineRow(verticalPadding: Space.xs) {
                        Text("\(index + 1)").kitType(.microMono).foregroundStyle(Ink.textFaint)
                            .frame(width: Space.lg, alignment: .leading)
                        Text(harness.title).kitType(.body).foregroundStyle(Ink.text)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        KitIconButton(icon: .arrowUp, label: "Move \(harness.title) up", small: true) {
                            editor.moveUp(at: index)
                        }
                        .disabled(!editor.canMoveUp(at: index))
                        KitIconButton(icon: .chevronDown, label: "Move \(harness.title) down", small: true) {
                            editor.moveDown(at: index)
                        }
                        .disabled(!editor.canMoveDown(at: index))
                        KitIconButton(icon: .close, label: "Remove \(harness.title)", small: true) {
                            editor.remove(harness)
                        }
                        .disabled(!editor.canRemove)
                    }
                }
            }
            if !editor.addable.isEmpty {
                Menu {
                    ForEach(editor.addable) { harness in
                        Button(harness.title) { editor.add(harness) }
                    }
                } label: {
                    Text("Add agent").kitType(.meta)
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .accessibilityLabel("Add an agent to the list")
            }
            if showsWhere {
                HStack(spacing: Space.md) {
                    Text("Where").kitType(.meta).foregroundStyle(Ink.textFaint)
                    ChoiceSegmented(
                        options: Placement.allCases.map { SegmentOption(id: $0.rawValue, title: $0.switchTitle) },
                        selection: Binding(
                            get: { editor.placement.rawValue },
                            set: { editor.placement = Placement(rawValue: $0) ?? .local }))
                }
            }
            Text("If one is out of quota or signed out, the next one takes over.")
                .kitType(.meta).foregroundStyle(Ink.textFaint)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: - Change limits

/// "Change limits": the budget and the time one run may use.
struct LimitsPopover: View {
    @Binding var editor: LimitsEditor
    let done: () -> Void

    var body: some View {
        PopoverFrame("Change limits", done: done) {
            VStack(alignment: .leading, spacing: Space.md) {
                HStack {
                    Text("Budget per run").kitType(.meta).foregroundStyle(Ink.textMuted)
                    Spacer()
                    Picker("Budget per run", selection: $editor.budgetUSD) {
                        ForEach(LimitsEditor.budgetChoices, id: \.self) { Text(Money.short(Double($0))).tag($0) }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
                HStack {
                    Text("Time per run").kitType(.meta).foregroundStyle(Ink.textMuted)
                    Spacer()
                    Picker("Time per run", selection: $editor.hours) {
                        ForEach(LimitsEditor.hourChoices, id: \.self) { Text(Elapsed.hm($0 * 3600)).tag($0) }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
                Text("A run stops when it reaches either one. Wins found so far are kept.")
                    .kitType(.meta).foregroundStyle(Ink.textFaint)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: - Change schedule

/// "Change schedule": how often it runs; the first run starts at once either way.
struct SchedulePopover: View {
    @Binding var kind: ScheduleKind?
    /// Offers "Not scheduled" (an optimization page can run by hand only).
    var allowsNone = false
    let done: () -> Void

    var body: some View {
        PopoverFrame("Change schedule", done: done) {
            VStack(alignment: .leading, spacing: 0) {
                Hairline()
                if allowsNone { row(nil, "Not scheduled") }
                ForEach(ScheduleKind.allCases) { option in row(option, option.title) }
            }
        }
    }

    private func row(_ option: ScheduleKind?, _ title: String) -> some View {
        Button {
            kind = option
        } label: {
            HStack(spacing: Space.md) {
                KitIconView(icon: .check, size: Dim.icon - Space.xs, weight: 1.8)
                    .foregroundStyle(Ink.brassText)
                    .opacity(kind == option ? 1 : 0)
                Text(title).kitType(.body).foregroundStyle(Ink.text)
                Spacer(minLength: 0)
            }
            .padding(.vertical, Space.sm)
        }
        .buttonStyle(TableRowButtonStyle())
        .accessibilityAddTraits(kind == option ? .isSelected : [])
    }
}

// MARK: - Choose repo, choose cloud

/// A list to pick one from (the repo, the AWS account), with a way to pick none.
struct ChoicePopover: View {
    let title: String
    let options: [String]
    let selected: String?
    let noneTitle: String?
    var searchPrompt: String?
    let onSelect: (String?) -> Void
    let done: () -> Void
    @State private var query = ""

    private var shown: [String] {
        query.isEmpty ? options : options.filter { $0.localizedStandardContains(query) }
    }

    var body: some View {
        PopoverFrame(title, done: done) {
            VStack(alignment: .leading, spacing: Space.sm) {
                if let searchPrompt {
                    TextField(searchPrompt, text: $query).kitField()
                }
                if options.isEmpty {
                    Text("Nothing to choose from yet. Connect an account on the Connectors page.")
                        .kitType(.meta).foregroundStyle(Ink.textFaint)
                        .fixedSize(horizontal: false, vertical: true)
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        Hairline()
                        if let noneTitle { row(nil, noneTitle) }
                        ForEach(shown, id: \.self) { row($0, $0) }
                    }
                }
                .frame(maxHeight: Space.section * 6)
            }
        }
    }

    private func row(_ option: String?, _ text: String) -> some View {
        Button {
            onSelect(option)
        } label: {
            HStack(spacing: Space.md) {
                KitIconView(icon: .check, size: Dim.icon - Space.xs, weight: 1.8)
                    .foregroundStyle(Ink.brassText)
                    .opacity(selected == option ? 1 : 0)
                Text(text).kitType(.body).foregroundStyle(Ink.text).lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.vertical, Space.sm)
        }
        .buttonStyle(TableRowButtonStyle())
        .accessibilityAddTraits(selected == option ? .isSelected : [])
    }
}

// MARK: - The chip that opens one

/// A chip with its popover attached: the caller gives the chip's text, whether it was set, the popover's title (also its
/// accessibility label) and the popover's content.
struct PopoverChip<Content: View>: View {
    let title: String
    let label: String
    var isSet = false
    let content: (_ close: @escaping () -> Void) -> Content
    @State private var open = false
    @State private var focus = SnapshotFocus.shared

    var body: some View {
        KitChip(title: title, isOpen: open, isSet: isSet, label: label) { open.toggle() }
            .task(id: focus.popover) { await SnapshotFocus.opens(label, focus: focus) { open = true } }
            .popover(isPresented: $open, arrowEdge: .bottom) {
                content { open = false }
            }
    }
}
