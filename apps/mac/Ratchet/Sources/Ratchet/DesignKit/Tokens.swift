import AppKit
import SwiftUI

// Midnight Brass, calm version, v0.3.1. Three layers (design-kit skill, design/KIT.md, design/DESIGN-SYSTEM.md):
//   1. Palette: raw colors, named by hue and step. Only this file reads them.
//   2. Ink: meaning (text, canvas, accent, status), each with a light, a dark and an Increase Contrast value.
//      Parts read only this layer.
//   3. Dim, Layout, Motion: sizes, window and column widths, timing. KitType (Type.swift) holds the type roles.
// Source of the values: design/tools/v03.css (the mockup source of v0.3.1; its two token layers carry these names).
// tools/grid-lint.sh lints this file like any other: every size is 0, 1, 2 or a multiple of 4, or says why not.

/// Layer 1.
enum Palette {
    // Dark ground
    static let night950: UInt32 = 0x070B12
    static let night900: UInt32 = 0x0B1019
    static let night850: UInt32 = 0x0E1420
    static let night800: UInt32 = 0x101724
    static let night700: UInt32 = 0x1A2233
    static let slate400: UInt32 = 0x94A3BE
    static let snow: UInt32 = 0xECEFF4
    static let slate300: UInt32 = 0xA7B1C2
    static let slate200: UInt32 = 0xC9D1DF
    static let slate500: UInt32 = 0x7E8CA3
    // Light ground
    static let paper: UInt32 = 0xFBFAF6
    static let paperDeep: UInt32 = 0xF3F1EA
    static let white: UInt32 = 0xFFFFFF
    static let umber: UInt32 = 0x3C3220
    static let umberDeep: UInt32 = 0x2A2316
    static let ink900: UInt32 = 0x15181F
    static let ink700: UInt32 = 0x343C4B
    static let ink600: UInt32 = 0x4A5363
    /// v0.3.1: was 0x6B7587 (4.4:1 on paper, failed AA as text).
    static let ink500: UInt32 = 0x5C6676
    // Brass
    static let brass500: UInt32 = 0xD9B26F
    static let brass300: UInt32 = 0xEBCB8E
    static let brass600: UInt32 = 0xB8861F
    static let brass700: UInt32 = 0x8A5E0B
    static let brass800: UInt32 = 0x6B4708
    static let brassInk: UInt32 = 0x16110A
    // Status
    static let mint400: UInt32 = 0x7FD1AE
    /// v0.3.1: was 0x1F8A62 (4.1:1 as text on paper).
    static let mint700: UInt32 = 0x187250
    static let amber400: UInt32 = 0xE9B85C
    /// F3: was 0xA86F0B (4.1:1 as text on paper, the same AA miss as mint). v03.css still has the old value.
    static let amber700: UInt32 = 0x956108
    static let rose400: UInt32 = 0xE58C8C
    static let rose700: UInt32 = 0xB4414A
    static let gray600: UInt32 = 0x4A5568
    static let gray400: UInt32 = 0x9AA3B2
    // The brand mark (RatchetMark.swift): glass chambers and the brass newest chamber.
    static let markSlate: UInt32 = 0x8E9BB2
    static let markHi: UInt32 = 0xF8E4B8
    static let markMid: UInt32 = 0xE2BE7C
    static let markLo: UInt32 = 0xBE8F49
}

/// Layer 2: what a color means. Parts never name a hue. Dark values come from design/v03.html; light values are the same
/// palette on warm paper. Increase Contrast has its own value where the normal one is thin (lines, faint text, brass text).
enum Ink {
    static let canvas = Color(light: Palette.paper, dark: Palette.night900)
    /// The sidebar's own opaque surface (a touch off the canvas) and the line at its trailing edge.
    static let sidebar = Color(light: Palette.paperDeep, dark: Palette.night850)
    static let sidebarEdge = Color(light: 0x1E232D, dark: Palette.slate400, lightAlpha: 0.10, darkAlpha: 0.14)
    static let raised = Color(light: Palette.white, dark: Palette.night800)
    /// Cards and the composer: a step under `raised`.
    static let raisedSoft = Color(light: Palette.white, dark: Palette.night850)
    /// Popovers, tooltips, bubbles.
    static let glass = Color(light: Palette.white, dark: Palette.night700)
    static let scrim = Color(light: Palette.umberDeep, dark: 0x030509, lightAlpha: 0.32, darkAlpha: 0.62)

