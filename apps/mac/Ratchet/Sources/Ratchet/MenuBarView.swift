import AppKit
import RatchetKit
import SwiftUI

/// The menu bar icon, with the number of things that need you next to it.
struct MenuBarLabel: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        HStack(spacing: Space.xs) {
            Image(systemName: "chart.line.uptrend.xyaxis")
            if store.needsYouCount > 0 {
                Text("\(store.needsYouCount)")
            }
        }
        .accessibilityLabel("Ratchet, \(store.needsYouCount) need you")
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
            HStack {
                Text("Ratchet").kitType(.brand).foregroundStyle(Ink.text)
                Spacer()
                if store.needsYouCount > 0 {
                    Text("\(store.needsYouCount) need you").kitType(.caption).foregroundStyle(Ink.wait)
                }
            }
            Hairline()
            if store.runningOptimizations.isEmpty {
                Text("No optimizations running").kitType(.meta).foregroundStyle(Ink.textFaint)
            } else {
                ForEach(store.runningOptimizations) { optimization in
                    Button { show { router.open(optimization: optimization.id) } } label: { runRow(optimization) }
                        .buttonStyle(.plain)
                }
            }
            if store.needsYouCount > 0 {
                Hairline()
                ForEach(store.needsYou) { item in
                    Button { show { router.open(optimization: item.optimizationID) } } label: {
                        HStack(alignment: .top, spacing: Space.sm) {
                            StatusDot(tone: .wait)
                            TitleStack(title: item.title, subtitle: item.detail)
                            Spacer(minLength: 0)
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            Hairline()
            HStack(alignment: .top, spacing: Space.xs) {
                StatusDot(tone: store.sandbox.isOn ? .ok : .off)
                Text(sandboxLine).kitType(.caption).foregroundStyle(Ink.textMuted)
            }
            HStack(spacing: Space.sm) {
                Button("Open Ratchet") { show {} }.buttonStyle(.kit(.quiet, small: true))
                Button("New optimization") { show { router.startWizard(client: store.client) } }
                    .buttonStyle(.kit(.primary, small: true))
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }.buttonStyle(.kit(.link, small: true))
            }
        }
        .padding(Space.lg)
        .frame(width: 320)
        .background(Ink.canvas)
    }

    private var sandboxLine: String {
        guard store.sandbox.isOn else { return "Sandbox off" }
        var line = "Sandbox on · " + (store.sandbox.account ?? "AWS") + " · " + Money.exact(store.sandbox.costSoFarUSD) + " so far"
        if let offAt = store.sandbox.offAt { line += " · off at " + Elapsed.timeOfDay(offAt) }
        return line
    }

    private func runRow(_ optimization: Optimization) -> some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            HStack {
                Text(optimization.title).kitType(.bodyStrong).foregroundStyle(Ink.text)
                Spacer()
                if let change = optimization.change {
                    Text(Percent.signed(change)).kitType(.mono).foregroundStyle(Ink.ok)
                }
            }
            ThinBar(fraction: timeFraction(optimization))
            Text(runNote(optimization)).kitType(.caption).foregroundStyle(Ink.textFaint)
        }
        .contentShape(.rect)
    }

    private func timeFraction(_ optimization: Optimization) -> Double {
        let total = optimization.timeLimitHours * 3600
        guard total > 0, let left = optimization.secondsLeft else { return 0 }
        return 1 - left / total
    }

    private func runNote(_ optimization: Optimization) -> String {
        var parts: [String] = []
        if let left = optimization.secondsLeft { parts.append(Elapsed.hm(left) + " left") }
        parts.append(Money.short(optimization.spentUSD) + " spent")
        return parts.joined(separator: " · ")
    }

    /// Brings the window forward, then runs `action`.
    private func show(_ action: () -> Void) {
        openWindow(id: "main")
        NSApp.activate()
        action()
    }
}
