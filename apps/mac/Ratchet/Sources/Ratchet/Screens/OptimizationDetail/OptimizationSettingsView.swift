import RatchetKit
import SwiftUI

/// The settings of one optimization (screen 3): Goal, Repository, Agents, Each run, Schedule, Value, Environment, Output, Notify.
/// Every row has its words, a "Using default" or "Custom" tag where the daemon knows which, and a visible Edit button that opens
/// a popover. Agents, Schedule and Value save through the daemon and show "Saved" beside the row; a list that `ratchet.yaml`
/// holds says so and has no Edit. Archive… is at the foot and in the header's ⋯ menu.
struct OptimizationSettingsView: View {
    let optimization: Optimization
    let detail: OptimizationDetail?
    @Environment(ChatHub.self) private var hub
    @Environment(AppRouter.self) private var router
    @Environment(\.openSettings) private var openSettings
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    var body: some View {
        let model = hub.settings(for: optimization.id)
        let rows = Dictionary(uniqueKeysWithValues: model.rows.map { ($0.key, $0) })
        VStack(alignment: .leading, spacing: Space.xl) {
            Text("Defaults come from Settings → Defaults for new optimizations. “Custom” rows win over them.")
                .kitType(.meta).foregroundStyle(Ink.textFaint)
            HairlineList {
                SettingRowView(title: "Goal", value: optimization.goal.isEmpty ? optimization.title : optimization.goal,
                               note: "Fixed when it was created.")
                SettingRowView(title: "Repository", value: optimization.repo + " · " + optimization.branch,
                               note: "Fixed when it was created.")
                agentsRow(model, rows[.agents])
                SettingRowView(title: "Each run", value: limitsText, note: "Fixed when it was created.")
                scheduleRow(model, rows[.schedule])
                valueRow(model, rows[.value])
                SettingRowView(
                    title: "Environment", value: "Checked before every run; falls back to a smaller setup if a check fails.",
                    tag: .usingDefault, note: "Nothing waits for you.")
                SettingRowView(title: "Output", value: outputText, tag: .usingDefault)
                SettingRowView(
                    title: "Notify", value: notifyText, tag: .usingDefault, editTitle: "Edit",
                    onEdit: { router.settingsTab = .notifications; openSettings() })
            }
            .gridProbe("settings-rows", area: "detail", edge: nil)
            HStack(alignment: .center, spacing: Space.lg) {
                VStack(alignment: .leading, spacing: Space.xs) {
                    Text("Archive optimization").kitType(.bodyStrong).foregroundStyle(Ink.text)
                    Text("Stops the schedule and keeps its history and wins.").kitType(.meta).foregroundStyle(Ink.textFaint)
                }
                Spacer(minLength: Space.lg)
                Button("Archive…") { router.pendingArchive = optimization.id }
                    .buttonStyle(.kit(.quiet))
                    .disabled(!actionsEnabled || model.isArchived)
            }
            .padding(.top, Space.md)
            .overlay(alignment: .top) { Hairline() }
        }
    }

    // MARK: Values

    private var limitsText: String {
        var text = optimization.budgetUSD.map { "up to " + Money.short($0) } ?? "no budget set"
        text += " and " + Elapsed.hm(optimization.timeLimitHours * 3600)
        return text
    }

    private var outputText: String {
        var text = "a pull request with the verified wins"
        if let number = optimization.prNumber { text += " · PR #\(number)" }
        return text
    }

    private var notifyText: String {
        let on = NotificationCategory.allCases.filter { $0 != .environmentDrafted && NotificationPreferences.isOn($0) }
        return on.isEmpty ? "nothing" : on.map { $0.title.lowercased() }.joined(separator: " · ")
    }

    // MARK: Rows with an editor

    @ViewBuilder
    private func agentsRow(_ model: OptimizationSettingsModel, _ row: SettingsRow?) -> some View {
        let placement = detail?.agents.first?.placement.phrase
        SettingRowView(
            title: "Agents", value: (row?.value ?? "…") + (placement.map { " · on " + $0 } ?? ""), tag: row?.tag, isSaved: row?.isSaved ?? false,
            note: row?.lockedNote ?? row?.error, noteIsError: row?.lockedNote == nil && row?.error != nil,
            editEnabled: (row?.isEditable ?? false) && actionsEnabled && model.agentsEditor != nil
        ) { close in
            AgentsRowEditor(model: model, available: hub.agents.catalog.ready, close: close)
        }
    }

