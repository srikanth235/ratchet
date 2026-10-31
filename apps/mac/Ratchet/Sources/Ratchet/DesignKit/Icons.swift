import SwiftUI

/// The icon set. Views name a `KitIcon`; the drawing lives here. One 16 x 16 box, one round stroke.
/// Shapes follow the SVGs of design/v02.html.
enum KitIcon {
    case overview, optimizations, benchmarks, connectors, check, back, plus
}

private struct KitGlyph: Shape {
    let icon: KitIcon

    func path(in rect: CGRect) -> Path {
        var path = Path()
        switch icon {
        case .overview:
            path.move(to: CGPoint(x: 2.5, y: 11.5))
            path.addCurve(to: CGPoint(x: 13.5, y: 11.5), control1: CGPoint(x: 2.5, y: 3.5), control2: CGPoint(x: 13.5, y: 3.5))
            path.move(to: CGPoint(x: 8, y: 11.5))
            path.addLine(to: CGPoint(x: 10.8, y: 7.9))
        case .optimizations:
            path.addLines([
                CGPoint(x: 2, y: 13.5), CGPoint(x: 5.5, y: 6), CGPoint(x: 5.5, y: 13.5), CGPoint(x: 9, y: 6),
                CGPoint(x: 9, y: 13.5), CGPoint(x: 12.5, y: 6), CGPoint(x: 12.5, y: 13.5),
            ])
        case .benchmarks:
            for (x, top) in [(3.0, 9.0), (6.3, 5.0), (9.7, 7.5), (13.0, 3.0)] {
                path.move(to: CGPoint(x: x, y: 13.5))
                path.addLine(to: CGPoint(x: x, y: top))
            }
        case .connectors:
            path.move(to: CGPoint(x: 6, y: 10))
            path.addLine(to: CGPoint(x: 10, y: 6))
            path.move(to: CGPoint(x: 5, y: 7.5))
            path.addLine(to: CGPoint(x: 3.5, y: 9))
            path.addQuadCurve(to: CGPoint(x: 7, y: 12.5), control: CGPoint(x: 2, y: 11.5))
            path.addLine(to: CGPoint(x: 8.5, y: 11))
            path.move(to: CGPoint(x: 11, y: 8.5))
            path.addLine(to: CGPoint(x: 12.5, y: 7))
            path.addQuadCurve(to: CGPoint(x: 9, y: 3.5), control: CGPoint(x: 14, y: 4.5))
            path.addLine(to: CGPoint(x: 7.5, y: 5))
        case .check:
            path.addLines([CGPoint(x: 3, y: 8.5), CGPoint(x: 6, y: 11.5), CGPoint(x: 13, y: 4.5)])
        case .back:
            path.addLines([CGPoint(x: 10, y: 3), CGPoint(x: 5, y: 8), CGPoint(x: 10, y: 13)])
        case .plus:
            path.move(to: CGPoint(x: 8, y: 3))
            path.addLine(to: CGPoint(x: 8, y: 13))
            path.move(to: CGPoint(x: 3, y: 8))
            path.addLine(to: CGPoint(x: 13, y: 8))
        }
        let scale = rect.width / 16
        return path.applying(CGAffineTransform(scaleX: scale, y: scale))
    }
}

struct KitIconView: View {
    let icon: KitIcon
    var size: CGFloat = Dim.icon
    var weight: CGFloat = 1.5

    var body: some View {
        KitGlyph(icon: icon)
            .stroke(style: StrokeStyle(lineWidth: weight * size / 16, lineCap: .round, lineJoin: .round))
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}
