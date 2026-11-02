import AppKit
import RatchetKit
import SwiftUI

/// One connector in the list: its state, its actions, and the form it opens in place.
struct ConnectorRow: View {
    let connector: Connector
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ConnectorsPageState.self) private var connectorsState
    @Environment(\.daemonActionsEnabled) private var actionsEnabled
    @Environment(\.undoManager) private var undoManager
    @State private var confirmingRemove = false
    @State private var draftName = ""

    private var mode: ConnectorRowMode? { connectorsState.rowModes[connector.id] }

    private func setMode(_ new: ConnectorRowMode?) { connectorsState.rowModes[connector.id] = new }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.lg) {
                nameBlock.frame(maxWidth: .infinity, alignment: .leading)
                // "Make Default" is in the actions menu (and the context menu): nothing here appears only on hover.
                if connector.isDefault {
                    Text("Default").kitType(.meta).foregroundStyle(Ink.textFaint)
                }
                // A broken status always has its fix beside it.
                fixButton
                StatusLabel(tone: connector.state.tone, text: connector.state.title)
                    .frame(width: Layout.statusColumn, alignment: .leading)
                actions
            }
            extras
        }
        .padding(.vertical, Dim.rowPadding)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .bottom) { Hairline() }
        .contentShape(.rect)
        .contextMenu { menuItems }
        .confirmationDialog("Remove \(connector.name)?", isPresented: $confirmingRemove) {
            Button("Remove", role: .destructive) { remove() }
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
                    .frame(maxWidth: Layout.fieldWide)
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

    /// What fixes a connector that is not usable: sign in again, add a key, or look for the login again.
    private enum Fix { case signIn, addKey, redetect }

    private var fix: Fix? {
        guard !connector.state.isUsable else { return nil }
        switch connector.kind {
        case .github: return connector.isFromEnvironment ? nil : .signIn
        case .aws: return .signIn
        case .agent: return connector.origin == .apiKey ? .addKey : .redetect
        }
    }

    @ViewBuilder
    private var fixButton: some View {
        switch fix {
        case .signIn:
            Button("Sign in") { signIn() }
                .buttonStyle(.kit(.quiet, small: true))
                .disabled(!actionsEnabled)
                .accessibilityLabel("Sign in to \(connector.name)")
        case .addKey:
            Button("Add key") { connectorsState.sheet = ConnectorSheetRequest(kind: .agent) }
                .buttonStyle(.kit(.quiet, small: true))
                .disabled(!actionsEnabled)
                .accessibilityLabel("Add a key for \(connector.name)")
        case .redetect:
            Button("Re-detect") { Task { await store.connectorStore.refreshConnectors() } }
                .buttonStyle(.kit(.quiet, small: true))
                .disabled(!actionsEnabled)
                .accessibilityLabel("Look for the \(connector.name) login again")
        case nil:
            EmptyView()
        }
    }

    /// AWS shows the sign-in steps; GitHub runs the device flow.
    private func signIn() {
        if connector.kind == .aws {
            setMode(mode == .ssoHint ? nil : .ssoHint)
        } else {
            Task { await store.connectorStore.signInConnector(connector.id) }
        }
    }

    /// One line under a connector that is not usable, saying what is wrong and what to do.
    private var brokenNote: String? {
        guard !connector.state.isUsable else { return nil }
        switch connector.kind {
        case .github: return "GitHub access is \(connector.state.title.lowercased()). Sign in again to open pull requests."
        case .aws: return "The AWS session is \(connector.state.title.lowercased()). Sign in again before a long run."
        case .agent:
            return connector.origin == .apiKey
                ? "The key is \(connector.state.title.lowercased()). Add a new key."
                : "Not signed in. Sign in with its own command line tool in a terminal, then Re-detect."
        }
    }

    private var actions: some View {
        KitMoreMenu("Actions for \(connector.name)") {
            menuItems
        }
    }

    /// "Make Default" can be taken back: Undo in the Edit menu makes the previous one the default again.
    private func makeDefault() {
        let previous = store.connectorStore.connectors(of: connector.kind).first { $0.isDefault }?.id
        Task {
            await store.connectorStore.setDefaultConnector(connector.id)
            guard let previous, previous != connector.id else { return }
            undoManager?.registerUndo(withTarget: store.connectorStore) { target in
                Task { await target.setDefaultConnector(previous) }
            }
            undoManager?.setActionName("Make Default")
        }
    }

    /// An AWS profile is only a name, a region and a cap: removing it can be undone. Anything with a secret asks first.
    private func remove() {
        let removed = connector
        Task {
            await store.connectorStore.removeConnector(removed.id)
            guard removed.kind == .aws, store.lastError == nil else { return }
            let again = NewConnector(
                kind: .aws, name: removed.name, detail: removed.detail, region: removed.region, monthlyCapUSD: removed.monthlyCapUSD)
            undoManager?.registerUndo(withTarget: store.connectorStore) { target in
                Task { await target.addConnector(again) }
            }
            undoManager?.setActionName("Remove Connector")
        }
    }

    @ViewBuilder
    private var menuItems: some View {
        if connector.state.isUsable && !connector.isDefault {
            Button("Make Default") { makeDefault() }
                .disabled(!actionsEnabled)
        }
        Button("Check Connection") { check() }
            .disabled(!actionsEnabled)
        if connector.state == .expired || connector.state == .signedOut {
            if connector.kind != .github || !connector.isFromEnvironment {
                Button("Sign In") { Task { await store.connectorStore.signInConnector(connector.id) } }
                    .disabled(!actionsEnabled)
            }
        }
        if connector.kind == .aws {
            Button("Edit Region and Cap…") { setMode(.edit) }
        }
        if !connector.isFromEnvironment {
            Button("Rename…") { setMode(.rename) }
            Divider()
            if connector.kind == .aws {
                Button("Remove", role: .destructive) { remove() }
                    .disabled(!actionsEnabled)
            } else {
                Button("Remove…", role: .destructive) { confirmingRemove = true }
                    .disabled(!actionsEnabled)
            }
        }
    }

    private var ssoCommand: String { "aws sso login --profile \(connector.profile ?? connector.name)" }

    private func copySSOCommand() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(ssoCommand, forType: .string)
        connectorsState.rowNotes[connector.id] = ConnectorNote(ok: true, text: "Copied. Paste it in a terminal and finish in the browser, then check again.")
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
            // The sign-in is the AWS CLI's: it opens the browser for the SSO page. The row hands over the exact command and checks
            // the session again afterwards.
            HStack(spacing: Space.md) {
                CommandText(text: "Run `\(ssoCommand)` in a terminal, then check again.", type: .meta, color: Ink.textMuted)
                Button("Copy Command") { copySSOCommand() }
                    .buttonStyle(.kit(.quiet, small: true))
                    .help("Copy the sign-in command to paste in a terminal")
                Button("Check again") { check() }
                    .buttonStyle(.kit(.quiet, small: true))
            }
        }
        if mode == .edit {
            AwsEditForm(connector: connector)
        }
        if let note = connectorsState.rowNotes[connector.id] {
            NoteLine(tone: note.ok ? .ok : .bad, text: note.text)
        } else if let broken = brokenNote {
            NoteLine(tone: .bad, text: broken)
        }
    }
}