    @ViewBuilder
    private func scheduleRow(_ model: OptimizationSettingsModel, _ row: SettingsRow?) -> some View {
        let value = model.isSchedulePaused ? "Paused · was " + (model.pausedSchedule?.title.lowercased() ?? "scheduled") : (row?.value ?? "…")
        SettingRowView(
            title: "Schedule", value: value, tag: row?.tag, isSaved: row?.isSaved ?? false, note: row?.error, noteIsError: row?.error != nil,
            editEnabled: actionsEnabled && model.schedule != nil
        ) { close in
            ScheduleRowEditor(model: model, close: close)
        }
    }

    @ViewBuilder
    private func valueRow(_ model: OptimizationSettingsModel, _ row: SettingsRow?) -> some View {
        SettingRowView(
            title: "Value", value: row?.value ?? "…", tag: row?.tag, isSaved: row?.isSaved ?? false, note: row?.error,
            noteIsError: row?.error != nil, editEnabled: actionsEnabled
        ) { close in
            ValueRowEditor(model: model, close: close)
        }
        .id("value")
    }
}

// MARK: - One row

/// The tag beside a row: "Using default" quiet, "Custom" in brass.
struct SettingTag: View {
    let tag: SettingsTag

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusXS, style: .continuous)
        Text(tag.rawValue)
            .kitType(.micro)
            .foregroundStyle(tag == .custom ? Ink.brassText : Ink.textFaint)
            .padding(.horizontal, Space.sm)
            .padding(.vertical, Space.xs / 2)
            .overlay(shape.strokeBorder(tag == .custom ? Ink.brassBorder : Ink.hairlineStrong, lineWidth: Dim.hairline))
    }
}

/// A label, its words, and at the trailing edge "Saved", the tag and a visible Edit button that opens a popover.
struct SettingRowView<Content: View>: View {
    let title: String
    let value: String
    var tag: SettingsTag?
    var isSaved = false
    /// A line under the words: why Edit is off, or what went wrong saving.
    var note: String?
    var noteIsError = false
    var editEnabled = true
    var editTitle = "Edit"
    var onEdit: (() -> Void)?
    let content: ((_ close: @escaping () -> Void) -> Content)?
    @State private var open = false
    @State private var focus = SnapshotFocus.shared

    fileprivate init(
        title: String, value: String, tag: SettingsTag?, isSaved: Bool, note: String?, noteIsError: Bool, editEnabled: Bool,
        editTitle: String, onEdit: (() -> Void)?, storing content: ((_ close: @escaping () -> Void) -> Content)?
    ) {
        self.title = title
        self.value = value
        self.tag = tag
        self.isSaved = isSaved
        self.note = note
        self.noteIsError = noteIsError
        self.editEnabled = editEnabled
        self.editTitle = editTitle
        self.onEdit = onEdit
        self.content = content
    }

    /// A row whose Edit button opens a popover (the trailing closure builds it; `close` dismisses it).
    init(
        title: String, value: String, tag: SettingsTag? = nil, isSaved: Bool = false, note: String? = nil, noteIsError: Bool = false,
        editEnabled: Bool = true, @ViewBuilder content: @escaping (_ close: @escaping () -> Void) -> Content
    ) {
        self.init(
            title: title, value: value, tag: tag, isSaved: isSaved, note: note, noteIsError: noteIsError, editEnabled: editEnabled,
            editTitle: "Edit", onEdit: nil, storing: content)
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.lg) {
            Text(title).kitType(.meta).foregroundStyle(Ink.textFaint)
                .frame(width: Layout.labelColumn, alignment: .leading)
            VStack(alignment: .leading, spacing: Space.xs) {
                Text(value).kitType(.body).foregroundStyle(Ink.text).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                if let note {
                    Text(note).kitType(.meta).foregroundStyle(noteIsError ? Ink.bad : Ink.textFaint)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if isSaved {
                HStack(spacing: Space.xs) {
                    KitIconView(icon: .check, size: Dim.icon - Space.xs, weight: 1.8)
                    Text("Saved").kitType(.caption)
                }
                .foregroundStyle(Ink.ok)
                .accessibilityElement(children: .combine)
                .transition(.opacity)
            }
            if let tag { SettingTag(tag: tag) }
            if content != nil || onEdit != nil {
                Button(editTitle) {
                    if let onEdit { onEdit() } else { open = true }
                }
                .buttonStyle(.kit(.quiet, small: true))
                .disabled(!editEnabled)
                .accessibilityLabel("\(editTitle) \(title)")
                .popover(isPresented: $open, arrowEdge: .trailing) {
                    if let content { content { open = false } }
                }
                .task(id: focus.popover) { await SnapshotFocus.opens("\(editTitle) \(title)", focus: focus) { open = true } }
            }
        }
        .padding(.vertical, Space.md)
        .overlay(alignment: .bottom) { Hairline() }
        .animation(Motion.step, value: isSaved)
        .accessibilityElement(children: .contain)
    }
}

extension SettingRowView where Content == EmptyView {
    /// A row with no popover: words, an optional tag and a note; `onEdit` gives it a button that does something else (open
    /// Settings).
    init(title: String, value: String, tag: SettingsTag? = nil, note: String? = nil, editTitle: String = "Edit", onEdit: (() -> Void)? = nil) {
        self.init(
            title: title, value: value, tag: tag, isSaved: false, note: note, noteIsError: false, editEnabled: true,
            editTitle: editTitle, onEdit: onEdit, storing: nil)
    }
}

// MARK: - The editors

private struct AgentsRowEditor: View {
    let model: OptimizationSettingsModel
    let available: [Harness]
    let close: () -> Void
    @State private var editor: AgentOrderEditor

