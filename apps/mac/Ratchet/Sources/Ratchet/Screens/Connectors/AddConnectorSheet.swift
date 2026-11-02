import AppKit
import RatchetKit
import SwiftUI

/// The Add connector sheet: a kind picker over the fields of GitHub, AWS or an agent. What the person typed, what is wrong with
/// it and what pressing Add sends are `AddConnectorForm`'s (tested in RatchetKit); this view draws them and does the calls.
struct AddConnectorSheet: View {
    let request: ConnectorSheetRequest
    @Environment(AppStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var form: AddConnectorForm
    @State private var profiles: AwsProfiles?
    @State private var found: [DetectedHarness]?
    @State private var failure: String?
    @State private var busy = false

    init(request: ConnectorSheetRequest) {
        self.request = request
        var start = AddConnectorForm(kind: request.kind, showsToken: request.showsToken, awsProfile: request.pick)
        if let harness = request.harness { start.harness = harness }
        _form = State(initialValue: start)
    }

    private var canStore: Bool { store.connectorStore.connectorSetup.encryptionAvailable }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            Text("Add Connector").kitType(.pageTitle).foregroundStyle(Ink.text)
                .gridProbe("title", area: "add-connector")
            Picker("Kind", selection: $form.kind) {
                ForEach(ConnectorKind.allCases) { kind in Text(kind.title).tag(kind) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            fields
            if let failure {
                Text(failure).kitType(.meta).foregroundStyle(Ink.bad).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(.kit(.quiet))
                    .keyboardShortcut(.cancelAction)
                Button(form.primaryTitle) { submit() }
                    .buttonStyle(.kit(.primary))
                    .keyboardShortcut(.defaultAction)
                    .disabled(!form.canSubmit(canStore: canStore) || busy)
            }
        }
        .padding(Space.xxl)
        .frame(width: Layout.sheetWidth)
        .gridArea("add-connector", inset: Space.xxl)
        .onChange(of: form.kind) { failure = nil }
    }

    @ViewBuilder
    private var fields: some View {
        switch form.kind {
        case .github: githubFields
        case .aws: awsFields
        case .agent: agentFields
        }
    }

    private func submit() {
        guard let submission = form.submission(canStore: canStore) else { return }
        busy = true
        failure = nil
        Task {
            defer { busy = false }
            do {
                switch submission {
                case .githubDeviceFlow:
                    await store.connectorStore.startGithubSignIn()
                case .githubToken(let token):
                    try await store.connectorStore.addGithubToken(token)
                case .connector(let new):
                    try await store.connectorStore.addConnectorChecked(new)
                    Task { await store.connectorStore.refreshConnectors() }
                }
                form.clearSecrets()
                dismiss()
            } catch {
                failure = error.localizedDescription
            }
        }
    }

    // MARK: GitHub

    private var githubFields: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            Text("Sign in with GitHub").kitType(.section).foregroundStyle(Ink.textMuted)
            Text("GitHub shows a short code. Type it at github.com/login/device and Ratchet adds the account. No password or token passes through the app.")
                .kitType(.meta).foregroundStyle(Ink.textMuted).fixedSize(horizontal: false, vertical: true)
            if !store.connectorStore.connectorSetup.clientIdConfigured {
                CommandText(text: "This build uses the built-in placeholder OAuth client id: set `RATCHET_GITHUB_CLIENT_ID` to your OAuth app.", type: .caption)
            }
            DisclosureGroup(isExpanded: $form.showsToken) {
                VStack(alignment: .leading, spacing: Space.sm) {
                    if !canStore { keychainNotice("a token") }
                    FieldBlock("Token", problem: form.tokenProblem) {
                        SecureField("ghp_… or github_pat_…", text: $form.token)
                            .kitField()
                            .disabled(!canStore)
                            .accessibilityLabel("GitHub token")
                    }
                    Text("For an organization, use a fine-grained token or a GitHub App installation token with repository access. It is checked, then kept in your keychain.")
                        .kitType(.caption).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
                }
                .padding(.top, Space.sm)
            } label: {
                Text("Use an access token instead").kitType(.meta).foregroundStyle(Ink.textMuted)
            }
        }
    }

    private func keychainNotice(_ what: String) -> some View {
        NoteLine(tone: .wait, text: "The keychain is unavailable, so \(what) cannot be stored. The environment's own variables are still honoured.")
    }

    // MARK: AWS

    private var awsFields: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            Text("AWS profile").kitType(.section).foregroundStyle(Ink.textMuted)
            if let profiles {
                let available = profiles.available(excluding: store.connectorStore.connectors)
                if let note = profiles.emptyNote(excluding: store.connectorStore.connectors) {
                    CommandText(text: note + " Run `aws configure sso` in a terminal, then rescan.", type: .meta, color: Ink.textMuted)
                } else {
                    ScrollView {
                        VStack(spacing: Dim.cardGap) {
                            ForEach(available) { profile in
                                ChoiceCard(mark: .radio, isSelected: form.awsProfile == profile.name) {
                                    form.awsProfile = profile.name
                                } content: {
                                    Text(profile.name).kitType(.monoStrong).foregroundStyle(Ink.text)
                                    Text(profile.hint).kitType(.meta).foregroundStyle(Ink.textFaint)
                                }
                            }
                        }
                    }
                    .frame(maxHeight: 200)
                }
                if !profiles.ssoSessions.isEmpty {
                    CommandText(text: "SSO sessions found: `" + profiles.ssoSessions.joined(separator: ", ") + "`. Pick a profile that uses one.", type: .caption)
                }
            } else {
                Text("Looking for profiles…").kitType(.meta).foregroundStyle(Ink.textFaint)
            }
            HStack(alignment: .top, spacing: Space.lg) {
                FieldBlock("Region", problem: form.regionProblem) {
                    TextField("us-east-1", text: $form.region).kitField().accessibilityLabel("AWS region")
                }
                FieldBlock("Monthly cap (USD), optional", problem: form.capProblem) {
                    TextField("none", text: $form.cap).kitField().accessibilityLabel("Monthly cap in dollars")
                }
            }
            HStack {
                Text("Only the profile name is stored; credentials stay where AWS keeps them. A repo can override the region in ratchet.yaml.")
                    .kitType(.caption).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: Space.md)
                Button("Rescan") { Task { profiles = try? await store.client.awsProfiles() } }
                    .buttonStyle(.kit(.link))
            }
        }
        .task { profiles = try? await store.client.awsProfiles() }
    }

    // MARK: Agents

    private var agentFields: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            VStack(alignment: .leading, spacing: Space.sm) {
                Text("Found on this Mac").kitType(.section).foregroundStyle(Ink.textMuted)
                if let found {
                    let addable = DetectedHarness.addable(found, connected: store.connectorStore.connectors)
                    if addable.isEmpty {
                        Text(DetectedHarness.emptyNote(found)).kitType(.meta).foregroundStyle(Ink.textFaint)
                    } else {
                        ForEach(addable) { agent in
                            HStack {
                                Text(agent.title).kitType(.body).foregroundStyle(Ink.text)
                                Spacer()
                                Button("Add your login") { addLogin(agent) }
                                    .buttonStyle(.kit(.quiet, small: true))
                                    .disabled(busy)
                            }
                        }
                    }
                } else {
                    Text("Looking…").kitType(.meta).foregroundStyle(Ink.textFaint)
                }
                Button("Re-detect") { redetect() }
                    .buttonStyle(.kit(.link))
            }
            VStack(alignment: .leading, spacing: Space.sm) {
                Text("Or paste an API key").kitType(.section).foregroundStyle(Ink.textMuted)
                Text("A key can also run in the cloud, with your laptop closed. It goes to ratchetd once and is kept in your keychain; the app never keeps it.")
                    .kitType(.caption).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
                if !canStore { keychainNotice("an API key") }
                Picker("Agent", selection: $form.harness) {
                    ForEach(Harness.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                FieldBlock("\(form.harness.vendor) API key", problem: form.keyProblem) {
                    SecureField("", text: $form.apiKey).kitField().disabled(!canStore).accessibilityLabel("API key")
                }
                FieldBlock("Name, optional") {
                    TextField(form.harness.title + " · team key", text: $form.keyName).kitField().accessibilityLabel("Name")
                }
            }
        }
        .task { found = try? await store.client.detectedHarnesses(redetect: false) }
    }

    private func addLogin(_ agent: DetectedHarness) {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await store.connectorStore.addConnectorChecked(NewConnector(kind: .agent, name: agent.harness.title))
                dismiss()
            } catch {
                failure = error.localizedDescription
            }
        }
    }

    private func redetect() {
        Task {
            do {
                found = try await store.client.detectedHarnesses(redetect: true)
                await store.connectorStore.refreshConnectors()
                failure = nil
            } catch {
                failure = error.localizedDescription
            }
        }
    }
}
