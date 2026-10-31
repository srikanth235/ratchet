import SwiftUI

/// A card you choose: a radio (one of many) or a checkbox (any of several). The top is one real Button; the footer
/// (for example "Runs on this Mac · change") sits below it with its own controls.
enum ChoiceMarkKind { case radio, checkbox }

struct ChoiceCard<Content: View, Footer: View>: View {
    let mark: ChoiceMarkKind
    let isSelected: Bool
    var isEnabled = true
    let action: () -> Void
    let content: Content
    let footer: Footer

    init(mark: ChoiceMarkKind, isSelected: Bool, isEnabled: Bool = true, action: @escaping () -> Void,
         @ViewBuilder content: () -> Content, @ViewBuilder footer: () -> Footer) {
        self.mark = mark
        self.isSelected = isSelected
        self.isEnabled = isEnabled
        self.action = action
        self.content = content()
        self.footer = footer()
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        VStack(alignment: .leading, spacing: Space.md) {
            Button(action: action) {
                HStack(alignment: .top, spacing: Space.md) {
                    ChoiceMark(mark: mark, isSelected: isSelected)
                        .padding(.top, Space.xs / 2)
                    VStack(alignment: .leading, spacing: Space.xs / 2) { content }
                    Spacer(minLength: 0)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled(!isEnabled)
            .accessibilityAddTraits(isSelected ? .isSelected : [])
            footer
                .padding(.leading, Dim.mark + Space.md)
        }
        .padding(.horizontal, Space.lg)
        .padding(.vertical, Dim.rowPadding)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(shape.fill(isSelected ? Ink.brassFill.opacity(0.06) : Color.clear))
        .overlay(shape.strokeBorder(isSelected ? Ink.brassFill.opacity(0.6) : Ink.hairline, lineWidth: Dim.hairline))
        .opacity(isEnabled ? 1 : 0.5)
        .animation(Motion.hover, value: isSelected)
    }
}

extension ChoiceCard where Footer == EmptyView {
    init(mark: ChoiceMarkKind, isSelected: Bool, isEnabled: Bool = true, action: @escaping () -> Void,
         @ViewBuilder content: () -> Content) {
        self.init(mark: mark, isSelected: isSelected, isEnabled: isEnabled, action: action, content: content) { EmptyView() }
    }
}

private struct ChoiceMark: View {
    let mark: ChoiceMarkKind
    let isSelected: Bool

    var body: some View {
        Group {
            switch mark {
            case .radio:
                Circle()
                    .strokeBorder(isSelected ? Ink.brassFill : Ink.hairlineStrong, lineWidth: isSelected ? 4.5 : 1.5)
            case .checkbox:
                RoundedRectangle(cornerRadius: 4, style: .continuous)
                    .fill(isSelected ? Ink.brassFill : Color.clear)
                    .overlay {
                        if isSelected {
                            KitIconView(icon: .check, size: Dim.mark - 3, weight: 2)
                                .foregroundStyle(Ink.onBrass)
                        } else {
                            RoundedRectangle(cornerRadius: 4, style: .continuous)
                                .strokeBorder(Ink.hairlineStrong, lineWidth: Dim.markStroke)
                        }
                    }
            }
        }
        .frame(width: Dim.mark, height: Dim.mark)
        .accessibilityHidden(true)
    }
}

/// "Step 2 of 4 · Connections" with four segments, the done ones in brass.
struct StepProgress: View {
    let current: Int
    let total: Int
    let caption: String

    var body: some View {
        HStack(spacing: Space.md) {
            HStack(spacing: Space.xs) {
                ForEach(0..<total, id: \.self) { index in
                    Capsule()
                        .fill(index <= current ? Ink.brassFill : Ink.track)
                        .frame(width: Dim.segment, height: Dim.bar)
                }
            }
            Text(caption).kitType(.meta).foregroundStyle(Ink.textFaint)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(caption)
    }
}
