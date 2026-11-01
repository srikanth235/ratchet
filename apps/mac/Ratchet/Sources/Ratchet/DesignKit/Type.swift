import SwiftUI

/// Type roles, from the reference CSS in design/v02.html: Instrument Sans for text (13.5 body, 12.5 notes, 12 captions),
/// Instrument Serif for page titles and big numbers (32 title, 56 hero), SF Mono for numbers. The system fonts stand in
/// when the bundled faces are missing (`KitFonts`). Views set fonts only through `.kitType(...)`.
enum KitType {
    case pageTitle, brand, hero, heroUnit, quote
    case body, bodyStrong, meta, caption, section, button, buttonSmall
    case buttonPrimary, captionStrong, mono, monoStrong, axis, axisMono

    var font: Font {
        switch self {
        case .pageTitle: serif(32)
        case .brand: serif(23)
        case .hero: serif(Dim.heroValue)
        case .heroUnit: serif(22)
        case .quote: serif(26)
        case .body: sans(KitFonts.sansRegular, 13.5, .regular)
        case .bodyStrong: sans(KitFonts.sansMedium, 13.5, .medium)
        case .meta: sans(KitFonts.sansRegular, 12.5, .regular)
        case .caption: sans(KitFonts.sansRegular, 12, .regular)
        case .buttonPrimary: sans(KitFonts.sansSemibold, 13, .semibold)
        case .captionStrong: sans(KitFonts.sansMedium, 12, .medium)
        case .section: sans(KitFonts.sansSemibold, 13, .semibold)
        case .button: sans(KitFonts.sansMedium, 13, .medium)
        case .buttonSmall: sans(KitFonts.sansMedium, 12, .medium)
        case .mono: .system(size: 12.5, design: .monospaced)
        case .monoStrong: .system(size: 12.5, weight: .medium, design: .monospaced)
        // Chart axes: the reference scales its 11 px labels down with the chart, to about 8 pt.
        case .axis: sans(KitFonts.sansRegular, 8, .regular)
        case .axisMono: .system(size: 8, design: .monospaced)
        }
    }

    /// Serif titles and numbers sit a little tighter, as in the reference (letter-spacing -0.01em).
    var tracking: CGFloat {
        switch self {
        case .pageTitle: -0.32
        case .brand: -0.23
        case .hero: -0.56
        case .heroUnit: -0.22
        case .quote: -0.26
        default: 0
        }
    }

    private func serif(_ size: CGFloat) -> Font {
        KitFonts.font(KitFonts.serif, size: size, fallback: .system(size: size, weight: .regular, design: .serif))
    }

    private func sans(_ name: String, _ size: CGFloat, _ weight: Font.Weight) -> Font {
        KitFonts.font(name, size: size, fallback: .system(size: size, weight: weight))
    }
}

extension KitType {
    /// The reference's line height for running text, or nil for roles that size themselves (serif, buttons, mono).
    var minLine: CGFloat? {
        switch self {
        case .body, .bodyStrong: Dim.lineBody
        case .meta, .caption, .captionStrong: Dim.lineMeta
        default: nil
        }
    }
}

extension View {
    @ViewBuilder
    func kitType(_ type: KitType) -> some View {
        if let line = type.minLine {
            font(type.font).tracking(type.tracking).frame(minHeight: line)
        } else {
            font(type.font).tracking(type.tracking)
        }
    }
}
