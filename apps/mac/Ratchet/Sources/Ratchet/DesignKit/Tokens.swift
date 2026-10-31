import AppKit
import SwiftUI

// Midnight Brass, calm version. Three layers (design-kit skill):
//   1. Palette: raw colors, named by hue and step. Only this file reads them.
//   2. Ink: meaning (text, canvas, accent, status), each with a light and a dark value. Parts read only this.
//   3. Dim, Motion: sizes and timing. KitType (Type.swift) holds the type roles.
// Source of the dark values: design/v02.html. The light values are the same palette on warm paper.
// This file is skipped by tools/grid-lint.sh: the design's own sizes (3, 7, 15 pt) live here, once.

/// Layer 1.
enum Palette {
    // Dark ground
    static let night900: UInt32 = 0x0B1019
    static let night800: UInt32 = 0x101724
    static let night850: UInt32 = 0x0E1420
    static let slate400: UInt32 = 0x94A3BE
    static let snow: UInt32 = 0xECEFF4
    static let slate300: UInt32 = 0xA7B1C2
    static let slate500: UInt32 = 0x7E8CA3
    // Light ground
    static let paper: UInt32 = 0xFBFAF6
    static let paperDeep: UInt32 = 0xF3F1EA
    static let white: UInt32 = 0xFFFFFF
    static let umber: UInt32 = 0x3C3220
    static let ink900: UInt32 = 0x15181F
    static let ink600: UInt32 = 0x4A5363
    static let ink500: UInt32 = 0x6B7587
    // Brass
    static let brass500: UInt32 = 0xD9B26F
    static let brass300: UInt32 = 0xEBCB8E
    static let brass600: UInt32 = 0xB8861F
    static let brass700: UInt32 = 0x8A5E0B
    static let brassInk: UInt32 = 0x16110A
    // Status
    static let mint400: UInt32 = 0x7FD1AE
    static let mint700: UInt32 = 0x1F8A62
    static let amber400: UInt32 = 0xE9B85C
    static let amber700: UInt32 = 0xA86F0B
    static let rose400: UInt32 = 0xE58C8C
    static let rose700: UInt32 = 0xB4414A
    static let gray600: UInt32 = 0x4A5568
    static let gray400: UInt32 = 0x9AA3B2
}

/// Layer 2: what a color means. Parts never name a hue.
enum Ink {
    static let canvas = Color(light: Palette.paper, dark: Palette.night900)
    /// The sidebar's own opaque surface (a touch off the canvas) and the line at its trailing edge.
    static let sidebar = Color(light: Palette.paperDeep, dark: Palette.night850)
    static let sidebarEdge = Color(light: 0x1E232D, dark: Palette.slate400, lightAlpha: 0.10, darkAlpha: 0.14)
    static let raised = Color(light: Palette.white, dark: Palette.night800)
    static let hairline = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.14, darkAlpha: 0.11)
    static let hairlineStrong = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.24, darkAlpha: 0.20)
    static let hover = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.06, darkAlpha: 0.08)
    static let track = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.12, darkAlpha: 0.14)

    static let text = Color(light: Palette.ink900, dark: Palette.snow)
    static let textMuted = Color(light: Palette.ink600, dark: Palette.slate300)
    static let textFaint = Color(light: Palette.ink500, dark: Palette.slate500)

    /// The one accent: fills (buttons, chart line, selection marks) and accent-colored text.
    static let brass = Color(light: Palette.brass600, dark: Palette.brass500)
    static let brassFill = Color(light: Palette.brass500, dark: Palette.brass500)
    static let brassText = Color(light: Palette.brass700, dark: Palette.brass300)
    static let onBrass = Color(light: Palette.brassInk, dark: Palette.brassInk)

    static let ok = Color(light: Palette.mint700, dark: Palette.mint400)
    static let wait = Color(light: Palette.amber700, dark: Palette.amber400)
    static let bad = Color(light: Palette.rose700, dark: Palette.rose400)
    static let off = Color(light: Palette.gray400, dark: Palette.gray600)
}

/// Layer 3: sizes. Pages use `Space` (UI/Grid.swift) for margins and gaps.
enum Dim {
    static let radiusButton: CGFloat = 9
    static let radiusRow: CGFloat = 8
    static let radiusCard: CGFloat = 12
    static let radiusField: CGFloat = 9
    static let buttonHeight: CGFloat = 34
    static let buttonHeightSmall: CGFloat = 28
    static let fieldHeight: CGFloat = 36
    static let sidebarRow: CGFloat = 32
    static let sidebarWidth: CGFloat = 216
    static let hairline: CGFloat = 1
    static let dot: CGFloat = 7
    static let dotBox: CGFloat = 13
    static let bar: CGFloat = 3
    static let segment: CGFloat = 28
    static let mark: CGFloat = 15
    static let icon: CGFloat = 16
    static let rowPadding: CGFloat = 14
    /// Rows of a page that has to fit a short window (the Environment step).
    static let rowPaddingTight: CGFloat = 8
    /// Heading to content (the reference's h2 margin), and the gap between choice cards.
    static let sectionGap: CGFloat = 14
    static let cardGap: CGFloat = 10
    static let heroValue: CGFloat = 56
    /// The Overview chart. The reference draws a 760 x 250 viewBox about 440 pt wide, so about 145 pt tall with its axis
    /// labels; the extra room here is the tooltip's.
    static let chartHeight: CGFloat = 160
    /// The chart of a running optimization has the page to itself.
    static let chartHeightDetail: CGFloat = 248
    static let textLine: CGFloat = 18
    /// Line heights of the reference (line-height 1.45): 13.5 px text 19.6, 12.5 px text 18. The bundled sans is set
    /// tighter by default, so text roles get these as a minimum height.
    /// The page title: font size 32, line-height 1.
    static let titleLine: CGFloat = 32
    static let lineBody: CGFloat = 20
    static let lineMeta: CGFloat = 18
    static let markStroke: CGFloat = 1.5
}

enum Motion {
    static let hover = Animation.easeOut(duration: 0.12)
    static let step = Animation.easeOut(duration: 0.2)
}

// MARK: - Color helpers

/// Pure color math with no actor, so the dynamic color below can call it from any thread.
enum ColorMath {
    nonisolated static func ns(_ hex: UInt32, _ alpha: Double) -> NSColor {
        NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: CGFloat(alpha))
    }
}

extension Color {
    /// A color with a light and a dark value, both hex with an optional alpha.
    init(light: UInt32, dark: UInt32, lightAlpha: Double = 1, darkAlpha: Double = 1) {
        self.init(nsColor: NSColor(name: nil) { @Sendable appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return isDark ? ColorMath.ns(dark, darkAlpha) : ColorMath.ns(light, lightAlpha)
        })
    }
}
