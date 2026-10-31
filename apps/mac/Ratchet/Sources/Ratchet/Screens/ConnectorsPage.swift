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
                Text("Default").kitType(.meta).foregroundStyle(Ink.brassText)
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

struct AddConnectorSheet: View {
    @Environment(AppStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var kind: ConnectorKind = .github
    @State private var name = ""
    @State private var detail = ""

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            Text("Add connector").kitType(.pageTitle).foregroundStyle(Ink.text)
            Picker("Kind", selection: $kind) {
                ForEach(ConnectorKind.allCases) { kind in Text(kind.title).tag(kind) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            VStack(alignment: .leading, spacing: Space.sm) {
                Text(nameLabel).kitType(.section).foregroundStyle(Ink.textMuted)
                TextField(namePlaceholder, text: $name).kitField()
                Text("Note").kitType(.section).foregroundStyle(Ink.textMuted).padding(.top, Space.xs)
                TextField("Optional: org, region, key hint", text: $detail).kitField()
                Text("Secrets are stored in your keychain by ratchetd. The app never holds them.")
                    .kitType(.caption).foregroundStyle(Ink.textFaint)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(.kit(.quiet))
                    .keyboardShortcut(.cancelAction)
                Button("Add") {
                    Task {
                        await store.addConnector(NewConnector(kind: kind, name: name, detail: detail))
                        dismiss()
                    }
                }
                .buttonStyle(.kit(.primary))
                .keyboardShortcut(.defaultAction)
                .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(Space.xxl)
        .frame(width: 480)
        .background(Ink.canvas)
    }

    private var nameLabel: String {
        switch kind {
        case .github: "GitHub account or org"
        case .aws: "AWS profile"
        case .agent: "Agent"
        }
    }

    private var namePlaceholder: String {
        switch kind {
        case .github: "acme-org"
        case .aws: "acme-prod"
        case .agent: "Claude Code · team key"
        }
    }
}
