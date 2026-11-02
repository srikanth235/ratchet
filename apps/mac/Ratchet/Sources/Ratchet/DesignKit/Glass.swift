import SwiftUI

// Liquid Glass for what floats over content, and chrome that follows the window.
// Native rule (design/DESIGN-SYSTEM.md): sheets, popovers, toolbars and the menu bar window get no custom background, the
// system paints them as glass. Only a custom floating surface (the composer, the steering field) asks for glass itself, and
// it does so inside one `KitGlassContainer`, so glass never samples glass.

/// The one container every floating glass surface sits in. Neighbouring glass shares it and blends; nothing sits on glass.
/// Pages put the floating composer or the steering field inside it and fill it.
struct KitGlassContainer<Content: View>: View {
    var spacing: CGFloat = Space.md
    let content: Content

    init(spacing: CGFloat = Space.md, @ViewBuilder content: () -> Content) {
        self.spacing = spacing
        self.content = content()
    }

    var body: some View {
        GlassEffectContainer(spacing: spacing) {
            content
        }
    }
}

extension View {
    /// Liquid Glass behind a floating container, in the kit's large radius. Tint only to carry meaning (brass on send).
    func kitGlass(radius: CGFloat = Dim.radiusLarge) -> some View {
        glassEffect(.regular, in: .rect(cornerRadius: radius))
    }

    /// Escape closes a popover or sheet from wherever the focus is inside it; the system gives the focus back to what opened it.
    func closesOnEscape() -> some View {
        modifier(ClosesOnEscape())
    }

    /// Custom chrome (page titles, status items, segments) dims when the window is not in front, like the system's own.
    func dimsWhenInactive() -> some View {
        modifier(DimsWhenInactive())
    }
}

private struct ClosesOnEscape: ViewModifier {
    @Environment(\.dismiss) private var dismiss

    func body(content: Content) -> some View {
        content.onExitCommand { dismiss() }
    }
}

private struct DimsWhenInactive: ViewModifier {
    @Environment(\.appearsActive) private var appearsActive

    func body(content: Content) -> some View {
        content.opacity(appearsActive ? 1 : Dim.inactiveOpacity)
    }
}

/// The floating field under a running optimization, where the person steers it: a glass container with a control edge and a
/// concentric inner area. Pages anchor it with `.safeAreaInset(edge: .bottom)`. P5b fills it (the note field, the timeline
/// hint); the glass, the shape and the focus ring are this part's. States: rest, hover, focused, disabled.
struct KitSteeringField<Content: View>: View {
    /// Whether the field inside has the keyboard (the screen owns its `FocusState`): the part draws the ring.
    var isFocused = false
    let content: Content
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.forcedPartState) private var forced
    @State private var hovering = false

    init(isFocused: Bool = false, @ViewBuilder content: () -> Content) {
        self.isFocused = isFocused
        self.content = content()
    }

    var body: some View {
        let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: false, hovering: hovering,
                                      focused: isFocused)
        let shape = RoundedRectangle(cornerRadius: Dim.radiusLarge, style: .continuous)
        KitGlassContainer {
            content
                .padding(Space.sm)
                .background(Ink.raisedSoft, in: ConcentricRectangle())
                .padding(Space.sm)
                .frame(maxWidth: .infinity)
                .kitGlass()
                .overlay(shape.strokeBorder(state == .focused || state == .hover ? Ink.brassBorder : Ink.controlBorder,
                                            lineWidth: Dim.hairline))
                .kitFocusRing(radius: Dim.radiusLarge, visible: state == .focused)
        }
        .containerShape(shape)
        .opacity(state == .disabled ? Dim.disabledOpacity : 1)
        .onHover { hovering = $0 }
        .animation(Motion.hover, value: hovering)
    }
}
