import SwiftUI

// The v0.3.1 parts: popover chip, segmented control, toggle style, composer, settings sidebar and the status shape.
// Every part takes its state from `PartState.resolve` and honours `forcedPartState`, so the gallery can show all six.

// MARK: - Popover chip

/// A chip that opens a popover: "Claude Code (2 logins) ⌄". A real Button; the caller attaches `.popover` to it and
/// passes `isOpen`. States: rest, hover, pressed, open (selected), focused, disabled.
struct KitChip: View {
    let title: String
    var isOpen = false
    /// The person set this chip (it is not a default): the text and the edge wear the accent.
    var isSet = false
    /// What a screen reader says ("Change agents"); the chip's text is then its value.
    var label: String?
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: Space.sm) {
                Text(title).kitType(.meta)
                KitIconView(icon: .chevronDown, size: Dim.icon - Space.xs)
            }
        }
        .buttonStyle(KitChipStyle(isOpen: isOpen, isSet: isSet))
        .accessibilityLabel(label ?? title)
        .accessibilityValue(label == nil ? (isOpen ? "Open" : "") : title + (isOpen ? ", open" : ""))
        .accessibilityHint("Opens a list of choices")
    }
}

struct KitChipStyle: ButtonStyle {
    var isOpen = false
    var isSet = false

    func makeBody(configuration: Configuration) -> some View {
        PartBody(configuration: configuration, isOpen: isOpen, isSet: isSet)
    }

    private struct PartBody: View {
        let configuration: ButtonStyleConfiguration
        let isOpen: Bool
        let isSet: Bool
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @Environment(\.forcedPartState) private var forced
        @State private var hovering = false

        var body: some View {
            let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: configuration.isPressed,
                                          hovering: hovering, focused: isFocused, selected: isOpen)
            let open = isOpen || state == .selected
            let shape = RoundedRectangle(cornerRadius: Dim.radiusButton, style: .continuous)
            configuration.label
                .foregroundStyle(open || state == .hover || state == .pressed ? Ink.text : isSet ? Ink.brassText : Ink.textMuted)
                .padding(.leading, Space.md)
                .padding(.trailing, Space.sm)
                .frame(height: Dim.chipHeight)
                .background(shape.fill(state == .pressed ? Ink.track : Color.clear))
                .overlay(shape.strokeBorder(open ? Ink.brass : state == .hover || state == .pressed || isSet ? Ink.brassBorder : Ink.controlBorder,
                                            lineWidth: Dim.hairline))
                .kitFocusRing(radius: Dim.radiusButton, visible: state == .focused)
                .opacity(state == .disabled ? Dim.disabledOpacity : 1)
                .contentShape(shape)
                .onHover { hovering = $0 }
                .focusEffectDisabled()
                .animation(Motion.hover, value: hovering)
        }
    }
}

// MARK: - Segmented control

/// A bordered row of segments with the selected one lifted and underlined in brass ("Yours | Discover",
/// "Overview | By Agent | Everything"). One real Button per segment; Left and Right move the selection when the
/// control has keyboard focus. `forcedState` (gallery) applies to the second segment, which stays unselected unless it is `.selected`.
struct KitSegmented<Item: Identifiable & Hashable>: View {
    let items: [Item]
    @Binding var selection: Item
    let title: (Item) -> String
    /// A tooltip for each segment ("Agents, one lane each"); nil for none.
    var help: ((Item) -> String)?
    @Environment(\.forcedPartState) private var forced

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusButton, style: .continuous)
        HStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                if index > 0 { Rectangle().fill(Ink.hairlineStrong).frame(width: Dim.hairline) }
                Button(title(item)) { selection = item }
                    .buttonStyle(KitSegmentStyle(isSelected: item == selection, forced: index == 1 ? forced : nil))
                    .help(help?(item) ?? "")
                    .accessibilityAddTraits(item == selection ? .isSelected : [])
            }
        }
        .clipShape(shape)
        .overlay(shape.strokeBorder(Ink.controlBorder, lineWidth: Dim.hairline))
        .onMoveCommand { direction in
            guard let index = items.firstIndex(of: selection) else { return }
            switch direction {
            case .left: selection = items[max(index - 1, 0)]
            case .right: selection = items[min(index + 1, items.count - 1)]
            default: break
            }
        }
        .accessibilityElement(children: .contain)
    }
}

