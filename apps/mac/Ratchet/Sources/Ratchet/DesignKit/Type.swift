import SwiftUI

/// Type roles. SF for text, SF Mono for numbers, the system serif for page titles and big numbers (the design's
/// Instrument Serif is not bundled). Views set fonts only through `.kitType(...)`.
enum KitType {
    case pageTitle, brand, hero, heroUnit, quote
    case body, bodyStrong, meta, caption, section, button, buttonSmall
    case mono, monoStrong

    var font: Font {
        switch self {
        case .pageTitle: .system(size: 32, weight: .regular, design: .serif)
        case .brand: .system(size: 23, weight: .regular, design: .serif)
        case .hero: .system(size: Dim.heroValue, weight: .regular, design: .serif)
        case .heroUnit: .system(size: 22, weight: .regular, design: .serif)
        case .quote: .system(size: 26, weight: .regular, design: .serif)
        case .body: .system(size: 13.5)
        case .bodyStrong: .system(size: 13.5, weight: .medium)
        case .meta: .system(size: 12.5)
        case .caption: .system(size: 12)
        case .section: .system(size: 13, weight: .semibold)
        case .button: .system(size: 13, weight: .medium)
        case .buttonSmall: .system(size: 12, weight: .medium)
        case .mono: .system(size: 12.5, design: .monospaced)
        case .monoStrong: .system(size: 12.5, weight: .medium, design: .monospaced)
        }
    }
}

extension View {
    func kitType(_ type: KitType) -> some View {
        font(type.font)
    }
}
