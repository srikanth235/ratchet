import AppKit
import RatchetKit
import SwiftUI

/// One connector in the list: its state, its actions, and the form it opens in place.
struct ConnectorRow: View {
    let connector: Connector
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ConnectorsPageState.self) private var connectorsState
    @State private var hovering = false
    @State private var confirmingRemove = false
    @State private var draftName = ""

    private var mode: ConnectorRowMode? { connectorsState.rowModes[connector.id] }

    private func setMode(_ new: ConnectorRowMode?) { connectorsState.rowModes[connector.id] = new }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.lg) {
                nameBlock.frame(maxWidth: .infinity, alignment: .leading)
                if connector.isDefault {
                    Text("Default").kitType(.meta).foregroundStyle(Ink.textFaint)
                } else if hovering && connector.state.isUsable {
                    Button("Make default") { Task { await store.connectorStore.setDefaultConnector(connector.id) } }
                        .buttonStyle(.kit(.link))
                }
                signInButton
                StatusLabel(tone: connector.state.tone, text: connector.state.title)
                    .frame(width: 120, alignment: .leading)
                actions
            }
            extras
        }
        .padding(.vertical, Dim.rowPadding)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .bottom) { Hairline() }
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .contextMenu { menuItems }
        .confirmationDialog("Remove \(connector.name)?", isPresented: $confirmingRemove) {
            Button("Remove", role: .destructive) { Task { await store.connectorStore.removeConnector(connector.id) } }
        } message: {
            Text(removeNote)
        }
    }

    private var removeNote: String {
        switch connector.kind {
        case .aws: "Optimizations stop using it. Its saved choice is forgotten; your AWS files are untouched."
        case .agent where connector.origin == .login: "Optimizations stop using it. The CLI itself stays signed in."
        default: "Optimizations stop using it. Its secrets are deleted from your keychain."
        }
    }

    // MARK: Name and detail

    @ViewBuilder
    private var nameBlock: some View {
        if mode == .rename {
            HStack(spacing: Space.sm) {
                TextField("Name", text: $draftName)
                    .kitField()
                    .frame(maxWidth: 280)
                    .onSubmit { saveName() }
                    .accessibilityLabel("Name of \(connector.name)")
                    .onAppear { draftName = connector.name }
                Button("Save") { saveName() }
                    .buttonStyle(.kit(.primary, small: true))
                    .disabled(draftName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || draftName.trimmingCharacters(in: .whitespacesAndNewlines) == connector.name)
                Button("Cancel") { setMode(nil) }
                    .buttonStyle(.kit(.link, small: true))
            }
        } else {
            VStack(alignment: .leading, spacing: 0) {
                Text(connector.name).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
                CommandText(text: connector.detail).lineLimit(1)
            }
            .layoutPriority(1)
        }
    }

    private func saveName() {
        let name = draftName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        Task {
            do {
                try await store.connectorStore.updateConnector(connector.id, ConnectorUpdate(name: name))
                setMode(nil)
            } catch {
                connectorsState.rowNotes[connector.id] = ConnectorNote(ok: false, text: error.localizedDescription)
            }
        }
    }

    // MARK: Buttons

    @ViewBuilder
    private var signInButton: some View {
        if connector.kind == .aws && connector.state == .expired {
            Button("Sign in") { setMode(mode == .ssoHint ? nil : .ssoHint) }
                .buttonStyle(.kit(.quiet, small: true))
        } else if connector.state == .expired || (connector.kind == .github && connector.state == .signedOut) {
            if !connector.isFromEnvironment {
                Button("Sign in") { Task { await store.connectorStore.signInConnector(connector.id) } }
                    .buttonStyle(.kit(.quiet, small: true))
            }
        }
    }

    private var actions: some View {
        Menu {
            menuItems
        } label: {
            MoreDots()
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .accessibilityLabel("Actions for \(connector.name)")
    }

    @ViewBuilder
    private var menuItems: some View {
        if connector.state.isUsable && !connector.isDefault {
            Button("Make Default") { Task { await store.connectorStore.setDefaultConnector(connector.id) } }
        }
        Button("Check Connection") { check() }
        if connector.state == .expired || connector.state == .signedOut {
            if connector.kind != .github || !connector.isFromEnvironment {
                Button("Sign In") { Task { await store.connectorStore.signInConnector(connector.id) } }
            }
        }
        if connector.kind == .aws {
            Button("Edit Region and Cap…") { setMode(.edit) }
        }
        if !connector.isFromEnvironment {
            Button("Rename…") { setMode(.rename) }
            Divider()
            Button("Remove…", role: .destructive) { confirmingRemove = true }
        }
    }

    private func check() {
        Task {
            do {
                let result = try await store.connectorStore.checkConnector(connector.id)
                connectorsState.rowNotes[connector.id] = ConnectorNote(ok: result.ok, text: result.detail)
            } catch {
                connectorsState.rowNotes[connector.id] = ConnectorNote(ok: false, text: error.localizedDescription)
            }
        }
    }

    // MARK: Under the row

    @ViewBuilder
    private var extras: some View {
        if mode == .ssoHint {
            HStack(spacing: Space.md) {
                CommandText(text: "Run `aws sso login --profile \(connector.profile ?? connector.name)` in a terminal, then check again.", type: .meta, color: Ink.textMuted)
                Button("Check again") { check() }
                    .buttonStyle(.kit(.quiet, small: true))
            }
        }
        if mode == .edit {
            AwsEditForm(connector: connector)
        }
        if let note = connectorsState.rowNotes[connector.id] {
            NoteLine(tone: note.ok ? .ok : .bad, text: note.text)
        }
    }
}
