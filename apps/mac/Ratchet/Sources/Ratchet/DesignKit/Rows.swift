import SwiftUI

// Lines, status, list rows and sidebar rows.

struct Hairline: View {
    var body: some View {
        Rectangle().fill(Ink.hairline).frame(height: Dim.hairline)
    }
}

struct VHairline: View {
    var body: some View {
        Rectangle().fill(Ink.hairline).frame(width: Dim.hairline)
    }
}

enum StatusTone {
    case ok, run, wait, off, bad

    var color: Color {
        switch self {
        case .ok: Ink.ok
        case .run: Ink.brass
        case .wait: Ink.wait
        case .off: Ink.off
        case .bad: Ink.bad
        }
    }
}

/// A small colored dot. A running dot has a soft halo.
struct StatusDot: View {
    let tone: StatusTone

    var body: some View {
        ZStack {
            if tone == .run {
                Circle().fill(Ink.brass.opacity(0.2)).frame(width: Dim.dotBox, height: Dim.dotBox)
            }
            Circle().fill(tone.color).frame(width: Dim.dot, height: Dim.dot)
        }
        .frame(width: Dim.dotBox, height: Dim.dotBox)
        .accessibilityHidden(true)
    }
}

/// A status dot and a word, with an optional grey line under it. The word carries the meaning; the color backs it.
struct StatusLabel: View {
    let tone: StatusTone
    let text: String
    var caption: String?

    var body: some View {
        HStack(alignment: .top, spacing: Space.xs) {
            StatusDot(tone: tone).frame(height: Dim.textLine)
            VStack(alignment: .leading, spacing: 0) {
                Text(text).kitType(.body).foregroundStyle(Ink.text)
                if let caption {
                    Text(caption).kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// Title with a grey line under it, as in every list of the design.
struct TitleStack: View {
    let title: String
    var subtitle: String?
    /// How many lines the grey line may take before it is cut. One keeps table rows level; rows with free text
    /// (Needs you, resources) allow two.
    var subtitleLines = 1

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
            if let subtitle {
                Text(subtitle).kitType(.meta).foregroundStyle(Ink.textFaint)
                    .lineLimit(subtitleLines)
                    .fixedSize(horizontal: false, vertical: subtitleLines > 1)
            }
        }
        // The text column gives way last: the columns beside it shrink first.
        .layoutPriority(1)
    }
}

/// A list: a hairline above, each row closed by a hairline.
struct HairlineList<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Hairline()
            content
        }
    }
}

/// One row of a hairline list.
struct HairlineRow<Content: View>: View {
    var verticalPadding = Dim.rowPadding
    let content: Content

    init(verticalPadding: CGFloat = Dim.rowPadding, @ViewBuilder content: () -> Content) {
        self.verticalPadding = verticalPadding
        self.content = content()
    }

    var body: some View {
        HStack(spacing: Space.lg) {
            content
        }
        .padding(.vertical, verticalPadding)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .bottom) { Hairline() }
    }
}

/// A tappable table row: hover tint, a hairline underneath.
struct TableRowButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        TableRowBody(configuration: configuration)
    }

    private struct TableRowBody: View {
        let configuration: ButtonStyleConfiguration
        @State private var hovering = false

        var body: some View {
            configuration.label
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(configuration.isPressed || hovering ? Ink.hover : Color.clear)
                .overlay(alignment: .bottom) { Hairline() }
                .contentShape(.rect)
                .onHover { hovering = $0 }
                .animation(Motion.hover, value: hovering)
        }
    }
}

/// A sidebar row: quiet icon and word, a soft fill and a brass icon when selected.
struct SidebarRow: View {
    let title: String
    let icon: KitIcon
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: Space.md) {
                KitIconView(icon: icon)
                    .foregroundStyle(isSelected ? Ink.brass : Ink.textFaint)
                Text(title)
                    .kitType(.body)
                    .foregroundStyle(isSelected ? Ink.text : Ink.textFaint)
                Spacer(minLength: 0)
            }
        }
        .buttonStyle(SidebarRowStyle(isSelected: isSelected))
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}

private struct SidebarRowStyle: ButtonStyle {
    let isSelected: Bool

    func makeBody(configuration: Configuration) -> some View {
        SidebarRowBody(configuration: configuration, isSelected: isSelected)
    }

    private struct SidebarRowBody: View {
        let configuration: ButtonStyleConfiguration
        let isSelected: Bool
        @State private var hovering = false
        @Environment(\.appearsActive) private var appearsActive

        var body: some View {
            let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
            configuration.label
                .padding(.horizontal, Space.md)
                .frame(maxWidth: .infinity, minHeight: Dim.sidebarRow, alignment: .leading)
                .background(shape.fill(fill))
                .contentShape(shape)
                .onHover { hovering = $0 }
                .animation(Motion.hover, value: hovering)
        }

        private var fill: Color {
            if isSelected { return Ink.hover.opacity(appearsActive ? 1 : 0.6) }
            return configuration.isPressed || hovering ? Ink.hover.opacity(0.6) : .clear
        }
    }
}
