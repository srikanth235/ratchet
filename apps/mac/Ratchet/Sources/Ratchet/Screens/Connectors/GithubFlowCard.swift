import AppKit
import RatchetKit
import SwiftUI

/// The GitHub device flow: type this code at github.com/login/device. The account appears in the list when GitHub confirms.
struct GithubFlowCard: View {
    let flow: GithubSignInFlow
    @Environment(AppStore.self) private var store
    @Environment(\.openURL) private var openURL

    var body: some View {
        HStack(alignment: .center, spacing: Space.xxl) {
            VStack(alignment: .leading, spacing: Space.xs) {
                Text(title).kitType(.bodyStrong).foregroundStyle(Ink.text)
                Text(note).kitType(.meta).foregroundStyle(Ink.textMuted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if flow.state == .pending {
                Text(flow.userCode)
                    .kitType(.hero).foregroundStyle(Ink.brassText)
                    .textSelection(.enabled)
                    .accessibilityLabel("Your code is " + flow.userCode)
                VStack(alignment: .trailing, spacing: Space.sm) {
                    Button("Open GitHub") { if let url = URL(string: flow.verificationURL) { openURL(url) } }
                        .buttonStyle(.kit(.primary, small: true))
                    HStack(spacing: Space.sm) {
                        Button("Copy code") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(flow.userCode, forType: .string)
                        }
                        .buttonStyle(.kit(.quiet, small: true))
                        Button("Cancel") { Task { await store.connectorStore.cancelGithubSignIn() } }
                            .buttonStyle(.kit(.link, small: true))
                    }
                }
            } else {
                Button("Try again") { Task { await store.connectorStore.startGithubSignIn() } }
                    .buttonStyle(.kit(.primary, small: true))
                Button("Dismiss") { Task { await store.connectorStore.cancelGithubSignIn() } }
                    .buttonStyle(.kit(.link, small: true))
            }
        }
        .padding(Space.lg)
        .background(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).fill(Ink.raised.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).strokeBorder(Ink.hairlineStrong, lineWidth: Dim.hairline))
        .accessibilityElement(children: .contain)
    }

    private var title: String {
        switch flow.state {
        case .pending: "Finish signing in to GitHub"
        case .expired: "That code expired"
        case .denied: "GitHub sign-in was declined"
        case .error: "GitHub sign-in did not finish"
        }
    }

    private var note: String {
        if flow.state == .pending {
            return "Open github.com/login/device and type this code. Ratchet picks the account up when you approve it."
        }
        return flow.message ?? "Start again to get a new code."
    }
}