private struct KitSegmentStyle: ButtonStyle {
    let isSelected: Bool
    let forced: PartState?

    func makeBody(configuration: Configuration) -> some View {
        PartBody(configuration: configuration, isSelected: isSelected, forced: forced)
    }

    private struct PartBody: View {
        let configuration: ButtonStyleConfiguration
        let isSelected: Bool
        let forced: PartState?
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @State private var hovering = false

        var body: some View {
            let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: configuration.isPressed,
                                          hovering: hovering, focused: isFocused, selected: isSelected)
            let selected = isSelected || state == .selected
            configuration.label
                .kitType(selected ? .captionStrong : .meta)
                .foregroundStyle(selected ? Ink.text : Ink.textMuted)
                .padding(.horizontal, Space.md)
                .frame(height: Dim.segmentedHeight)
                .background(selected || state == .hover || state == .pressed ? Ink.hover : Color.clear)
                .overlay(alignment: .bottom) {
                    if selected { Rectangle().fill(Ink.brass).frame(height: Dim.focusRing) }
                }
                .kitFocusRing(radius: Dim.radiusXS, visible: state == .focused, outside: false)
                .opacity(state == .disabled ? Dim.disabledOpacity : 1)
                .contentShape(.rect)
                .onHover { hovering = $0 }
                .focusEffectDisabled()
                .animation(Motion.hover, value: hovering)
        }
    }
}

// MARK: - Toggle

/// The switch: a 40 x 24 track with a control border, a faint knob; on, a brass track and a dark knob. A real Toggle
/// (`Toggle("Launch at login", isOn: $on).toggleStyle(.kit)`), so VoiceOver says "on" or "off".
struct KitToggleStyle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        PartBody(configuration: configuration)
    }

    private struct PartBody: View {
        let configuration: ToggleStyleConfiguration
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @Environment(\.forcedPartState) private var forced
        @State private var hovering = false

        var body: some View {
            let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: false, hovering: hovering,
                                          focused: isFocused, selected: configuration.isOn)
            let on = configuration.isOn || state == .selected
            let track = Capsule()
            Button { configuration.isOn.toggle() } label: {
                HStack(spacing: Space.md) {
                    configuration.label
                        .kitType(.body)
                        .foregroundStyle(Ink.text)
                    Spacer(minLength: Space.lg)
                    ZStack(alignment: on ? .trailing : .leading) {
                        track.fill(on ? Ink.brassFill : state == .pressed ? Ink.track : Color.clear)
                        track.strokeBorder(on ? Ink.brassFill : state == .hover ? Ink.brassBorder : Ink.controlBorder,
                                           lineWidth: Dim.hairline)
                        Circle()
                            .fill(on ? Ink.onBrass : Ink.textFaint)
                            .frame(width: Dim.toggleKnob, height: Dim.toggleKnob)
                            .padding(Space.xs)
                    }
                    .frame(width: Dim.toggleWidth, height: Dim.toggleHeight)
                    .kitFocusRing(radius: Dim.toggleHeight / 2, visible: state == .focused)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .opacity(state == .disabled ? Dim.disabledOpacity : 1)
            .onHover { hovering = $0 }
            .focusEffectDisabled()
            .animation(Motion.press, value: on)
            .accessibilityAddTraits(.isToggle)
            .accessibilityValue(configuration.isOn ? "On" : "Off")
        }
    }
}

extension ToggleStyle where Self == KitToggleStyle {
    static var kit: KitToggleStyle { KitToggleStyle() }
}

// MARK: - Composer

