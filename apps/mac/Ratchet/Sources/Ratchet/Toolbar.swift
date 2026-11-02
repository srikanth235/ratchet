import RatchetKit
import SwiftUI

// The window's toolbar: the sandbox status, a connection note, and New Optimization. Pages add their own items (the range, the
// filter, Add…) next to these and a page-scoped `.searchable`. Every item here also has a menu bar command.

/// The items every page shares, at the trailing edge of the detail column. Declared on the detail view, never on the split view.
struct ShellToolbar: ToolbarContent {
    /// Demo data, or a dropped connection: the badge is an item only then, so no empty capsule floats in the bar.
    let showsConnectionBadge: Bool

    var body: some ToolbarContent {
        ToolbarSpacer(.flexible)
        if showsConnectionBadge {
            ToolbarItem { ConnectionBadge() }
        }
        ToolbarItem { SandboxStatusItem() }
        ToolbarSpacer(.fixed)
        ToolbarItem(placement: .primaryAction) { NewOptimizationToolbarButton() }
    }
}

/// New Optimization (Command-N): the one action of the toolbar. Off while the background service is not answering.
struct NewOptimizationToolbarButton: View {
    @Environment(AppRouter.self) private var router
    @Environment(AppStore.self) private var store
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    var body: some View {
        Button {
            router.startWizard(client: store.client)
        } label: {
            Label("New Optimization", systemImage: "plus")
                .labelStyle(.titleAndIcon)
        }
        .disabled(!actionsEnabled)
        .help("Create an optimization (Command-N)")
    }
}

/// "Demo data" or "Reconnecting…": what the person must know about the numbers on screen. Nothing at all when the data is
/// real and live.
struct ConnectionBadge: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        if case .reconnecting = store.streamState {
            Text("Reconnecting…")
                .kitType(.caption).foregroundStyle(Ink.textMuted)
                .dimsWhenInactive()
                .help("The live connection to the background service dropped and is being retried. What you see may be a little old.")
        } else if store.isDemoData {
            Text("Demo data")
                .kitType(.caption).foregroundStyle(Ink.textMuted)
                .dimsWhenInactive()
                .help("Ratchet shows made-up data and uses no accounts. Turn demo mode off in Settings, This Mac.")
                .accessibilityLabel("Demo data, no accounts are used")
        }
    }
}

/// "Sandbox on · acme-prod · $1.20" as a toolbar status item: a shape and the word, never colour alone. Opens the popover with
/// End Now and the last receipt.
struct SandboxStatusItem: View {
    @Environment(AppStore.self) private var store
    @State private var showing = false

    private var sandbox: SandboxStatus { store.sandboxStore.sandbox }

    var body: some View {
        Button {
            showing.toggle()
        } label: {
            HStack(spacing: Space.xs) {
                StatusShape(tone: sandbox.isOn ? .ok : .off, size: Dim.icon - Space.xs)
                Text(headline).kitType(.caption)
            }
            .dimsWhenInactive()
        }
        .help(detail)
        .popover(isPresented: $showing, arrowEdge: .bottom) {
            SandboxPopover()
        }
        .accessibilityLabel("Sandbox status")
        .accessibilityValue(headline + ", " + detail)
        .accessibilityHint("Opens the sandbox details")
    }

    private var headline: String {
        guard sandbox.isOn else { return "Sandbox off" }
        return "Sandbox on · " + Money.exact(sandbox.costSoFarUSD)
    }

    private var detail: String {
        guard sandbox.isOn else { return "Nothing is running in the cloud. Click for the last teardown." }
        var line = (sandbox.account ?? "AWS") + " · " + Money.exact(sandbox.costSoFarUSD) + " so far"
        if let offAt = sandbox.offAt { line += " · off at " + Elapsed.timeOfDay(offAt) }
        return line
    }
}

/// The sandbox's details, End Now behind one confirmation, and the last teardown. No custom background: the system paints the
/// popover.
struct SandboxPopover: View {
    @Environment(AppStore.self) private var store
    @Environment(\.daemonActionsEnabled) private var actionsEnabled
    @State private var confirming = false

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            HStack(spacing: Space.sm) {
                StatusShape(tone: store.sandboxStore.sandbox.isOn ? .ok : .off, size: Dim.icon - Space.xs)
                Text(store.sandboxStore.sandbox.isOn ? "Sandbox on" : "Sandbox off").kitType(.bodyStrong).foregroundStyle(Ink.text)
            }
            if store.sandboxStore.sandbox.isOn {
                VStack(alignment: .leading, spacing: Space.xs) {
                    detailLine("Account", store.sandboxStore.sandbox.account ?? "AWS")
                    detailLine("Cost so far", Money.exact(store.sandboxStore.sandbox.costSoFarUSD))
                    if let offAt = store.sandboxStore.sandbox.offAt { detailLine("Switches off at", Elapsed.timeOfDay(offAt)) }
                }
                if confirming {
                    Text("Running agents stop and everything in the sandbox is torn down.")
                        .kitType(.meta).foregroundStyle(Ink.textMuted)
                    HStack(spacing: Space.sm) {
                        Button("Cancel") { confirming = false }.buttonStyle(.kit(.quiet, small: true))
                        Button("End Now") {
                            confirming = false
                            Task { await store.sandboxStore.endSandbox() }
                        }
                        .buttonStyle(.kit(.primary, small: true))
                    }
                } else {
                    Button("End Now…") { confirming = true }
                        .buttonStyle(.kit(.quiet, small: true))
                        .disabled(!actionsEnabled)
                }
            } else {
                Text("Nothing is running in your AWS account.").kitType(.meta).foregroundStyle(Ink.textMuted)
            }
            if let receipt = store.sandboxStore.receipt {
                Hairline()
                VStack(alignment: .leading, spacing: Space.sm) {
                    Text("Last teardown").kitType(.section).foregroundStyle(Ink.textMuted)
                    ReceiptView(receipt: receipt)
                }
            }
        }
        .padding(Space.lg)
        .frame(width: Layout.popoverWidth, alignment: .leading)
    }

    private func detailLine(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).kitType(.meta).foregroundStyle(Ink.textFaint)
            Spacer()
            Text(value).kitType(.mono).foregroundStyle(Ink.text)
        }
    }
}

/// What a teardown left behind: nothing. Shared by the popover and the result page.
struct ReceiptView: View {
    let receipt: TeardownReceipt

    var body: some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            Text(receipt.account + " · " + Elapsed.hm(receipt.duration) + " · " + Money.exact(receipt.costUSD))
                .kitType(.meta).foregroundStyle(Ink.textMuted)
            ForEach(receipt.destroyed, id: \.self) { item in
                HStack(spacing: Space.sm) {
                    KitIconView(icon: .check, size: Dim.icon - 4, weight: 1.8).foregroundStyle(Ink.ok)
                    Text(item + " destroyed").kitType(.meta).foregroundStyle(Ink.text)
                }
            }
            if receipt.allGone {
                Text("Nothing is left running or billing.").kitType(.caption).foregroundStyle(Ink.textFaint)
            }
        }
        .accessibilityElement(children: .combine)
    }
}
