import SwiftUI

/// A card you choose: a radio (one of many) or a checkbox (any of several). The top is one real Button; the footer
/// (for example "Runs on this Mac · change") sits below it with its own controls.
enum ChoiceMarkKind { case radio, checkbox }

struct ChoiceCard<Content: View, Footer: View>: View {
    let mark: ChoiceMarkKind
    let isSelected: Bool
    var isEnabled = true
    let action: () -> Void
    let content: Content
    let footer: Footer
    @Environment(\.forcedPartState) private var forced
    @State private var hovering = false
    @State private var focused = false

    init(mark: ChoiceMarkKind, isSelected: Bool, isEnabled: Bool = true, action: @escaping () -> Void,
         @ViewBuilder content: () -> Content, @ViewBuilder footer: () -> Footer) {
        self.mark = mark
        self.isSelected = isSelected
        self.isEnabled = isEnabled
        self.action = action
        self.content = content()
        self.footer = footer()
    }

    private var state: PartState {
        PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: false, hovering: hovering, focused: focused,
                          selected: isSelected)
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        let shown = state
        let selected = isSelected || shown == .selected
        VStack(alignment: .leading, spacing: Space.md) {
            Button(action: action) {
                HStack(alignment: .top, spacing: Space.md) {
                    ChoiceMark(mark: mark, isSelected: selected)
                        .padding(.top, Space.xs / 2)
                    VStack(alignment: .leading, spacing: Space.xs / 2) { content }
                    Spacer(minLength: 0)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(.rect)
            }
            .buttonStyle(ChoiceFocusStyle(focused: $focused))
            .disabled(!isEnabled)
            .accessibilityAddTraits(isSelected ? .isSelected : [])
            footer
                .padding(.leading, Dim.mark + Space.md)
        }
        .padding(.horizontal, Space.lg)
        .padding(.vertical, Dim.rowPadding)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(shape.fill(selected ? Ink.brassWash : shown == .hover || shown == .pressed ? Ink.hover : Color.clear))
        .overlay(shape.strokeBorder(selected || shown == .hover ? Ink.brassBorder : Ink.hairlineStrong, lineWidth: Dim.hairline))
        .kitFocusRing(radius: Dim.radiusCard, visible: shown == .focused)
        .opacity(shown == .disabled ? Dim.disabledOpacity : 1)
        .onHover { hovering = $0 }
        .animation(Motion.hover, value: isSelected)
        .animation(Motion.hover, value: hovering)
    }
}

/// The card's own button draws nothing; it only reports keyboard focus so the whole card can wear the ring.
private struct ChoiceFocusStyle: ButtonStyle {
    @Binding var focused: Bool

    func makeBody(configuration: Configuration) -> some View {
        Reporter(configuration: configuration, focused: $focused)
    }

    private struct Reporter: View {
        let configuration: ButtonStyleConfiguration
        @Binding var focused: Bool
        @Environment(\.isFocused) private var isFocused

        var body: some View {
            configuration.label
                .focusEffectDisabled()
                .onChange(of: isFocused) { _, now in focused = now }
        }
    }
}

extension ChoiceCard where Footer == EmptyView {
    init(mark: ChoiceMarkKind, isSelected: Bool, isEnabled: Bool = true, action: @escaping () -> Void,
         @ViewBuilder content: () -> Content) {
        self.init(mark: mark, isSelected: isSelected, isEnabled: isEnabled, action: action, content: content) { EmptyView() }
    }
}

private struct ChoiceMark: View {
    /// The selected radio is a thick ring around a small hole, so it reads as a dot.
    static let selectedRing: CGFloat = 4.5 // grid: ok stroke width
    let mark: ChoiceMarkKind
    let isSelected: Bool

    var body: some View {
        Group {
            switch mark {
            case .radio:
                Circle()
                    .strokeBorder(isSelected ? Ink.brassFill : Ink.controlBorder, lineWidth: isSelected ? Self.selectedRing : Dim.markStroke)
            case .checkbox:
                RoundedRectangle(cornerRadius: Dim.radiusXS, style: .continuous)
                    .fill(isSelected ? Ink.brassFill : Color.clear)
                    .overlay {
                        if isSelected {
                            KitIconView(icon: .check, size: Dim.mark - Space.xs + 1, weight: 2)
                                .foregroundStyle(Ink.onBrass)
                        } else {
                            RoundedRectangle(cornerRadius: Dim.radiusXS, style: .continuous)
                                .strokeBorder(Ink.controlBorder, lineWidth: Dim.markStroke)
                        }
                    }
            }
        }
        .frame(width: Dim.mark, height: Dim.mark)
        .accessibilityHidden(true)
    }
}

/// "Step 2 of 4 · Connections" with four segments, the done ones in brass.
struct StepProgress: View {
    let current: Int
    let total: Int
    let caption: String

    var body: some View {
        HStack(spacing: Space.md) {
            HStack(spacing: Space.xs) {
                ForEach(0..<total, id: \.self) { index in
                    Capsule()
                        .fill(index <= current ? Ink.brassFill : Ink.track)
                        .frame(width: Dim.segment, height: Dim.bar)
                }
            }
            Text(caption).kitType(.meta).foregroundStyle(Ink.textFaint)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(caption)
    }
}
