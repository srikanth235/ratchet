import SwiftUI

/// Primary (brass fill), quiet (control border), ghost (no border), danger (quiet, red text) and link (brass text) buttons.
/// Real `Button`s with a style, so keyboard, focus and VoiceOver work. States: rest, hover, pressed, focused, disabled.
struct KitButtonStyle: ButtonStyle {
    enum Kind { case primary, quiet, ghost, danger, link }

    var kind: Kind = .primary
    var small = false

    func makeBody(configuration: Configuration) -> some View {
        KitButtonBody(configuration: configuration, kind: kind, small: small)
    }
}

extension ButtonStyle where Self == KitButtonStyle {
    static func kit(_ kind: KitButtonStyle.Kind = .primary, small: Bool = false) -> KitButtonStyle {
        KitButtonStyle(kind: kind, small: small)
    }
}

private struct KitButtonBody: View {
    let configuration: ButtonStyleConfiguration
    let kind: KitButtonStyle.Kind
    let small: Bool
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.isFocused) private var isFocused
    @Environment(\.forcedPartState) private var forced
    @State private var hovering = false

    private var resolved: PartState {
        PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: configuration.isPressed, hovering: hovering,
                          focused: isFocused)
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusButton, style: .continuous)
        let state = resolved
        if kind == .link {
            configuration.label
                .kitType(small ? .caption : .meta)
                .foregroundStyle(Ink.brassText.opacity(state == .pressed ? 0.7 : 1))
                .underline(state == .hover)
                .padding(.vertical, Space.xs)
                .kitFocusRing(radius: Dim.radiusXS, visible: state == .focused)
                .opacity(state == .disabled ? Dim.disabledOpacity : 1)
                .contentShape(.rect)
                .onHover { hovering = $0 }
                .focusEffectDisabled()
        } else {
            configuration.label
                .kitType(small ? .buttonSmall : kind == .primary ? .buttonPrimary : .button)
                .padding(.horizontal, small ? Space.md : Space.lg)
                .frame(height: small ? Dim.buttonHeightSmall : Dim.buttonHeight)
                .foregroundStyle(foreground)
                .background(shape.fill(fill(state)))
                .overlay(shape.strokeBorder(border(state), lineWidth: Dim.hairline))
                .kitFocusRing(radius: Dim.radiusButton, visible: state == .focused)
                .opacity(state == .disabled ? Dim.disabledOpacity : 1)
                .contentShape(shape)
                .onHover { hovering = $0 }
                .focusEffectDisabled()
                .animation(Motion.hover, value: hovering)
        }
    }

    private var foreground: Color {
        switch kind {
        case .primary: Ink.onBrass
        case .danger: Ink.bad
        case .ghost: Ink.textMuted
        default: Ink.text
        }
    }

    private func fill(_ state: PartState) -> Color {
        switch kind {
        case .primary: Ink.brassFill.opacity(state == .pressed ? 0.8 : state == .hover ? 0.92 : 1)
        case .quiet, .danger: state == .pressed ? Ink.track : state == .hover ? Ink.hover : .clear
        case .ghost: state == .pressed ? Ink.track : state == .hover ? Ink.hover : .clear
        case .link: .clear
        }
    }

    private func border(_ state: PartState) -> Color {
        switch kind {
        case .primary: Ink.brassFill
        case .ghost: .clear
        default: state == .hover || state == .pressed ? Ink.brassBorder : Ink.controlBorder
        }
    }
}

/// An icon-only button: 32 pt (28 small), the control border, a label for VoiceOver and the tooltip. Never an unlabelled glyph.
struct KitIconButton: View {
    let icon: KitIcon
    let label: String
    var small = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            KitIconView(icon: icon, size: small ? Dim.icon - Space.xs : Dim.icon)
        }
        .buttonStyle(KitIconButtonStyle(small: small))
        .accessibilityLabel(label)
        .help(label)
    }
}

struct KitIconButtonStyle: ButtonStyle {
    var small = false

    func makeBody(configuration: Configuration) -> some View {
        PartBody(configuration: configuration, small: small)
    }

    private struct PartBody: View {
        let configuration: ButtonStyleConfiguration
        let small: Bool
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @Environment(\.forcedPartState) private var forced
        @State private var hovering = false

        var body: some View {
            let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: configuration.isPressed,
                                          hovering: hovering, focused: isFocused)
            let side = small ? Dim.buttonHeightSmall : Dim.buttonHeight
            let shape = RoundedRectangle(cornerRadius: Dim.radiusButton, style: .continuous)
            configuration.label
                .foregroundStyle(state == .hover || state == .pressed ? Ink.text : Ink.textMuted)
                .frame(width: side, height: side)
                .background(shape.fill(state == .pressed ? Ink.track : state == .hover ? Ink.hover : .clear))
                .overlay(shape.strokeBorder(state == .hover || state == .pressed ? Ink.brassBorder : Ink.controlBorder,
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

/// The ⋯ of a row: an `ellipsis` symbol in a 28 pt hit area (the smallest target), opening the row's actions. Every action
/// in the menu is also in the context menu and the menu bar. Always labelled.
struct KitMoreMenu<Items: View>: View {
    let label: String
    let items: Items

    init(_ label: String, @ViewBuilder items: () -> Items) {
        self.label = label
        self.items = items()
    }

    var body: some View {
        Menu {
            items
        } label: {
            MoreGlyph()
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .accessibilityLabel(label)
        .help(label)
    }

    private struct MoreGlyph: View {
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.forcedPartState) private var forced
        @State private var hovering = false

        var body: some View {
            let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: false, hovering: hovering,
                                          focused: false)
            Image(systemName: "ellipsis")
                .imageScale(.large)
                .foregroundStyle(state == .hover || state == .pressed ? Ink.text : Ink.textMuted)
                .frame(width: Dim.buttonHeightSmall, height: Dim.buttonHeightSmall)
                .background(RoundedRectangle(cornerRadius: Dim.radiusButton, style: .continuous)
                    .fill(state == .hover || state == .pressed ? Ink.hover : Color.clear))
                .opacity(state == .disabled ? Dim.disabledOpacity : 1)
                .contentShape(.rect)
                .onHover { hovering = $0 }
                .animation(Motion.hover, value: hovering)
        }
    }
}
