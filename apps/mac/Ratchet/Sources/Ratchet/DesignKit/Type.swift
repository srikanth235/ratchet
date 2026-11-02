import SwiftUI

/// Type roles, from the reference CSS in design/tools/v03.css: three text sizes, 11 (micro), 12.5 (notes, captions, buttons, mono) and 14
/// (body, headings), plus 10 for chart axes only. Instrument Serif for page titles and big numbers (32 title, 52 hero), SF Mono for numbers. The system fonts stand in
/// when the bundled faces are missing (`KitFonts`). Views set fonts only through `.kitType(...)`.
enum KitType: CaseIterable {
    case pageTitle, brand, hero, heroUnit, quote
    case body, bodyStrong, meta, caption, section, button, buttonSmall
    case buttonPrimary, captionStrong, mono, monoStrong, axis, axisMono, micro, microStrong, microMono

    var font: Font {
        switch self {
        case .pageTitle: serif(32)
        case .brand: serif(22)
        case .hero: serif(Dim.heroValue)
        case .heroUnit: serif(22)
        case .quote: serif(28)
        case .body: sans(KitFonts.sansRegular, 14, .regular)
        case .bodyStrong: sans(KitFonts.sansMedium, 14, .medium)
        case .meta: sans(KitFonts.sansRegular, 12.5, .regular)
        case .caption: sans(KitFonts.sansRegular, 12.5, .regular)
        case .buttonPrimary: sans(KitFonts.sansSemibold, 12.5, .semibold)
        case .captionStrong: sans(KitFonts.sansMedium, 12.5, .medium)
        case .section: sans(KitFonts.sansSemibold, 14, .semibold)
        case .button: sans(KitFonts.sansMedium, 12.5, .medium)
        case .buttonSmall: sans(KitFonts.sansMedium, 12.5, .medium)
        case .mono: .system(size: 12.5, design: .monospaced)
        case .monoStrong: .system(size: 12.5, weight: .medium, design: .monospaced)
        // Chart axes: 10 pt, the smallest text anywhere (the reference scaled them to 8 pt, below the 10 pt floor).
        case .axis: sans(KitFonts.sansRegular, 10, .regular)
        case .axisMono: .system(size: 10, design: .monospaced)
        // v0.3.1 `.xs` (11 px): badges, counters, group headings, the debug overlay.
        case .micro: sans(KitFonts.sansRegular, 11, .regular)
        case .microStrong: sans(KitFonts.sansSemibold, 11, .semibold)
        case .microMono: .system(size: 11, design: .monospaced)
        }
    }

    /// The role's size in points: the contrast check treats 24 and up as large text.
    var pointSize: CGFloat {
        switch self {
        case .pageTitle: 32
        case .brand, .heroUnit: 22
        case .quote: 28
        case .hero: Dim.heroValue
        case .body, .bodyStrong, .section: 14
        case .meta, .caption, .captionStrong, .button, .buttonSmall, .buttonPrimary, .mono, .monoStrong: 12.5
        case .micro, .microStrong, .microMono: 11
        case .axis, .axisMono: 10
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
