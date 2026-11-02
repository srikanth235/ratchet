import SwiftUI

/// The parts gallery: every part of design/KIT.md in every state (rest, hover, pressed, selected, focused, disabled), in the
/// window's current appearance. Snapshot mode shoots it in light and dark.
///   APP_KIT_GALLERY=1 Ratchet     shows the whole gallery instead of the app, in one scrolling page
///   snapshot scenes `gallery-<section>` show one section each (SnapScene in Snapshot.swift)
/// Each part gets its state from the `forcedPartState` environment value, so hover, pressed and focused are real renderings
/// of the part, not pictures.
enum GallerySection: String, CaseIterable, Identifiable {
    case actions, choices, rows, inputs, status, identity, type, tokens

    var id: String { rawValue }

    var title: String {
        switch self {
        case .actions: "Buttons"
        case .choices: "Choices and switches"
        case .rows: "Rows and navigation"
        case .inputs: "Fields and composer"
        case .status: "Status and progress"
        case .identity: "Mark and icons"
        case .type: "Type roles"
        case .tokens: "Ink tokens"
        }
    }
}

/// Which gallery section is on screen, if any. Starts shown (all sections) when `APP_KIT_GALLERY=1`.
@Observable
final class KitGalleryState {
    static let shared = KitGalleryState()
    var isShown = KitGallery.enabled
    var section: GallerySection?
}

struct KitGallery: View {
    static let enabled = ProcessInfo.processInfo.environment["APP_KIT_GALLERY"] == "1"

    /// One cell per state: 136 pt, with a 112 pt label column in front and 12 pt gaps, inside a 24 pt page margin. That is
    /// 1000 pt, so the gallery fits the smallest window (1080) without clipping.
    private static let cell: CGFloat = Space.section * 4 + Space.sm
    private static let labelColumn: CGFloat = Space.xxl * 4 + Space.lg

    private struct DemoItem: Identifiable, Hashable {
        let id: String
    }

    @State private var state = KitGalleryState.shared
    @State private var segment = DemoItem(id: "Yours")
    @State private var range = DemoItem(id: "30d")
    @State private var toggleOn = true
    @State private var toggleOff = false
    @State private var settingsPage = "general"
    @State private var draft = ""
    @State private var filled = "Make checkout p95 faster"
    @State private var fieldText = "acme-org/web-app"

