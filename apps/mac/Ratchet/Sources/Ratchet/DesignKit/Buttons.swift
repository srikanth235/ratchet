import SwiftUI

/// Primary (brass fill), quiet (hairline border) and link (brass text) buttons. Real `Button`s with a style, so
/// keyboard, focus and VoiceOver work.
struct KitButtonStyle: ButtonStyle {
    enum Kind { case primary, quiet, link }

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
    @State private var hovering = false

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusButton, style: .continuous)
        let pressed = configuration.isPressed
        if kind == .link {
            configuration.label
                .kitType(small ? .caption : .meta)
                .foregroundStyle(Ink.brassText.opacity(pressed ? 0.7 : 1))
                .underline(hovering)
                .opacity(isEnabled ? 1 : 0.45)
                .contentShape(.rect)
                .onHover { hovering = $0 }
        } else {
            configuration.label
                .kitType(small ? .buttonSmall : .button)
                .padding(.horizontal, small ? Space.md : Space.lg)
                .frame(height: small ? Dim.buttonHeightSmall : Dim.buttonHeight)
                .foregroundStyle(kind == .primary ? Ink.onBrass : Ink.text)
                .background(shape.fill(fill(pressed: pressed)))
                .overlay(shape.strokeBorder(kind == .primary ? Ink.brassFill : Ink.hairlineStrong, lineWidth: Dim.hairline))
                .opacity(isEnabled ? 1 : 0.45)
                .contentShape(shape)
                .onHover { hovering = $0 }
                .animation(Motion.hover, value: hovering)
        }
    }

    private func fill(pressed: Bool) -> Color {
        switch kind {
        case .primary: Ink.brassFill.opacity(pressed ? 0.8 : hovering ? 0.92 : 1)
        case .quiet: pressed || hovering ? Ink.hover : .clear
        case .link: .clear
        }
    }
}
