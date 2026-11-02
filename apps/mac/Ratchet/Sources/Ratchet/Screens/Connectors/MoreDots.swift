import AppKit
import RatchetKit
import SwiftUI

/// Three dots: the row's actions.
struct MoreDots: View {
    var body: some View {
        HStack(spacing: Space.xs) {
            ForEach(0..<3, id: \.self) { _ in
                Circle().fill(Ink.textFaint).frame(width: Space.xs, height: Space.xs)
            }
        }
        .padding(.horizontal, Space.sm)
        .padding(.vertical, Space.sm)
        .contentShape(.rect)
    }
}
