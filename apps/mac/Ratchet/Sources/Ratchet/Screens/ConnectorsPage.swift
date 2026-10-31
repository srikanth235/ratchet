import AppKit
import RatchetKit
import SwiftUI

/// GitHub, AWS and agent connectors. Also the Connectors tab of Settings (`embedded`).
struct ConnectorsPage: View {
    var embedded = false
    @Environment(AppStore.self) private var store
    @State private var adding = false

    private var areaName: String { embedded ? "settings-connectors" : "connectors" }
    private var probePrefix: String { embedded ? "settings-" : "" }

    var body: some View {
        KitPage(maxWidth: 980) {
            PageHeader("Connectors", subtitle: "Accounts each optimization can use. Secrets stay in your keychain.") {
                Button("Add connector") { adding = true }
                    .buttonStyle(.kit(.primary))
            }
            .gridProbe(probePrefix + "header", area: areaName)
            if let flow = store.githubFlow {
                GithubFlowCard(flow: flow)
                    .gridProbe(probePrefix + "github-flow", area: areaName)
            }
            ForEach(ConnectorKind.allCases) { kind in
                KitSection(kind.title) {
                    HairlineList {
                        ForEach(store.connectors(of: kind)) { connector in
                            ConnectorRow(connector: connector)
                        }
                    }
                }
                .gridProbe(probePrefix + kind.rawValue, area: areaName)
            }
        }
        .gridArea(areaName)
        .sheet(isPresented: $adding) { AddConnectorSheet() }
    }
}

private struct ConnectorRow: View {
    let connector: Connector
    @Environment(AppStore.self) private var store
    @State private var hovering = false
    @State private var confirmingRemove = false

    var body: some View {
        HairlineRow {
            TitleStack(title: connector.name, subtitle: connector.detail)
                .frame(maxWidth: .infinity, alignment: .leading)
            if connector.isDefault {
                Text("Default").kitType(.meta).foregroundStyle(Ink.textFaint)
            } else if hovering && connector.state.isUsable {
                Button("Make default") { Task { await store.setDefaultConnector(connector.id) } }
                    .buttonStyle(.kit(.link))
            }
            if connector.state == .expired {
                Button("Sign in") { Task { await store.signInConnector(connector.id) } }
                    .buttonStyle(.kit(.quiet, small: true))
            }
            StatusLabel(tone: connector.state.tone, text: connector.state.title)
                .frame(width: 120, alignment: .leading)
        }
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .contextMenu {
            if connector.state.isUsable && !connector.isDefault {
                Button("Make Default") { Task { await store.setDefaultConnector(connector.id) } }
            }
            if connector.state == .expired || connector.state == .signedOut {
                Button("Sign In") { Task { await store.signInConnector(connector.id) } }
            }
            Divider()
            Button("Remove…", role: .destructive) { confirmingRemove = true }
        }
        .confirmationDialog("Remove \(connector.name)?", isPresented: $confirmingRemove) {
            Button("Remove", role: .destructive) { Task { await store.removeConnector(connector.id) } }
        } message: {
            Text("Optimizations stop using it. Its secrets are deleted from your keychain.")
        }
    }
}

/// The GitHub device flow: type this code at github.com/login/device. The account appears in the list when GitHub confirms.
private struct GithubFlowCard: View {
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
                        Button("Cancel") { Task { await store.cancelGithubSignIn() } }
                            .buttonStyle(.kit(.link, small: true))
                    }
                }
            } else {
                Button("Try again") { Task { await store.startGithubSignIn() } }
                    .buttonStyle(.kit(.primary, small: true))
                Button("Dismiss") { Task { await store.cancelGithubSignIn() } }
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

struct AddConnectorSheet: View {
    @Environment(AppStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var kind: ConnectorKind = .github
    @State private var name = ""
    @State private var detail = ""
    @State private var agent = "Claude Code"
    @State private var apiKey = ""

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            Text("Add connector").kitType(.pageTitle).foregroundStyle(Ink.text)
            Picker("Kind", selection: $kind) {
                ForEach(ConnectorKind.allCases) { kind in Text(kind.title).tag(kind) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            fields
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(.kit(.quiet))
                    .keyboardShortcut(.cancelAction)
                Button(primaryTitle) {
                    Task {
                        switch kind {
                        case .github:
                            await store.startGithubSignIn()
                        case .aws:
                            await store.addConnector(NewConnector(kind: .aws, name: name, detail: detail))
                        case .agent:
                            await store.addConnector(NewConnector(kind: .agent, name: agent, detail: detail, secret: apiKey))
                        }
                        apiKey = ""
                        dismiss()
                    }
                }
                .buttonStyle(.kit(.primary))
                .keyboardShortcut(.defaultAction)
                .disabled(kind == .aws && name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(Space.xxl)
        .frame(width: 480)
        .background(Ink.canvas)
    }

    @ViewBuilder
    private var fields: some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            switch kind {
            case .github:
                Text("Sign in with GitHub").kitType(.section).foregroundStyle(Ink.textMuted)
                Text("GitHub shows a short code. Type it at github.com/login/device and Ratchet adds the account. No password or token passes through the app.")
                    .kitType(.meta).foregroundStyle(Ink.textMuted)
            case .aws:
                Text("AWS profile").kitType(.section).foregroundStyle(Ink.textMuted)
                TextField("acme-prod", text: $name).kitField()
                Text("A profile from ~/.aws/config. Credentials stay where AWS keeps them.")
                    .kitType(.caption).foregroundStyle(Ink.textFaint)
            case .agent:
                Text("Agent").kitType(.section).foregroundStyle(Ink.textMuted)
                Picker("Agent", selection: $agent) {
                    Text("Claude Code").tag("Claude Code")
                    Text("Codex").tag("Codex")
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                Text("API key").kitType(.section).foregroundStyle(Ink.textMuted).padding(.top, Space.xs)
                SecureField("Optional: leave empty to use the CLI login on this Mac", text: $apiKey).kitField()
                Text("The key goes to ratchetd once and is filed in your keychain. The app never keeps it.")
                    .kitType(.caption).foregroundStyle(Ink.textFaint)
            }
        }
    }

    private var primaryTitle: String {
        switch kind {
        case .github: "Get a code"
        case .aws, .agent: "Add"
        }
    }
}