    /// Dividers.
    static let hairline = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.14, darkAlpha: 0.11,
                                contrastLightAlpha: 0.4, contrastDarkAlpha: 0.4)
    /// Card edges. Not for controls: those use `controlBorder`.
    static let hairlineStrong = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.24, darkAlpha: 0.20,
                                      contrastLightAlpha: 0.6, contrastDarkAlpha: 0.6)
    /// Control outlines (buttons, fields, chips, toggles, segmented): 3:1 on canvas, sidebar, raised and glass (WCAG 1.4.11).
    static let controlBorder = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.58, darkAlpha: 0.60,
                                     contrastLightAlpha: 0.9, contrastDarkAlpha: 0.95)
    static let hover = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.06, darkAlpha: 0.08)
    static let track = Color(light: Palette.umber, dark: Palette.slate400, lightAlpha: 0.12, darkAlpha: 0.14)

    static let text = Color(light: Palette.ink900, dark: Palette.snow)
    static let textMuted = Color(light: Palette.ink600, dark: Palette.slate300,
                                 contrastLight: Palette.ink700, contrastDark: Palette.slate200)
    static let textFaint = Color(light: Palette.ink500, dark: Palette.slate500,
                                 contrastLight: Palette.ink700, contrastDark: Palette.slate200)

    /// The one accent: fills (buttons, chart line, selection marks) and accent-colored text.
    static let brass = Color(light: Palette.brass600, dark: Palette.brass500, contrastLight: Palette.brass700)
    static let brassFill = Color(light: Palette.brass500, dark: Palette.brass500)
    static let brassText = Color(light: Palette.brass700, dark: Palette.brass300, contrastLight: Palette.brass800)
    static let onBrass = Color(light: Palette.brassInk, dark: Palette.brassInk)
    /// A selected card or row: a faint wash and a brass edge.
    static let brassWash = Color(light: Palette.brass600, dark: Palette.brass500, lightAlpha: 0.08, darkAlpha: 0.06)
    static let brassBorder = Color(light: Palette.brass700, dark: Palette.brass500, lightAlpha: 0.5, darkAlpha: 0.5,
                                   contrastLightAlpha: 1, contrastDarkAlpha: 1)
    /// The keyboard focus ring.
    static let focus = Color(light: Palette.brass700, dark: Palette.brass300)

    static let ok = Color(light: Palette.mint700, dark: Palette.mint400)
    static let wait = Color(light: Palette.amber700, dark: Palette.amber400)
    static let bad = Color(light: Palette.rose700, dark: Palette.rose400)
    static let off = Color(light: Palette.gray400, dark: Palette.gray600)
    static let okWash = Color(light: Palette.mint700, dark: Palette.mint400, lightAlpha: 0.10, darkAlpha: 0.12)
    static let waitWash = Color(light: Palette.amber700, dark: Palette.amber400, lightAlpha: 0.08, darkAlpha: 0.10)
    static let badWash = Color(light: Palette.rose700, dark: Palette.rose400, lightAlpha: 0.08, darkAlpha: 0.10)

    /// The brand mark: the same colors in light and dark.
    static let markGlass = Color(light: Palette.markSlate, dark: Palette.markSlate)
    static let markHi = Color(light: Palette.markHi, dark: Palette.markHi)
    static let markMid = Color(light: Palette.markMid, dark: Palette.markMid)
    static let markLo = Color(light: Palette.markLo, dark: Palette.markLo)
}

/// Layer 3: sizes. Pages use `Space` (UI/Grid.swift) for margins and gaps.
enum Dim {
    // Radii: 4 chips and checkboxes, 8 controls and rows, 12 cards, 16 composer and window.
    static let radiusXS: CGFloat = 4
    static let radiusButton: CGFloat = 8
    static let radiusRow: CGFloat = 8
    static let radiusField: CGFloat = 8
    static let radiusCard: CGFloat = 12
    static let radiusLarge: CGFloat = 16
    // Control heights: button 32, small 28, field 36, sidebar row 32. 28 is also the smallest hit target.
    static let buttonHeight: CGFloat = 32
    static let buttonHeightSmall: CGFloat = 28
    static let fieldHeight: CGFloat = 36
    static let sidebarRow: CGFloat = 32
    static let sidebarWidth: CGFloat = 216
    static let chipHeight: CGFloat = 28
    static let segmentedHeight: CGFloat = 32
    static let toggleWidth: CGFloat = 40
    static let toggleHeight: CGFloat = 24
    static let toggleKnob: CGFloat = 16
    static let sendButton: CGFloat = 32
    static let composerMinHeight: CGFloat = 48
    /// One opacity for every disabled part.
    static let disabledOpacity: Double = 0.45
    /// Custom chrome while its window is not in front (`dimsWhenInactive`).
    static let inactiveOpacity: Double = 0.6
    static let focusRing: CGFloat = 2
    static let focusOffset: CGFloat = 2