    init(model: OptimizationSettingsModel, available: [Harness], close: @escaping () -> Void) {
        self.model = model
        self.available = available
        self.close = close
        var start = model.agentsEditor ?? AgentOrderEditor(order: [.claudeCode])
        if !available.isEmpty { start.available = available }
        _editor = State(initialValue: start)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            AgentOrderPopover(editor: $editor, showsWhere: false) {
                Task { if await model.saveAgents(editor) { close() } }
            }
            if let error = model.errors[.agents] {
                Text(error).kitType(.meta).foregroundStyle(Ink.bad)
                    .padding(.horizontal, Space.lg).padding(.bottom, Space.lg)
                    .frame(width: Layout.popoverWidth, alignment: .leading)
            }
        }
    }
}

private struct ScheduleRowEditor: View {
    let model: OptimizationSettingsModel
    let close: () -> Void
    @State private var kind: ScheduleKind?

    init(model: OptimizationSettingsModel, close: @escaping () -> Void) {
        self.model = model
        self.close = close
        _kind = State(initialValue: model.schedule?.kind)
    }

    var body: some View {
        SchedulePopover(kind: $kind, allowsNone: true) {
            Task { if await model.saveSchedule(kind) { close() } }
        }
    }
}

private struct ValueRowEditor: View {
    let model: OptimizationSettingsModel
    let close: () -> Void
    @State private var editor: ValueModelEditor

    init(model: OptimizationSettingsModel, close: @escaping () -> Void) {
        self.model = model
        self.close = close
        _editor = State(initialValue: model.valueEditor)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            Text("Change value").kitType(.section).foregroundStyle(Ink.text).accessibilityAddTraits(.isHeader)
            Text("What one unit of improvement is worth. Changing it re-values every win.")
                .kitType(.meta).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
            field("Worth per unit, in dollars", text: $editor.usdPerUnitText, prompt: "1200")
            field("Unit", text: $editor.unit, prompt: "ms")
            HStack(spacing: Space.md) {
                Text("Per").kitType(.meta).foregroundStyle(Ink.textFaint)
                ChoiceSegmented(
                    options: ValuePeriod.allCases.map { SegmentOption(id: $0.rawValue, title: $0 == .year ? "Year" : "Month") },
                    selection: Binding(get: { editor.period.rawValue }, set: { editor.period = ValuePeriod(rawValue: $0) ?? .year }))
                ChoiceSegmented(
                    options: [SegmentOption(id: "estimated", title: "Estimated"), SegmentOption(id: "measured", title: "Measured")],
                    selection: Binding(get: { editor.basis.rawValue }, set: { editor.basis = ValueBasisKind(rawValue: $0) ?? .estimated }))
            }
            field("In plain words", text: $editor.note, prompt: "ms saved × checkouts a day")
            if let problem = model.errors[.value] ?? (editor.usdPerUnitText.isEmpty ? nil : editor.problem) {
                Text(problem).kitType(.meta).foregroundStyle(Ink.bad).fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: Space.sm) {
                if model.valueModel?.isSet == true {
                    Button("Use Default") { Task { if await model.clearValue() { close() } } }
                        .buttonStyle(.kit(.link, small: true))
                }
                Spacer()
                Button("Cancel", action: close).buttonStyle(.kit(.ghost, small: true))
                Button("Save") { Task { if await model.saveValue(editor) { close() } } }
                    .buttonStyle(.kit(.primary, small: true))
                    .keyboardShortcut(.defaultAction)
                    .disabled(editor.problem != nil)
            }
        }
        .padding(Space.lg)
        .frame(width: Layout.popoverWidth, alignment: .leading)
    }

    private func field(_ label: String, text: Binding<String>, prompt: String) -> some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            Text(label).kitType(.meta).foregroundStyle(Ink.textMuted)
            TextField(prompt, text: text).kitField().accessibilityLabel(label)
        }
    }
}
