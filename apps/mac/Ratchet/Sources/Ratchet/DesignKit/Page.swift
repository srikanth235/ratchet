import RatchetKit
import SwiftUI

// Page-level parts: the scrolling page, its header, section headings, the hero number, the range switch.

/// A scrolling page on the canvas, with the design's generous margins.
struct KitPage<Content: View>: View {
    var maxWidth: CGFloat?
    let content: Content

    init(maxWidth: CGFloat? = nil, @ViewBuilder content: () -> Content) {
        self.maxWidth = maxWidth
        self.content = content()
    }

    @State private var focus = SnapshotFocus.shared

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: Space.block) {
                    content
                }
                .frame(maxWidth: maxWidth ?? .infinity, alignment: .leading)
                .padding(.horizontal, Space.pageX)
                .padding(.top, Space.pageTop)
                .padding(.bottom, Space.pageBottom)
                .frame(maxWidth: .infinity, alignment: .leading)
                .id("page-top")
            }
            .background(Ink.canvas)
            .task(id: focus.target) {
                // Snapshot mode only: bring the section of the scene into view once its data is there.
                guard Store.isOverridden else { return }
                try? await Task.sleep(for: .milliseconds(300))
                proxy.scrollTo(focus.target ?? "page-top", anchor: .top)
            }
        }
    }
}

/// Serif title with a grey line under it, and controls on the trailing side.
struct PageHeader<Trailing: View>: View {
    let title: String
    var subtitle: String?
    let trailing: Trailing

    init(_ title: String, subtitle: String? = nil, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.subtitle = subtitle
        self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .center, spacing: Space.lg) {
            VStack(alignment: .leading, spacing: Space.sm) {
                Text(title).kitType(.pageTitle).foregroundStyle(Ink.text).frame(height: Dim.titleLine)
                if let subtitle {
                    Text(subtitle).kitType(.body).foregroundStyle(Ink.textFaint)
                }
            }
            Spacer(minLength: Space.lg)
            trailing
        }
    }
}

extension PageHeader where Trailing == EmptyView {
    init(_ title: String, subtitle: String? = nil) {
        self.init(title, subtitle: subtitle) { EmptyView() }
    }
}

/// A small heading over a block ("Needs you", "GitHub").
struct KitSection<Content: View>: View {
    let title: String
    let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Dim.sectionGap) {
            Text(title).kitType(.section).foregroundStyle(Ink.textMuted)
            content
        }
    }
}

/// A big serif number with a label above and a note below. `progress` adds a thin brass bar.
struct HeroNumber: View {
    let label: String
    let value: String
    var unit: String?
    let note: String
    var progress: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            if !label.isEmpty {
                Text(label).kitType(.meta).foregroundStyle(Ink.textFaint)
            }
            HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                Text(value)
                    .kitType(.hero)
                    .foregroundStyle(Ink.text)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                if let unit {
                    Text(unit).kitType(.heroUnit).foregroundStyle(Ink.textMuted)
                }
            }
            if let progress {
                ThinBar(fraction: progress).padding(.top, Space.sm)
            }
            Text(note).kitType(.meta).foregroundStyle(Ink.textFaint).padding(.top, Space.xs)
        }
        .accessibilityElement(children: .combine)
    }
}

struct ThinBar: View {
    let fraction: Double

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule().fill(Ink.track)
                Capsule().fill(Ink.brassFill).frame(width: proxy.size.width * min(max(fraction, 0), 1))
            }
        }
        .frame(height: Dim.bar)
        .accessibilityHidden(true)
    }
}

/// Three hero numbers side by side, divided by hairlines.
struct HeroRow<A: View, B: View, C: View>: View {
    let first: A
    let second: B
    let third: C

    init(@ViewBuilder first: () -> A, @ViewBuilder second: () -> B, @ViewBuilder third: () -> C) {
        self.first = first()
        self.second = second()
        self.third = third()
    }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            first.frame(maxWidth: .infinity, alignment: .leading).padding(.trailing, Space.section)
            VHairline()
            second.frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, Space.section)
            VHairline()
            third.frame(maxWidth: .infinity, alignment: .leading).padding(.leading, Space.section)
        }
        .fixedSize(horizontal: false, vertical: true)
    }
}

/// A row of text segments, one selected: "30 days · 90 days · All time", "All · Running · Done".
struct KitSwitch<Item: Identifiable & Hashable>: View {
    let items: [Item]
    let selection: Item
    let title: (Item) -> String
    let onSelect: (Item) -> Void

    var body: some View {
        HStack(spacing: Space.xs / 2) {
            ForEach(items) { item in
                Button(title(item)) { onSelect(item) }
                    .buttonStyle(RangeSegmentStyle(isSelected: item == selection))
                    .accessibilityAddTraits(item == selection ? .isSelected : [])
            }
        }
    }
}

/// "30 days · 90 days · All time".
struct RangeSwitch: View {
    let selection: TimeRange
    let onSelect: (TimeRange) -> Void

    var body: some View {
        KitSwitch(items: TimeRange.allCases, selection: selection, title: { $0.title }, onSelect: onSelect)
    }
}

private struct RangeSegmentStyle: ButtonStyle {
    let isSelected: Bool

    func makeBody(configuration: Configuration) -> some View {
        RangeSegmentBody(configuration: configuration, isSelected: isSelected)
    }

    private struct RangeSegmentBody: View {
        let configuration: ButtonStyleConfiguration
        let isSelected: Bool
        @State private var hovering = false

        var body: some View {
            configuration.label
                .kitType(.caption)
                .foregroundStyle(isSelected ? Ink.text : Ink.textFaint)
                .padding(.horizontal, Space.md - 2)
                .padding(.vertical, Space.xs + 1)
                .background(RoundedRectangle(cornerRadius: Dim.radiusRow - 1, style: .continuous)
                    .fill(isSelected || hovering ? Ink.hover : Color.clear))
                .contentShape(.rect)
                .onHover { hovering = $0 }
        }
    }
}

/// A single-line field in the design's look.
struct KitField: ViewModifier {
    @FocusState private var focused: Bool

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusField, style: .continuous)
        content
            .textFieldStyle(.plain)
            .kitType(.body)
            .focused($focused)
            .padding(.horizontal, Space.md)
            .frame(height: Dim.fieldHeight)
            .background(shape.fill(Ink.raised.opacity(0.5)))
            .overlay(shape.strokeBorder(focused ? Ink.brassFill.opacity(0.7) : Ink.hairlineStrong, lineWidth: Dim.hairline))
    }
}

extension View {
    func kitField() -> some View {
        modifier(KitField())
    }
}
