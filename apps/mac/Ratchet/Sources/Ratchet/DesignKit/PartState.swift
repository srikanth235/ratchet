import SwiftUI

/// The six states of a part. Every part computes one state from the system (hover, press, focus, enabled, selected) and
/// the gallery forces it with `forcedPartState`, so hover and pressed show up in a snapshot.
enum PartState: String, CaseIterable, Identifiable {
    case rest, hover, pressed, selected, focused, disabled

    var id: String { rawValue }

    /// Resolves what a part shows. A forced state wins; then disabled, pressed, hover, selected, focused.
    static func resolve(forced: PartState?, isEnabled: Bool, pressed: Bool, hovering: Bool, focused: Bool,
                        selected: Bool = false) -> PartState {
        if let forced { return forced }
        if !isEnabled { return .disabled }
        if pressed { return .pressed }
        if hovering { return .hover }
        if selected { return .selected }
        if focused { return .focused }
        return .rest
    }
}

extension EnvironmentValues {
    /// Set by the gallery only. Parts read it instead of the live hover, press and focus.
    @Entry var forcedPartState: PartState?
}

extension View {
    /// The keyboard focus ring: `Ink.focus`, 2 pt wide. Outside the part by `Dim.focusOffset`, or inside it (`outside:
    /// false`) for rows that fill their column and would be clipped.
    func kitFocusRing(radius: CGFloat, visible: Bool, outside: Bool = true) -> some View {
        overlay {
            if visible {
                if outside {
                    RoundedRectangle(cornerRadius: radius + Dim.focusOffset + Dim.focusRing, style: .continuous)
                        .strokeBorder(Ink.focus, lineWidth: Dim.focusRing)
                        .padding(-(Dim.focusOffset + Dim.focusRing))
                        .allowsHitTesting(false)
                } else {
                    RoundedRectangle(cornerRadius: radius, style: .continuous)
                        .strokeBorder(Ink.focus, lineWidth: Dim.focusRing)
                        .allowsHitTesting(false)
                }
            }
        }
    }
}
