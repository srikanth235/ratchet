import AppKit
import RatchetKit
import SwiftUI

/// The menu bar icon, with the number of things that need you next to it.
struct MenuBarLabel: View {
    @Environment(AppStore.self) private var store
    /// Drawn once: the label is evaluated again whenever the count changes, and the image never does.
    private static let image = RatchetMarkImage.menuBar()

    private var shownCount: Int { store.needsYou.filter { $0.kind != .approveEnvironment }.count }

    var body: some View {
        HStack(spacing: Space.xs) {
            Image(nsImage: Self.image)
            if shownCount > 0 {
                Text("\(shownCount)")
            }
        }
        .accessibilityLabel("Ratchet, \(shownCount) need you")
    }
}

/// Running optimizations, what needs you and the sandbox. Nothing here is only reachable from the menu bar: the
/// window has all of it.
struct MenuBarView: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.sm) {
                RatchetMarkView(height: 16)
                Text("Ratchet").kitType(.brand).foregroundStyle(Ink.text)
                Spacer()
                if !shownNeedsYou.isEmpty {
                    Text("\(shownNeedsYou.count) need you").kitType(.caption).foregroundStyle(Ink.wait)
                }
            }
            Hairline()
            if store.optimizationStore.runningOptimizations.isEmpty {
                Text("No optimizations running").kitType(.meta).foregroundStyle(Ink.textFaint)
            } else {
                ForEach(store.optimizationStore.runningOptimizations) { optimization in
                    Button { show { router.open(optimization: optimization.id) } } label: { runRow(optimization) }
                        .buttonStyle(.plain)
                }
            }
            if !shownNeedsYou.isEmpty {
                Hairline()
                ForEach(shownNeedsYou) { item in
                    Button { show { router.open(optimization: item.optimizationID) } } label: {
                        HStack(alignment: .top, spacing: Space.sm) {
                            StatusShape(tone: .wait, size: Dim.icon - Space.xs).frame(height: Dim.textLine)
                            TitleStack(title: item.title, subtitle: item.detail)
                            Spacer(minLength: 0)
                        }
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel("Needs you: \(item.title), \(item.detail)")
                    }
                    .buttonStyle(.plain)
                }
            }
            Hairline()
            // "Keeping your Mac awake" only while the daemon holds the assertion (a run whose agents work on this Mac).
            if let line = store.power.menuLine {
                HStack(alignment: .top, spacing: Space.xs) {
                    StatusShape(tone: .run, size: Dim.icon - Space.xs).frame(height: Dim.textLine)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(line).kitType(.caption).foregroundStyle(Ink.textMuted)
                        if let detail = store.power.menuDetail {
                            Text(detail).kitType(.caption).foregroundStyle(Ink.textFaint)
                        }
                    }
                }
                .accessibilityElement(children: .combine)
            }
            HStack(alignment: .top, spacing: Space.xs) {
                StatusShape(tone: store.sandboxStore.sandbox.isOn ? .ok : .off, size: Dim.icon - Space.xs).frame(height: Dim.textLine)
                Text(MenuBarRules.sandboxLine(store.sandboxStore.sandbox)).kitType(.caption).foregroundStyle(Ink.textMuted)
            }
            .accessibilityElement(children: .combine)
            HStack(spacing: Space.sm) {
                Button("Open Ratchet") { show {} }.buttonStyle(.kit(.quiet, small: true))
                Button("New optimization") { show { router.newOptimization() } }
                    .buttonStyle(.kit(.primary, small: true))
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }.buttonStyle(.kit(.link, small: true))
            }
        }
        .padding(Space.lg)
        .frame(width: Layout.popoverWidth)
        .gridArea("menubar", inset: Space.lg)
        .tint(Ink.brass)
    }

    /// Nothing waits for an environment approval any more: only what a person can really act on.
    private var shownNeedsYou: [NeedsYouItem] { store.needsYou.filter { $0.kind != .approveEnvironment } }

    private func runRow(_ optimization: Optimization) -> some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            HStack {
                Text(optimization.title).kitType(.bodyStrong).foregroundStyle(Ink.text)
                Spacer()
                if let change = optimization.change {
                    Text(Percent.signed(change)).kitType(.mono).foregroundStyle(Ink.ok)
                }
            }
            ThinBar(fraction: MenuBarRules.timeFraction(optimization))
            Text(MenuBarRules.runNote(optimization)).kitType(.caption).foregroundStyle(Ink.textFaint)
        }
        .contentShape(.rect)
    }

    /// Brings the window forward, then runs `action`.
    private func show(_ action: () -> Void) {
        LaunchContext.revealWindow { openWindow(id: "main") }
        action()
    }
}
