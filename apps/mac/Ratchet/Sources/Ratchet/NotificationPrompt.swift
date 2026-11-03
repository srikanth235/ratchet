import RatchetKit
import SwiftUI

/// "Hear about it when something needs you": one calm card on the Overview, shown after the first optimization exists and
/// before permission was ever asked. Turning it on shows the system's dialog; "Not now" hides the card for good (Settings
/// still has the switch).
struct NotificationPrompt: View {
    @Environment(AppNotifier.self) private var notifier

    var body: some View {
        HStack(alignment: .center, spacing: Space.lg) {
            VStack(alignment: .leading, spacing: Space.xs) {
                Text("Get told when something needs you").kitType(.bodyStrong).foregroundStyle(Ink.text)
                Text("A win ready to merge, an environment to approve, a budget reached, a run that finished. Never anything else.")
                    .kitType(.meta).foregroundStyle(Ink.textMuted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Not now") { notifier.dismissPrompt() }.buttonStyle(.kit(.quiet, small: true))
            Button("Turn on notifications") { Task { await notifier.requestPermission() } }
                .buttonStyle(.kit(.primary, small: true))
        }
        .padding(Space.lg)
        .background(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).fill(Ink.raised.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).strokeBorder(Ink.hairlineStrong, lineWidth: Dim.hairline))
        .accessibilityElement(children: .contain)
    }
}