    var body: some View {
        let sections = state.section.map { [$0] } ?? GallerySection.allCases
        ScrollView {
            VStack(alignment: .leading, spacing: Space.section) {
                ForEach(sections) { section in
                    VStack(alignment: .leading, spacing: Space.lg) {
                        Text(section.title).kitType(.pageTitle).foregroundStyle(Ink.text)
                        content(of: section)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(Space.xxl)
        }
        .background(Ink.canvas)
        .accessibilityIdentifier("kit-gallery")
    }

    @ViewBuilder
    private func content(of section: GallerySection) -> some View {
        switch section {
        case .actions: actions
        case .choices: choices
        case .rows: rows
        case .inputs: inputs
        case .status: status
        case .identity: identity
        case .type: typeRoles
        case .tokens: tokens
        }
    }

    // MARK: Sections

    private var actions: some View {
        stateGrid {
            stateRow("Primary") { _ in Button("Create") {}.buttonStyle(.kit(.primary)) }
            stateRow("Primary, small") { _ in Button("Approve") {}.buttonStyle(.kit(.primary, small: true)) }
            stateRow("Quiet") { _ in Button("Add…") {}.buttonStyle(.kit(.quiet)) }
            stateRow("Quiet, small") { _ in Button("Edit") {}.buttonStyle(.kit(.quiet, small: true)) }
            stateRow("Ghost") { _ in Button("Cancel") {}.buttonStyle(.kit(.ghost)) }
            stateRow("Danger") { _ in Button("Stop") {}.buttonStyle(.kit(.danger)) }
            stateRow("Link") { _ in Button("Show sign-in steps") {}.buttonStyle(.kit(.link)) }
            stateRow("Icon button") { _ in KitIconButton(icon: .plus, label: "Add connector") {} }
            stateRow("Icon button, small") { _ in KitIconButton(icon: .close, label: "Dismiss", small: true) {} }
            stateRow("More menu", note: "28 pt hit area") { _ in
                KitMoreMenu("Actions for acme-prod") {
                    Button("Make Default") {}
                    Button("Rename…") {}
                }
            }
        }
    }

    private var choices: some View {
        stateGrid {
            stateRow("Choice, radio") { _ in
                ChoiceCard(mark: .radio, isSelected: false, action: {}) {
                    Text("Claude").kitType(.bodyStrong).foregroundStyle(Ink.text)
                    Text("2 logins").kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
            stateRow("Choice, checkbox") { _ in
                ChoiceCard(mark: .checkbox, isSelected: false, action: {}) {
                    Text("Codex").kitType(.bodyStrong).foregroundStyle(Ink.text)
                    Text("Signed in").kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
            stateRow("Toggle, off") { _ in
                Toggle("Notify", isOn: $toggleOff).toggleStyle(.kit).frame(width: Self.cell - Space.lg)
            }
            stateRow("Toggle, on") { _ in
                Toggle("Notify", isOn: $toggleOn).toggleStyle(.kit).frame(width: Self.cell - Space.lg)
            }
            stateRow("Segmented", note: "state on the 2nd segment") { _ in
                KitSegmented(items: [DemoItem(id: "Yours"), DemoItem(id: "All")], selection: $segment,
                             title: { $0.id })
            }
            stateRow("Range switch") { _ in
                KitSwitch(items: [DemoItem(id: "7d"), DemoItem(id: "30d")], selection: range, title: { $0.id },
                          onSelect: { range = $0 })
            }
            stateRow("Popover chip") { _ in KitChip(title: "Claude (2)") {} }
        }
    }

    private var rows: some View {
        VStack(alignment: .leading, spacing: Space.xxl) {
            stateGrid {
                stateRow("Sidebar row") { _ in SidebarRow(title: "Overview", icon: .overview, isSelected: false) {} }
                stateRow("Table row") { _ in
                    Button {} label: {
                        HStack {
                            Text("Checkout").kitType(.bodyStrong).foregroundStyle(Ink.text)
                            Spacer(minLength: 0)
                        }
                        .padding(.vertical, Dim.rowPadding)
                    }
                    .buttonStyle(TableRowButtonStyle())
                }
                stateRow("Status label") { _ in StatusLabel.shaped(.ok, "Merged") }
            }
            HStack(alignment: .top, spacing: Space.section) {
                VStack(alignment: .leading, spacing: Space.sm) {
                    caption("Settings sidebar (arrow keys move)")
                    SettingsSidebar(heading: "SETTINGS", items: [
                        SettingsSidebarItem(id: "general", title: "General", icon: .sliders),
                        SettingsSidebarItem(id: "notifications", title: "Notifications", icon: .bell),
                        SettingsSidebarItem(id: "cli", title: "Command line", icon: .terminal),
                    ], selection: $settingsPage)
                }
                VStack(alignment: .leading, spacing: Space.sm) {
                    caption("Hairline list")
                    HairlineList {
                        HairlineRow { TitleStack(title: "Checkout p95", subtitle: "3 wins · running") }
                        HairlineRow { TitleStack(title: "Bundle size", subtitle: "No win") }
                    }
                    .frame(width: Self.cell * 2)
                }
            }
        }
    }

    private var inputs: some View {
        VStack(alignment: .leading, spacing: Space.xxl) {
            stateGrid {
                stateRow("Field", note: "pressed, selected: n/a", states: [.rest, .hover, .focused, .disabled]) { _ in
                    TextField("Repository", text: $fieldText).kitField()
                }
            }
            caption("Steering field (glass in a container, docked under the scroll view): rest, hover, focused, disabled")
            Grid(alignment: .topLeading, horizontalSpacing: Space.xl, verticalSpacing: Space.xl) {
                GridRow {
                    steering(.rest)
                    steering(.hover)
                }
                GridRow {
                    steering(.focused)
                    steering(.disabled)
                }
            }
            caption("Composer: rest, hover, focused, disabled, sending, filled")
            Grid(alignment: .topLeading, horizontalSpacing: Space.xl, verticalSpacing: Space.xl) {
                GridRow {
                    composer(.rest, text: $draft)
                    composer(.hover, text: $draft)
                }
                GridRow {
                    composer(.focused, text: $filled)
                    composer(.disabled, text: $draft)
                }
                GridRow {
                    composer(nil, text: $filled, sending: true)
                    composer(nil, text: $filled)
                }
            }
        }
    }

    private var status: some View {
        VStack(alignment: .leading, spacing: Space.xxl) {
            Grid(alignment: .leading, horizontalSpacing: Space.section, verticalSpacing: Space.lg) {
                GridRow {
                    caption("Tone"); caption("Shape + word"); caption("Dot (v0.2, colour only)"); caption("Shape at 32 pt")
                }
                statusRow(.ok, "Merged")
                statusRow(.run, "Running")
                statusRow(.wait, "Waiting")
                statusRow(.bad, "Failed")
                statusRow(.off, "Stopped")
            }
            VStack(alignment: .leading, spacing: Space.lg) {
                caption("Step progress and bar")
                StepProgress(current: 1, total: 4, caption: "Step 2 of 4 · Connections")
                ThinBar(fraction: 0.62).frame(width: Self.cell * 2)
                HeroNumber(label: "Delivered", value: "$12.5k", unit: "/ yr", note: "plus $2.9k pending")
            }
        }
    }

    private var identity: some View {
        VStack(alignment: .leading, spacing: Space.xxl) {
            VStack(alignment: .leading, spacing: Space.md) {
                caption("Mark at 16, 24, 32 and 64 pt (the glyph is about 50% of the app icon's canvas)")
                HStack(alignment: .bottom, spacing: Space.xxl) {
                    ForEach([16.0, 24.0, 32.0, 64.0], id: \.self) { RatchetMarkView(height: $0) }
                }
            }
            VStack(alignment: .leading, spacing: Space.md) {
                caption("Icons at 16 and 24 pt (stroke 1.5, round caps)")
                LazyVGrid(columns: Array(repeating: GridItem(.fixed(Space.section * 3), alignment: .leading), count: 8),
                          alignment: .leading, spacing: Space.lg) {
                    ForEach(KitIcon.allCases, id: \.self) { icon in
                        VStack(alignment: .leading, spacing: Space.sm) {
                            HStack(spacing: Space.md) {
                                KitIconView(icon: icon)
                                KitIconView(icon: icon, size: Space.xxl)
                            }
                            .foregroundStyle(Ink.text)
                            Text(String(describing: icon)).kitType(.microMono).foregroundStyle(Ink.textFaint)
                        }
                    }
                }
            }
        }
    }

    private var typeRoles: some View {
        LazyVGrid(columns: [GridItem(.flexible(), alignment: .leading), GridItem(.flexible(), alignment: .leading)],
                  alignment: .leading, spacing: Space.md) {
            ForEach(KitType.allCases, id: \.self) { role in
                HStack(alignment: .firstTextBaseline, spacing: Space.lg) {
                    Text(String(describing: role)).kitType(.microMono).foregroundStyle(Ink.textFaint)
                        .frame(width: Space.section * 3, alignment: .leading)
                    Text(sample(role)).kitType(role).foregroundStyle(Ink.text).lineLimit(1)
                }
            }
        }
    }

    private var tokens: some View {
        let list: [(String, Color)] = [
            ("canvas", Ink.canvas), ("sidebar", Ink.sidebar), ("raised", Ink.raised), ("raisedSoft", Ink.raisedSoft),
            ("glass", Ink.glass), ("hover", Ink.hover), ("track", Ink.track), ("hairline", Ink.hairline),
            ("hairlineStrong", Ink.hairlineStrong), ("controlBorder", Ink.controlBorder), ("text", Ink.text),
            ("textMuted", Ink.textMuted), ("textFaint", Ink.textFaint), ("brass", Ink.brass), ("brassFill", Ink.brassFill),
            ("brassText", Ink.brassText), ("onBrass", Ink.onBrass), ("brassWash", Ink.brassWash),
            ("brassBorder", Ink.brassBorder), ("focus", Ink.focus), ("ok", Ink.ok), ("wait", Ink.wait), ("bad", Ink.bad),
            ("off", Ink.off), ("okWash", Ink.okWash), ("waitWash", Ink.waitWash), ("badWash", Ink.badWash),
            ("scrim", Ink.scrim), ("markGlass", Ink.markGlass), ("markHi", Ink.markHi), ("markMid", Ink.markMid),
            ("markLo", Ink.markLo),
        ]
        return LazyVGrid(columns: Array(repeating: GridItem(.fixed(Space.section * 5), alignment: .leading), count: 6),
                         alignment: .leading, spacing: Space.lg) {
            ForEach(list, id: \.0) { item in
                VStack(alignment: .leading, spacing: Space.xs) {
                    RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
                        .fill(item.1)
                        .frame(width: Space.section * 3, height: Space.section)
                        .overlay(RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
                            .strokeBorder(Ink.hairlineStrong, lineWidth: Dim.hairline))
                    Text(item.0).kitType(.microMono).foregroundStyle(Ink.textMuted)
                }
            }
        }
    }

    // MARK: Helpers

    private func sample(_ role: KitType) -> String {
        switch role {
        case .pageTitle, .brand, .quote: "Wins, one ratchet at a time"
        case .hero: "$12.5k"
        case .heroUnit: "/ yr"
        case .mono, .monoStrong, .axisMono, .microMono: "p95 412 ms  -18%"
        default: "Make checkout p95 faster"
        }
    }

    private func caption(_ text: String) -> some View {
        Text(text).kitType(.caption).foregroundStyle(Ink.textFaint)
    }

    private func statusRow(_ tone: StatusTone, _ word: String) -> some View {
        GridRow {
            Text(String(describing: tone)).kitType(.microMono).foregroundStyle(Ink.textFaint)
            StatusLabel.shaped(tone, word)
            StatusLabel(tone: tone, text: word)
            StatusShape(tone: tone, size: Space.section)
        }
    }

    private func steering(_ forced: PartState) -> some View {
        KitSteeringField(isFocused: forced == .focused) {
            Text("Tell the agents what to try next")
                .kitType(.body).foregroundStyle(Ink.textMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(Space.sm)
        }
        .environment(\.forcedPartState, forced)
        .disabled(forced == .disabled)
        .frame(width: Self.cell * 3 + Space.xxl)
    }

    private func composer(_ forced: PartState?, text: Binding<String>, sending: Bool = false) -> some View {
        KitComposer(text: text, isSending: sending, onSend: {}) {
            KitChip(title: "Claude Code (2 logins)") {}
            KitChip(title: "Cloud sandbox") {}
        }
        .environment(\.forcedPartState, forced)
        .disabled(forced == .disabled)
        .frame(width: Self.cell * 3 + Space.xxl)
    }

    /// The header row (state names) and the rows given by `rows`.
    private func stateGrid<Rows: View>(@ViewBuilder _ rows: () -> Rows) -> some View {
        Grid(alignment: .topLeading, horizontalSpacing: Space.md, verticalSpacing: Space.lg) {
            GridRow {
                Color.clear.frame(width: Self.labelColumn, height: 1)
                ForEach(PartState.allCases) { part in
                    caption(part.rawValue).frame(width: Self.cell, alignment: .leading)
                }
            }
            rows()
        }
    }

    /// One part in all six states (or the ones in `states`; the rest say n/a).
    private func stateRow<Part: View>(
        _ name: String, note: String? = nil, states: [PartState] = PartState.allCases,
        @ViewBuilder part: @escaping (PartState) -> Part
    ) -> some View {
        GridRow {
            VStack(alignment: .leading, spacing: 0) {
                Text(name).kitType(.meta).foregroundStyle(Ink.textMuted)
                if let note { Text(note).kitType(.micro).foregroundStyle(Ink.textFaint) }
            }
            .frame(width: Self.labelColumn, alignment: .leading)
            ForEach(PartState.allCases) { shown in
                if states.contains(shown) {
                    part(shown)
                        .environment(\.forcedPartState, shown)
                        .disabled(shown == .disabled)
                        .frame(width: Self.cell, alignment: .leading)
                } else {
                    caption("n/a").frame(width: Self.cell, alignment: .leading)
                }
            }
        }
    }
}
