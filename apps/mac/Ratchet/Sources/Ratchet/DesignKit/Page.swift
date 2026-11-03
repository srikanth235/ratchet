import RatchetKit
import SwiftUI

// Page-level parts: the scrolling page, its header, section headings, the hero number, the range switch.

/// A scrolling page on the canvas, with the design's generous margins. Every page has one content width (`Layout.contentWidth`),
/// kept to the leading edge so probes line up; a page that needs another says why with `maxWidth`. A page with a bar that floats
/// at the bottom (the steering field) passes it as `bottomBar`: it is docked under the scroll view, not floating over it, so the
/// content scrolls to a stop above it and the last rows are never under it.
struct KitPage<Content: View, Bar: View>: View {
    var maxWidth: CGFloat?
    let content: Content
    let bottomBar: Bar

    init(maxWidth: CGFloat? = nil, @ViewBuilder content: () -> Content, @ViewBuilder bottomBar: () -> Bar) {
        self.maxWidth = maxWidth
        self.content = content()
        self.bottomBar = bottomBar()
    }

    @State private var focus = SnapshotFocus.shared

    var body: some View {
        // The scroll view and, docked under it, the bar: the content scrolls to a stop above the bar and never runs under it.
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: Space.block) {
                        content
                    }
                    .frame(maxWidth: maxWidth ?? Layout.contentWidth, alignment: .leading)
                    .padding(.horizontal, Space.pageX)
                    .padding(.top, Space.pageTop)
                    .padding(.bottom, Space.pageBottom)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .id("page-top")
                }
                .task(id: focus.target) {
                    // Snapshot mode only: bring the section of the scene into view once its data is there.
                    guard Store.isOverridden else { return }
                    try? await Task.sleep(for: .milliseconds(300))
                    // "page-end" is the bottom of the page: the last rows, where a bar must not cover them.
                    if focus.target == "page-end" {
                        proxy.scrollTo("page-top", anchor: .bottom)
                    } else {
                        proxy.scrollTo(focus.target ?? "page-top", anchor: .top)
                    }
                }
            }
            bottomBar
        }
        .background(Ink.canvas)
    }
}

extension KitPage where Bar == EmptyView {
    init(maxWidth: CGFloat? = nil, @ViewBuilder content: () -> Content) {
        self.init(maxWidth: maxWidth, content: content, bottomBar: { EmptyView() })
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
        .dimsWhenInactive()
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
                    .focusable(false)
                    .accessibilityAddTraits(item == selection ? .isSelected : [])
            }
        }
        .dimsWhenInactive()
        // One tab stop; Left and Right move the selection, as in every segmented control.
        .arrowGroup(radius: Dim.radiusRow) { direction in
            guard let index = items.firstIndex(of: selection) else { return }
            switch direction {
            case .left: onSelect(items[max(index - 1, 0)])
            case .right: onSelect(items[min(index + 1, items.count - 1)])
            default: break
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
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @Environment(\.forcedPartState) private var forced
        @State private var hovering = false

        var body: some View {
            let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: configuration.isPressed,
                                          hovering: hovering, focused: isFocused, selected: isSelected)
            let selected = isSelected || state == .selected
            configuration.label
                .kitType(.caption)
                .foregroundStyle(selected ? Ink.text : Ink.textMuted)
                .padding(.horizontal, Space.md)
                .padding(.vertical, Space.xs)
                .background(RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
                    .fill(selected || state == .hover || state == .pressed ? Ink.hover : Color.clear))
                .kitFocusRing(radius: Dim.radiusRow, visible: state == .focused)
                .opacity(state == .disabled ? Dim.disabledOpacity : 1)
                .contentShape(.rect)
                .onHover { hovering = $0 }
                .focusEffectDisabled()
                .animation(Motion.hover, value: hovering)
        }
    }
}

/// A single-line field in the design's look: control border, brass edge and focus ring while editing.
struct KitField: ViewModifier {
    @FocusState private var focused: Bool
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.forcedPartState) private var forced
    @State private var hovering = false

    func body(content: Content) -> some View {
        let state = PartState.resolve(forced: forced, isEnabled: isEnabled, pressed: false, hovering: hovering,
                                      focused: focused)
        let shape = RoundedRectangle(cornerRadius: Dim.radiusField, style: .continuous)
        content
            .textFieldStyle(.plain)
            .kitType(.body)
            .focused($focused)
            .padding(.horizontal, Space.md)
            .frame(height: Dim.fieldHeight)
            .background(shape.fill(Ink.raised.opacity(0.5)))
            .overlay(shape.strokeBorder(state == .focused || state == .hover ? Ink.brassBorder : Ink.controlBorder,
                                        lineWidth: Dim.hairline))
            .kitFocusRing(radius: Dim.radiusField, visible: state == .focused)
            .opacity(state == .disabled ? Dim.disabledOpacity : 1)
            .onHover { hovering = $0 }
            .animation(Motion.hover, value: hovering)
    }
}

extension View {
    func kitField() -> some View {
        modifier(KitField())
    }
}
