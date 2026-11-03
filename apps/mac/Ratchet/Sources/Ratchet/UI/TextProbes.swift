import SwiftUI

/// Snapshot mode only: where every `kitType` text sits in the window, so `tools/check-contrast.swift` can measure the contrast of
/// the real pixels inside each frame (WCAG 1.4.3). Nothing runs outside snapshot mode: `GridProbes.enabled` is false then.
@MainActor
final class TextProbes {
    static let shared = TextProbes()

    struct Entry: Codable {
        var x: Double
        var y: Double
        var width: Double
        var height: Double
        /// The role's point size: 24 and up is large text (3:1), the rest needs 4.5:1.
        var size: Double
        /// A disabled control's text is exempt from the contrast rule.
        var enabled: Bool
    }

    private var entries: [UUID: Entry] = [:]

    func update(_ owner: UUID, _ entry: Entry) { entries[owner] = entry }
    func remove(_ owner: UUID) { entries[owner] = nil }

    /// Writes what is on screen next to a snapshot. `active` says whether the window was the key window when it was captured: an
    /// inactive window is dimmed by the system, and the check skips it.
    func write(to path: String, window: CGSize, active: Bool) {
        struct File: Codable {
            var window: [String: Double]
            var active: Bool
            var texts: [Entry]
        }
        let file = File(
            window: ["width": window.width, "height": window.height], active: active,
            texts: entries.values.sorted { ($0.y, $0.x) < ($1.y, $1.x) })
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        try? encoder.encode(file).write(to: URL(fileURLWithPath: path))
    }
}

private struct TextProbe: ViewModifier {
    let size: CGFloat
    @State private var owner = UUID()
    @Environment(\.isEnabled) private var isEnabled

    func body(content: Content) -> some View {
        if GridProbes.enabled {
            content
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { frame in
                    TextProbes.shared.update(
                        owner,
                        .init(x: frame.minX, y: frame.minY, width: frame.width, height: frame.height, size: size, enabled: isEnabled))
                }
                .onDisappear { TextProbes.shared.remove(owner) }
        } else {
            content
        }
    }
}

extension View {
    /// In snapshot mode, notes this view's frame for the contrast check (a view set in a kit role: text, or a label).
    func textProbe(size: CGFloat) -> some View {
        modifier(TextProbe(size: size))
    }
}
