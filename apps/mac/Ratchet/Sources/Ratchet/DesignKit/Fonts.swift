import AppKit
import CoreText
import SwiftUI

/// The design's own faces, bundled in `Fonts/` (build.sh copies them to Contents/Resources/Fonts): Instrument Serif for
/// titles and big numbers, Instrument Sans for text. Both are SIL OFL 1.1 (licence files sit beside the fonts).
/// Each face has a static file, so a PostScript name picks exactly one weight. When a face is missing (a bare
/// `swift run`, a broken bundle) the system fonts stand in and nothing else changes.
enum KitFonts {
    static let serif = "InstrumentSerif-Regular"
    static let sansRegular = "InstrumentSans-Regular"
    static let sansMedium = "InstrumentSans-Medium"
    static let sansSemibold = "InstrumentSans-SemiBold"

    /// Registers the bundled fonts for this process, once. Safe to call from anywhere on the main actor.
    static let registered: Bool = {
        var urls: [URL] = []
        if let folder = Bundle.main.resourceURL?.appending(path: "Fonts", directoryHint: .isDirectory),
           let files = try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil) {
            urls = files.filter { $0.pathExtension == "ttf" || $0.pathExtension == "otf" }
        }
        for url in urls {
            // An error here usually means "already registered" (Info.plist ATSApplicationFontsPath got there first).
            CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
        return !urls.isEmpty
    }()

    static func has(_ postScriptName: String) -> Bool {
        _ = registered
        return NSFont(name: postScriptName, size: 12) != nil
    }

    /// A bundled face at a fixed size, or `fallback` when it is not there.
    static func font(_ name: String, size: CGFloat, fallback: Font) -> Font {
        has(name) ? .custom(name, fixedSize: size) : fallback
    }
}
