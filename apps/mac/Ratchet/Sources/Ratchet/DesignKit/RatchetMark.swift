import AppKit
import SwiftUI

/// The Ratchet mark: five quarter-discs whose radii grow by the golden ratio. Four sealed chambers in glass, the newest
/// in brass. Geometry comes from design/icon/ratchet-mark.svg (viewBox 218 338 600 369) and ratchet-menubar.svg.
struct RatchetMarkGeometry: Sendable {
    /// One quarter disc: the right-angle corner, then the two ends of the arc.
    struct Chamber: Sendable {
        let center: CGPoint
        let start: CGPoint
        let end: CGPoint
        let opacity: Double
    }

    let viewBox: CGRect
    /// Oldest (smallest) first; the last one is the newest chamber, drawn in brass.
    let chambers: [Chamber]

    static let aspect: CGFloat = 600.0 / 369.0

    static let color = RatchetMarkGeometry(
        viewBox: CGRect(x: 218, y: 338, width: 600, height: 369), // grid: ok SVG units
        chambers: [
            .init(center: CGPoint(x: 397.8, y: 434.8), start: CGPoint(x: 397.8, y: 466.2), end: CGPoint(x: 366.4, y: 434.8), opacity: 0.12),
            .init(center: CGPoint(x: 407.7, y: 434.8), start: CGPoint(x: 407.7, y: 466.2), end: CGPoint(x: 439.2, y: 434.8), opacity: 0.18),
            .init(center: CGPoint(x: 361.6, y: 424.9), start: CGPoint(x: 439.2, y: 424.9), end: CGPoint(x: 361.6, y: 347.3), opacity: 0.28),
            .init(center: CGPoint(x: 351.7, y: 471.0), start: CGPoint(x: 227.9, y: 471.0), end: CGPoint(x: 351.7, y: 347.3), opacity: 0.42),
            .init(center: CGPoint(x: 444.0, y: 480.9), start: CGPoint(x: 444.0, y: 697.0), end: CGPoint(x: 227.9, y: 480.9), opacity: 0.6),
            .init(center: CGPoint(x: 453.9, y: 342.5), start: CGPoint(x: 453.9, y: 697.0), end: CGPoint(x: 808.4, y: 342.5), opacity: 1),
        ])

    /// The one-colour version (ratchet-menubar.svg): slightly smaller discs so the gaps survive at 18 pt.
    static let template = RatchetMarkGeometry(
        viewBox: CGRect(x: 218, y: 338, width: 600, height: 369), // grid: ok SVG units
        chambers: [
            .init(center: CGPoint(x: 395.7, y: 436.9), start: CGPoint(x: 395.7, y: 462.0), end: CGPoint(x: 370.6, y: 436.9), opacity: 0.3),
            .init(center: CGPoint(x: 409.9, y: 436.9), start: CGPoint(x: 409.9, y: 462.0), end: CGPoint(x: 435.0, y: 436.9), opacity: 0.4),
            .init(center: CGPoint(x: 363.7, y: 422.8), start: CGPoint(x: 435.0, y: 422.8), end: CGPoint(x: 363.7, y: 351.4), opacity: 0.55),
            .init(center: CGPoint(x: 349.6, y: 468.9), start: CGPoint(x: 232.1, y: 468.9), end: CGPoint(x: 349.6, y: 351.4), opacity: 0.7),
            .init(center: CGPoint(x: 441.9, y: 483.0), start: CGPoint(x: 441.9, y: 692.8), end: CGPoint(x: 232.1, y: 483.0), opacity: 0.85),
            .init(center: CGPoint(x: 456.0, y: 344.6), start: CGPoint(x: 456.0, y: 692.8), end: CGPoint(x: 804.2, y: 344.6), opacity: 1),
        ])

    /// A chamber's outline in view-box coordinates (y down, like the SVG).
    func path(of chamber: Chamber) -> Path {
        let k: CGFloat = 0.5522847498
        let c = chamber.center, a = chamber.start, b = chamber.end
        var path = Path()
        path.move(to: c)
        path.addLine(to: a)
        path.addCurve(to: b,
                      control1: CGPoint(x: a.x + k * (b.x - c.x), y: a.y + k * (b.y - c.y)),
                      control2: CGPoint(x: b.x + k * (a.x - c.x), y: b.y + k * (a.y - c.y)))
        path.closeSubpath()
        return path
    }

    /// Maps the view box onto `rect`, keeping the aspect ratio and centering.
    func transform(in rect: CGRect) -> CGAffineTransform {
        let scale = min(rect.width / viewBox.width, rect.height / viewBox.height)
        let w = viewBox.width * scale, h = viewBox.height * scale
        return CGAffineTransform(translationX: rect.minX + (rect.width - w) / 2, y: rect.minY + (rect.height - h) / 2)
            .scaledBy(x: scale, y: scale)
            .translatedBy(x: -viewBox.minX, y: -viewBox.minY)
    }
}

/// One chamber as a SwiftUI shape.
private struct ChamberShape: Shape {
    let geometry: RatchetMarkGeometry
    let index: Int

    func path(in rect: CGRect) -> Path {
        geometry.path(of: geometry.chambers[index]).applying(geometry.transform(in: rect))
    }
}

/// The mark in color: glass chambers (a fixed blue-grey at falling opacity) and the brass newest chamber with its
/// gradient. Same colors in light and dark; the glass opacities carry on both sidebars.
struct RatchetMarkView: View {
    var height: CGFloat = 20

    private static let glass = Color(red: 0x8E / 255, green: 0x9B / 255, blue: 0xB2 / 255)
    private static let brass = LinearGradient(
        stops: [
            .init(color: Color(red: 0xF8 / 255, green: 0xE4 / 255, blue: 0xB8 / 255), location: 0),
            .init(color: Color(red: 0xE2 / 255, green: 0xBE / 255, blue: 0x7C / 255), location: 0.6),
            .init(color: Color(red: 0xBE / 255, green: 0x8F / 255, blue: 0x49 / 255), location: 1),
        ],
        startPoint: .topLeading, endPoint: .bottomTrailing)

    var body: some View {
        let geometry = RatchetMarkGeometry.color
        ZStack {
            ForEach(0..<geometry.chambers.count - 1, id: \.self) { index in
                ChamberShape(geometry: geometry, index: index)
                    .fill(Self.glass.opacity(geometry.chambers[index].opacity))
            }
            ChamberShape(geometry: geometry, index: geometry.chambers.count - 1)
                .fill(Self.brass)
        }
        .frame(width: height * RatchetMarkGeometry.aspect, height: height)
        .accessibilityHidden(true)
    }
}

enum RatchetMarkImage {
    /// The menu bar image: black at the template's opacities, flagged as a template so the system tints it.
    static func menuBar(height: CGFloat = 16) -> NSImage {
        let geometry = RatchetMarkGeometry.template
        let size = NSSize(width: height * RatchetMarkGeometry.aspect, height: height)
        let image = NSImage(size: size, flipped: true) { rect in
            guard let context = NSGraphicsContext.current?.cgContext else { return false }
            let transform = geometry.transform(in: rect)
            for chamber in geometry.chambers {
                let path = geometry.path(of: chamber).applying(transform)
                context.setFillColor(CGColor(gray: 0, alpha: chamber.opacity))
                context.addPath(path.cgPath)
                context.fillPath()
            }
            return true
        }
        image.isTemplate = true
        return image
    }
}