/// The message box of Home and the steering field: a rounded card (16 pt radius) with a multi-line field, a row of chips
/// below it and a brass send button. Command-Return sends. States: rest, hover, focused (edge turns brass, ring), disabled
/// (daemon down), and `isSending` (the send button is a stop button).
struct KitComposer<Chips: View>: View {
    @Binding var text: String
    var placeholder = "Describe what to optimize"
    var isSending = false
    /// False while something else blocks sending (no agent is signed in); Stop stays available while `isSending`.
    var canSend = true
    let onSend: () -> Void
    let chips: Chips
    @FocusState private var focused: Bool
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.forcedPartState) private var forced
    @State private var hovering = false

    init(text: Binding<String>, placeholder: String = "Describe what to optimize", isSending: Bool = false,
         canSend: Bool = true, onSend: @escaping () -> Void, @ViewBuilder chips: () -> Chips) {
        _text = text
        self.placeholder = placeholder
        self.isSending = isSending
        self.canSend = canSend
        self.onSend = onSend
        self.chips = chips()
    }

    var body: some View {
        let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: false, hovering: hovering,
                                      focused: focused)
        let shape = RoundedRectangle(cornerRadius: Dim.radiusLarge, style: .continuous)
        KitGlassContainer {
            composerBody
        }
        .overlay(shape.strokeBorder(state == .focused || state == .hover ? Ink.brassBorder : Ink.controlBorder,
                                    lineWidth: Dim.hairline))
        .kitFocusRing(radius: Dim.radiusLarge, visible: state == .focused)
        .opacity(state == .disabled ? Dim.disabledOpacity : 1)
        .onHover { hovering = $0 }
        .animation(Motion.hover, value: hovering)
    }

    /// The floating card: Liquid Glass, so the page scrolls under it. The control edge and the focus ring stay the kit's.
    private var composerBody: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            TextField(placeholder, text: $text, axis: .vertical)
                .textFieldStyle(.plain)
                .kitType(.body)
                .foregroundStyle(Ink.text)
                .lineLimit(2...6)
                .focused($focused)
                // Return sends (a single-line composer: Shift-Return or Option-Return adds a line), as Command-Return does.
                .onSubmit { if !isSending && canSend && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { onSend() } }
                .frame(minHeight: Dim.composerMinHeight, alignment: .topLeading)
            HStack(spacing: Space.sm) {
                chips
                Spacer(minLength: Space.sm)
                Button(action: onSend) {
                    KitIconView(icon: isSending ? .close : .arrowUp, size: Dim.icon, weight: 2)
                }
                .buttonStyle(KitSendStyle())
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(isSending ? false : (text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !canSend))
                .accessibilityLabel(isSending ? "Stop" : "Send")
                .help(isSending ? "Stop" : "Send (Command-Return)")
            }
        }
        .padding(.horizontal, Space.lg)
        .padding(.top, Space.lg)
        .padding(.bottom, Space.md)
        .kitGlass()
    }
}

private struct KitSendStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        PartBody(configuration: configuration)
    }

    private struct PartBody: View {
        let configuration: ButtonStyleConfiguration
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused

        var body: some View {
            let shape = RoundedRectangle(cornerRadius: Dim.radiusButton, style: .continuous)
            configuration.label
                .foregroundStyle(Ink.onBrass)
                .frame(width: Dim.sendButton, height: Dim.sendButton)
                .background(shape.fill(Ink.brassFill.opacity(configuration.isPressed ? 0.8 : 1)))
                .kitFocusRing(radius: Dim.radiusButton, visible: isFocused)
                .opacity(isEnabled ? 1 : Dim.disabledOpacity)
                .contentShape(shape)
                .focusEffectDisabled()
        }
    }
}

// MARK: - Settings sidebar

struct SettingsSidebarItem: Identifiable, Hashable {
    let id: String
    let title: String
    let icon: KitIcon
}

/// The navigation of a settings area: a small group heading and a column of sidebar rows, 176 pt wide. Up and Down move the
/// selection. Native Settings tabs stay native (DESIGN-SYSTEM.md); this is for settings areas inside a page.
struct SettingsSidebar: View {
    let heading: String
    let items: [SettingsSidebarItem]
    @Binding var selection: String

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            Text(heading)
                .kitType(.microStrong)
                .foregroundStyle(Ink.textFaint)
                .padding(.horizontal, Space.md)
                .padding(.bottom, Space.xs)
            ForEach(items) { item in
                SidebarRow(title: item.title, icon: item.icon, isSelected: item.id == selection) {
                    selection = item.id
                }
            }
        }
        .frame(width: Layout.settingsSidebarWidth, alignment: .leading)
        .onMoveCommand { direction in
            guard let index = items.firstIndex(where: { $0.id == selection }) else { return }
            switch direction {
            case .up: selection = items[max(index - 1, 0)].id
            case .down: selection = items[min(index + 1, items.count - 1)].id
            default: break
            }
        }
    }
}

