import SwiftUI

/// A dismissible line for something that went wrong ("Couldn't stop the optimization. …"). It sits at the top of the
/// page, pushes the page down instead of covering its buttons, and goes away on its close button or when the next
/// action starts (`AppStore.lastError`). A triangle and the words carry the meaning; the color only supports them.
struct ErrorBanner: View {
    let message: String
    let dismiss: () -> Void

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        HStack(alignment: .top, spacing: Space.md) {
            KitIconView(icon: .alert)
                .foregroundStyle(Ink.bad)
                .padding(.top, Space.xs)
            Text(message)
                .kitType(.body)
                .foregroundStyle(Ink.text)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, Space.xs)
            Button(action: dismiss) {
                KitIconView(icon: .close)
            }
            .buttonStyle(.kit(.quiet, small: true))
            .help("Dismiss")
            .accessibilityLabel("Dismiss error")
        }
        .padding(.leading, Space.lg)
        .padding(.trailing, Space.sm)
        .padding(.vertical, Space.sm)
        .background(shape.fill(Ink.raised))
        .overlay(shape.strokeBorder(Ink.bad.opacity(0.6), lineWidth: Dim.hairline))
        .accessibilityElement(children: .contain)
        .onAppear { AccessibilityNotification.Announcement(message).post() }
    }
}