    static let hairline: CGFloat = 1
    static let dot: CGFloat = 8
    static let dotBox: CGFloat = 12
    static let bar: CGFloat = 4
    static let segment: CGFloat = 28
    static let mark: CGFloat = 16
    static let icon: CGFloat = 16
    static let statusShape: CGFloat = 16
    static let rowPadding: CGFloat = 12
    /// Rows of a page that has to fit a short window (the Environment step).
    static let rowPaddingTight: CGFloat = 8
    /// Heading to content (v0.3.1 h2 margin), and the gap between choice cards.
    static let sectionGap: CGFloat = 12
    static let cardGap: CGFloat = 12
    static let heroValue: CGFloat = 56
    /// The Overview chart. The reference draws a 760 x 250 viewBox about 440 pt wide, so about 145 pt tall with its axis
    /// labels; the extra room here is the tooltip's.
    static let chartHeight: CGFloat = 160
    /// The chart of a running optimization has the page to itself.
    static let chartHeightDetail: CGFloat = 248
    /// Line heights of the reference (line-height 1.45): 12.5 pt text 18, 13.5 pt text 20. The bundled sans is set
    /// tighter by default, so text roles get these as a minimum height.
    static let textLine: CGFloat = 18 // grid: ok text line height (12.5 pt x 1.45)
    static let titleLine: CGFloat = 32
    static let lineBody: CGFloat = 20
    static let lineMeta: CGFloat = 18 // grid: ok text line height (12.5 pt x 1.45)
    static let markStroke: CGFloat = 1.5 // grid: ok stroke width of the mark and the checkbox
}

/// Window, sheet and column widths that more than one screen shares. Screens name these instead of a number.
enum Layout {
    /// Sheets and the Settings window: one width, so nothing resizes between tabs.
    static let sheetWidth: CGFloat = 520
    static let settingsWidth: CGFloat = 520
    /// The menu bar window and popovers.
    static let popoverWidth: CGFloat = 320
    /// Settings navigation, and the label column of key/value rows (160; 128 inside a draft card).
    static let settingsSidebarWidth: CGFloat = 176
    static let labelColumn: CGFloat = 160
    static let labelColumnCompact: CGFloat = 128
    /// Columns of numbers, right aligned: a count or a percent, and a short money or time figure.
    static let numberColumnNarrow: CGFloat = 56
    static let numberColumn: CGFloat = 80
    /// Detail pages: the side column beside the main one.
    static let sideColumn: CGFloat = 360
    static let sideColumnNarrow: CGFloat = 320
    /// The reading width of a page and of the composer.
    static let contentWidth: CGFloat = 1000
    static let composerWidth: CGFloat = 720
    /// The smallest window and the default one.
    static let minWindowWidth: CGFloat = 1080
    static let minWindowHeight: CGFloat = 640
    static let defaultWindowWidth: CGFloat = 1280
    static let defaultWindowHeight: CGFloat = 820
}

/// Timing by intent. Every value is nil under Reduce Motion, so nothing animates and every state still changes.
enum Motion {
    static var reduced: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    /// Hover fills and borders.
    static var hover: Animation? { reduced ? nil : .easeOut(duration: 0.12) }
    /// Presses and small flips (a toggle's knob).
    static var press: Animation? { reduced ? nil : .easeOut(duration: 0.08) }
    /// A selection moving, a step changing.
    static var step: Animation? { reduced ? nil : .easeOut(duration: 0.2) }
    /// A popover or a banner arriving.
    static var appear: Animation? { reduced ? nil : .easeOut(duration: 0.24) }
    /// A spinner. Under Reduce Motion it stands still.
    static var loop: Animation? { reduced ? nil : .linear(duration: 1.2).repeatForever(autoreverses: false) }
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
    /// A color with a light and a dark value, both hex with an optional alpha. The `contrast...` values are used while
    /// Increase Contrast is on; left out, they fall back to the normal value of the same mode.
    init(light: UInt32, dark: UInt32, lightAlpha: Double = 1, darkAlpha: Double = 1,
         contrastLight: UInt32? = nil, contrastDark: UInt32? = nil,
         contrastLightAlpha: Double? = nil, contrastDarkAlpha: Double? = nil) {
        let names: [NSAppearance.Name] = [.aqua, .darkAqua, .accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua]
        self.init(nsColor: NSColor(name: nil) { @Sendable appearance in
            let match = appearance.bestMatch(from: names)
            let isDark = match == .darkAqua || match == .accessibilityHighContrastDarkAqua
            let isContrast = match == .accessibilityHighContrastAqua || match == .accessibilityHighContrastDarkAqua
            if isDark {
                return ColorMath.ns(isContrast ? (contrastDark ?? dark) : dark,
                                    isContrast ? (contrastDarkAlpha ?? darkAlpha) : darkAlpha)
            }
            return ColorMath.ns(isContrast ? (contrastLight ?? light) : light,
                                isContrast ? (contrastLightAlpha ?? lightAlpha) : lightAlpha)
        })
    }
}
