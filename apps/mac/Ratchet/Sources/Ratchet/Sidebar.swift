import RatchetKit
import SwiftUI

struct SidebarView: View {
    @Environment(AppRouter.self) private var router
    @Environment(AppStore.self) private var store

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xxl) {
            HStack(spacing: Space.sm) {
                KitIconView(icon: .optimizations, size: 22, weight: 1.7)
                    .foregroundStyle(Ink.brass)
                Text("Ratchet").kitType(.brand).foregroundStyle(Ink.text)
            }
            .padding(.horizontal, Space.md)
            .padding(.top, Space.sm)
            .accessibilityElement(children: .combine)

            VStack(spacing: Space.xs / 2) {
                ForEach(Page.allCases) { page in
                    SidebarRow(title: page.title, icon: page.icon, isSelected: router.page == page && router.wizard == nil
                               || (page == .optimizations && router.wizard != nil)) {
                        router.show(page)
                    }
                    .help("\(page.title) (Command-\(page.shortcutKey.character))")
                }
            }

            Spacer(minLength: 0)
            VStack(alignment: .leading, spacing: Space.xs) {
                if store.isDemoData {
                    Text("Demo data")
                        .kitType(.caption).foregroundStyle(Ink.textFaint)
                        .padding(.horizontal, Space.md)
                        .help("No ratchetd is running, so Ratchet shows sample data.")
                        .accessibilityLabel("Demo data, no daemon is running")
                }
                SandboxStatusLine()
            }
        }
        .padding(.horizontal, Space.sm)
        .padding(.top, Space.sm)
        .padding(.bottom, Space.lg)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(Ink.canvas)
    }
}

/// "Sandbox on · acme-prod / $1.20 so far · off at 15:40". Opens the popover with End now and the last receipt.
struct SandboxStatusLine: View {
    @Environment(AppStore.self) private var store
    @State private var showing = false

    var body: some View {
        Button {
            showing.toggle()
        } label: {
            HStack(alignment: .top, spacing: Space.xs) {
                StatusDot(tone: store.sandbox.isOn ? .ok : .off).frame(height: Dim.textLine)
                VStack(alignment: .leading, spacing: 0) {
                    Text(headline).kitType(.caption).foregroundStyle(Ink.textMuted)
                    Text(detail).kitType(.caption).foregroundStyle(Ink.textFaint)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Space.md)
            .padding(.vertical, Space.sm)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .help("Sandbox status, End now, last receipt")
        .popover(isPresented: $showing, arrowEdge: .trailing) {
            SandboxPopover()
        }
        .accessibilityLabel("Sandbox status")
        .accessibilityValue(headline + ", " + detail)
    }

    private var headline: String {
        let sandbox = store.sandbox
        guard sandbox.isOn else { return "Sandbox off" }
        return "Sandbox on · " + (sandbox.account ?? "AWS")
    }

    private var detail: String {
        let sandbox = store.sandbox
        guard sandbox.isOn else { return "Nothing running in the cloud" }
        var line = Money.exact(sandbox.costSoFarUSD) + " so far"
        if let offAt = sandbox.offAt { line += " · off at " + Elapsed.timeOfDay(offAt) }
        return line
    }
}

struct SandboxPopover: View {
    @Environment(AppStore.self) private var store
    @State private var confirming = false

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            HStack(spacing: Space.sm) {
                StatusDot(tone: store.sandbox.isOn ? .ok : .off)
                Text(store.sandbox.isOn ? "Sandbox on" : "Sandbox off").kitType(.bodyStrong).foregroundStyle(Ink.text)
            }
            if store.sandbox.isOn {
                VStack(alignment: .leading, spacing: Space.xs) {
                    detailLine("Account", store.sandbox.account ?? "AWS")
                    detailLine("Cost so far", Money.exact(store.sandbox.costSoFarUSD))
                    if let offAt = store.sandbox.offAt { detailLine("Switches off at", Elapsed.timeOfDay(offAt)) }
                }
                if confirming {
                    Text("Running agents stop and everything in the sandbox is torn down.")
                        .kitType(.meta).foregroundStyle(Ink.textMuted)
                    HStack(spacing: Space.sm) {
                        Button("Cancel") { confirming = false }.buttonStyle(.kit(.quiet, small: true))
                        Button("End now") {
                            confirming = false
                            Task { await store.endSandbox() }
                        }
                        .buttonStyle(.kit(.primary, small: true))
                    }
                } else {
                    Button("End now") { confirming = true }
                        .buttonStyle(.kit(.quiet, small: true))
                }
            } else {
                Text("Nothing is running in your AWS account.").kitType(.meta).foregroundStyle(Ink.textMuted)
            }
            if let receipt = store.receipt {
                Hairline()
                VStack(alignment: .leading, spacing: Space.sm) {
                    Text("Last teardown").kitType(.section).foregroundStyle(Ink.textMuted)
                    ReceiptView(receipt: receipt)
                }
            }
        }
        .padding(Space.lg)
        .frame(width: 320, alignment: .leading)
        .background(Ink.canvas)
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