// MARK: - Status shape

/// Status that survives without color (WCAG 1.4.1): every tone has its own silhouette. Done is a circle with a tick,
/// running a ring with a turning arc, waiting a diamond with a dot, failed a triangle with a bar, off a dashed circle.
/// Always shown with the status word beside it (`StatusLabel`).
struct StatusShape: View {
    let tone: StatusTone
    var size: CGFloat = Dim.statusShape
    @State private var turning = false

    var body: some View {
        Group {
            switch tone {
            case .ok:
                ZStack {
                    Circle().stroke(Ink.ok, lineWidth: Dim.markStroke).padding(Dim.markStroke)
                    StatusGlyph(kind: .tick).stroke(Ink.ok, style: Self.stroke)
                }
            case .run:
                ZStack {
                    Circle().stroke(Ink.track, lineWidth: Dim.markStroke).padding(Dim.markStroke)
                    Circle().trim(from: 0, to: 0.3)
                        .stroke(Ink.brass, style: Self.stroke)
                        .padding(Dim.markStroke)
                        .rotationEffect(.degrees(turning ? 360 : 0))
                }
                .onAppear {
                    guard !Motion.reduced else { return }
                    withAnimation(Motion.loop) { turning = true }
                }
            case .wait:
                ZStack {
                    StatusGlyph(kind: .diamond).stroke(Ink.wait, style: Self.stroke)
                    Circle().fill(Ink.wait).frame(width: Space.xs, height: Space.xs)
                }
            case .bad:
                StatusGlyph(kind: .triangle).stroke(Ink.bad, style: Self.stroke)
            case .off:
                Circle()
                    .stroke(Ink.textFaint, style: StrokeStyle(lineWidth: Dim.markStroke, lineCap: .round, dash: [2, 3]))
                    .padding(Dim.markStroke)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    private static let stroke = StrokeStyle(lineWidth: Dim.markStroke, lineCap: .round, lineJoin: .round)
}

private struct StatusGlyph: Shape {
    enum Kind { case tick, diamond, triangle }
    let kind: Kind

    func path(in rect: CGRect) -> Path {
        var path = Path()
        switch kind {
        case .tick:
            path.addLines([CGPoint(x: 5, y: 8.5), CGPoint(x: 7.2, y: 10.7), CGPoint(x: 11, y: 5.8)])
        case .diamond:
            path.addLines([CGPoint(x: 8, y: 1.5), CGPoint(x: 14.5, y: 8), CGPoint(x: 8, y: 14.5), CGPoint(x: 1.5, y: 8)])
            path.closeSubpath()
        case .triangle:
            path.addLines([CGPoint(x: 8, y: 2), CGPoint(x: 14.5, y: 13.5), CGPoint(x: 1.5, y: 13.5)])
            path.closeSubpath()
            path.move(to: CGPoint(x: 8, y: 6.5))
            path.addLine(to: CGPoint(x: 8, y: 9.5))
            path.move(to: CGPoint(x: 8, y: 11.5))
            path.addLine(to: CGPoint(x: 8, y: 11.6))
        }
        let scale = rect.width / 16
        return path.applying(CGAffineTransform(scaleX: scale, y: scale))
    }
}

extension StatusLabel {
    /// The same label with the colour-independent shape in place of the dot.
    static func shaped(_ tone: StatusTone, _ text: String, caption: String? = nil) -> some View {
        HStack(alignment: .top, spacing: Space.sm) {
            StatusShape(tone: tone).frame(height: Dim.textLine)
            VStack(alignment: .leading, spacing: 0) {
                Text(text).kitType(.body).foregroundStyle(Ink.text)
                if let caption {
                    Text(caption).kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}
