import RatchetKit
import SwiftUI

// Small parts the chat screens share: a layout that wraps, the one-line notice, the user's bubble, the step line.

/// Lays its children out left to right and wraps to the next line when the width runs out (the Home chips, the suggestions).
struct FlowLayout: SwiftUI.Layout {
    var spacing: CGFloat = Space.sm

    func sizeThatFits(proposal: ProposedViewSize, subviews: LayoutSubviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0
        var y: CGFloat = 0
        var rowHeight: CGFloat = 0
        var widest: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0 && x + size.width > width {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
            widest = max(widest, x - spacing)
        }
        return CGSize(width: widest, height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: LayoutSubviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > bounds.minX && x + size.width > bounds.maxX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}

/// A quiet one-line note with a close button: "First time: your messages … go to Claude Code. Nothing else."
struct NoticeLine: View {
    let text: String
    var closeLabel = "Dismiss notice"
    let close: () -> Void

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        HStack(alignment: .center, spacing: Space.md) {
            KitIconView(icon: .info).foregroundStyle(Ink.textMuted)
            Text(text).kitType(.meta).foregroundStyle(Ink.textMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            KitIconButton(icon: .close, label: closeLabel, small: true, action: close)
        }
        .padding(.leading, Space.lg)
        .padding(.trailing, Space.sm)
        .padding(.vertical, Space.sm)
        .background(shape.fill(Ink.raised.opacity(0.6)))
        .overlay(shape.strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
        .accessibilityElement(children: .contain)
    }
}

/// What the person said, right-aligned in a rounded box.
struct UserBubble: View {
    let text: String

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        HStack {
            Spacer(minLength: Space.section * 4)
            Text(text)
                .kitType(.body).foregroundStyle(Ink.text)
                .textSelection(.enabled)
                .padding(.horizontal, Space.lg)
                .padding(.vertical, Space.md)
                .background(shape.fill(Ink.raised))
                .overlay(shape.strokeBorder(Ink.hairlineStrong, lineWidth: Dim.hairline))
                .accessibilityLabel("You said: " + text)
        }
    }
}

/// One step of the harness: a shape (never colour alone) and "Read acme/shop · main".
struct StepLine: View {
    let step: ChatToolStep

    private var tone: StatusTone {
        switch step.state {
        case .running: .run
        case .done: .ok
        case .failed: .bad
        }
    }

    private var word: String {
        switch step.state {
        case .running: "Working"
        case .done: "Done"
        case .failed: "Failed"
        }
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
            StatusShape(tone: tone).frame(height: Dim.textLine)
            Text(step.summary.isEmpty ? step.name : step.summary).kitType(.meta).foregroundStyle(Ink.textMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(word): \(step.summary.isEmpty ? step.name : step.summary)")
    }
}

/// A small line of text with an optional trailing button, for "Written by Claude Code · Opus  Retry  Copy".
struct ReplyFooter: View {
    let label: ReplyLabel?
    let canRetry: Bool
    let retry: () -> Void
    let copy: () -> Void

    var body: some View {
        HStack(spacing: Space.md) {
            if let label {
                Text(label.text).kitType(.caption).foregroundStyle(Ink.textFaint)
            }
            if canRetry {
                Button(action: retry) {
                    HStack(spacing: Space.xs) {
                        KitIconView(icon: .undo, size: Dim.icon - Space.xs)
                        Text("Retry")
                    }
                }
                .buttonStyle(.kit(.link, small: true))
                .help("Ask again")
            }
            Button("Copy", action: copy)
                .buttonStyle(.kit(.link, small: true))
                .help("Copy this reply")
        }
    }
}
