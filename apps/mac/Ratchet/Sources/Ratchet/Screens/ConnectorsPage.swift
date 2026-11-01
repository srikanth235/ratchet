import AppKit
import RatchetKit
import SwiftUI

/// GitHub, AWS and agent connectors. Also the Connectors tab of Settings (`embedded`).
struct ConnectorsPage: View {
    var embedded = false
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @State private var sheet: ConnectorSheetRequest?
    @State private var refreshing = false

    private var areaName: String { embedded ? "settings-connectors" : "connectors" }
    private var probePrefix: String { embedded ? "settings-" : "" }

    var body: some View {
        KitPage(maxWidth: 980) {
            PageHeader("Connectors", subtitle: "Accounts each optimization can use. Secrets stay in your keychain.") {
                HStack(spacing: Space.md) {
                    Button(refreshing ? "Checking…" : "Refresh all") {
                        Task {
                            refreshing = true
                            await store.refreshConnectors()
                            refreshing = false
                        }
                    }
                    .buttonStyle(.kit(.quiet))
                    .disabled(refreshing)
                    .help("Check every connector with its provider")
                    Button("Add connector") { sheet = ConnectorSheetRequest(kind: .github) }
                        .buttonStyle(.kit(.primary))
                }
            }
            .gridProbe(probePrefix + "header", area: areaName)
            if !store.connectorSetup.encryptionAvailable {
                NoteLine(
                    tone: .wait,
                    text: "The keychain is unavailable, so tokens and API keys cannot be stored. `GITHUB_TOKEN`, `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` from the environment are still honoured.")
                    .setupPanel()
                    .gridProbe(probePrefix + "keychain", area: areaName)
            }
            if let flow = store.githubFlow {
                GithubFlowCard(flow: flow)
                    .gridProbe(probePrefix + "github-flow", area: areaName)
            }
            ForEach(ConnectorKind.allCases) { kind in
                KitSection(kind.title) {
                    let list = store.connectors(of: kind)
                    if list.isEmpty {
                        Text(Self.emptyText(kind)).kitType(.meta).foregroundStyle(Ink.textFaint)
                    } else {
                        HairlineList {
                            ForEach(list) { connector in
                                ConnectorRow(connector: connector)
                            }
                        }
                    }
                }
                .gridProbe(probePrefix + kind.rawValue, area: areaName)
            }
        }
        .gridArea(areaName)
        .sheet(item: $sheet) { AddConnectorSheet(request: $0) }
        .onChange(of: router.connectorSheet) { _, request in
            guard !embedded, let request else { return }
            sheet = request
            router.connectorSheet = nil
        }
    }

    private static func emptyText(_ kind: ConnectorKind) -> String {
        switch kind {
        case .github: "No GitHub account yet. Ratchet pushes candidate branches and opens the pull request as you."
        case .aws: "No AWS profile yet. The evaluator runs in a sandbox in your AWS account, removed after each run."
        case .agent: "No agent yet. Install Claude Code, Codex or Grok on this Mac, or add an API key."
        }
    }
}

// MARK: - One connector

private struct ConnectorRow: View {
    let connector: Connector
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @State private var hovering = false
    @State private var confirmingRemove = false
    @State private var draftName = ""

    private var mode: ConnectorRowMode? { router.rowModes[connector.id] }

    private func setMode(_ new: ConnectorRowMode?) { router.rowModes[connector.id] = new }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.lg) {
                nameBlock.frame(maxWidth: .infinity, alignment: .leading)
                if connector.isDefault {
                    Text("Default").kitType(.meta).foregroundStyle(Ink.textFaint)
                } else if hovering && connector.state.isUsable {
                    Button("Make default") { Task { await store.setDefaultConnector(connector.id) } }
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
            Button("Remove", role: .destructive) { Task { await store.removeConnector(connector.id) } }
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
                try await store.updateConnector(connector.id, ConnectorUpdate(name: name))
                setMode(nil)
            } catch {
                router.rowNotes[connector.id] = ConnectorNote(ok: false, text: error.localizedDescription)
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
                Button("Sign in") { Task { await store.signInConnector(connector.id) } }
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
            Button("Make Default") { Task { await store.setDefaultConnector(connector.id) } }
        }
        Button("Check Connection") { check() }
        if connector.state == .expired || connector.state == .signedOut {
            if connector.kind != .github || !connector.isFromEnvironment {
                Button("Sign In") { Task { await store.signInConnector(connector.id) } }
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
                let result = try await store.checkConnector(connector.id)
                router.rowNotes[connector.id] = ConnectorNote(ok: result.ok, text: result.detail)
            } catch {
                router.rowNotes[connector.id] = ConnectorNote(ok: false, text: error.localizedDescription)
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
        if let note = router.rowNotes[connector.id] {
            NoteLine(tone: note.ok ? .ok : .bad, text: note.text)
        }
    }
}

/// Three dots: the row's actions.
private struct MoreDots: View {
    var body: some View {
        HStack(spacing: Space.xs) {
            ForEach(0..<3, id: \.self) { _ in
                Circle().fill(Ink.textFaint).frame(width: Space.xs, height: Space.xs)
            }
        }
        .padding(.horizontal, Space.sm)
        .padding(.vertical, Space.sm)
        .contentShape(.rect)
    }
}

/// An AWS connector's region and monthly cap, edited where it stands.
private struct AwsEditForm: View {
    let connector: Connector
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @State private var region: String
    @State private var cap: String
    @State private var failure: String?

    init(connector: Connector) {
        self.connector = connector
        _region = State(initialValue: connector.region ?? "us-east-1")
        _cap = State(initialValue: connector.monthlyCapUSD.map { String(format: "%g", $0) } ?? "")
    }

    private var regionProblem: String? { SettingsCheck.regionProblem(region) }
    private var capCheck: (value: Double?, problem: String?) { SettingsCheck.cap(cap) }
    private var canSave: Bool {
        regionProblem == nil && capCheck.problem == nil && !region.trimmingCharacters(in: .whitespaces).isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(alignment: .top, spacing: Space.lg) {
                FieldBlock("Region", problem: regionProblem) {
                    TextField("us-east-1", text: $region).kitField().frame(width: 200)
                        .accessibilityLabel("Region")
                }
                FieldBlock("Monthly cap (USD)", hint: "Empty: no cap", problem: capCheck.problem) {
                    TextField("none", text: $cap).kitField().frame(width: 160)
                        .accessibilityLabel("Monthly cap in dollars")
                }
                VStack {
                    Spacer(minLength: 0)
                    HStack(spacing: Space.md) {
                        Button("Save") { save() }
                            .buttonStyle(.kit(.primary, small: true))
                            .disabled(!canSave)
                        Button("Cancel") { router.rowModes[connector.id] = nil }
                            .buttonStyle(.kit(.link, small: true))
                    }
                    .frame(height: Dim.fieldHeight)
                }
            }
            if let failure { Text(failure).kitType(.caption).foregroundStyle(Ink.bad) }
        }
        .setupPanel()
    }

    private func save() {
        let update = ConnectorUpdate(region: region, cap: capCheck.value.map { .set($0) } ?? .remove)
        Task {
            do {
                try await store.updateConnector(connector.id, update)
                router.rowModes[connector.id] = nil
            } catch {
                failure = error.localizedDescription
            }
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


// MARK: - Add connector

struct AddConnectorSheet: View {
    let request: ConnectorSheetRequest
    @Environment(AppStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var kind: ConnectorKind
    @State private var showsToken: Bool
    @State private var token = ""
    @State private var pick: String?
    @State private var region = "us-east-1"
    @State private var cap = ""
    @State private var profiles: AwsProfiles?
    @State private var found: [DetectedAgent]?
    @State private var cli: AgentCLI = .claudeCode
    @State private var apiKey = ""
    @State private var keyName = ""
    @State private var failure: String?
    @State private var busy = false

    init(request: ConnectorSheetRequest) {
        self.request = request
        _kind = State(initialValue: request.kind)
        _showsToken = State(initialValue: request.showsToken)
        _pick = State(initialValue: request.pick)
    }

    private var canStore: Bool { store.connectorSetup.encryptionAvailable }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            Text("Add connector").kitType(.pageTitle).foregroundStyle(Ink.text)
            Picker("Kind", selection: $kind) {
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
                Button(primaryTitle) { submit() }
                    .buttonStyle(.kit(.primary))
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canSubmit || busy)
            }
        }
        .padding(Space.xxl)
        .frame(width: 520)
        .background(Ink.canvas)
        .onChange(of: kind) { failure = nil }
    }

    @ViewBuilder
    private var fields: some View {
        switch kind {
        case .github: githubFields
        case .aws: awsFields
        case .agent: agentFields
        }
    }

    private var primaryTitle: String {
        switch kind {
        case .github: showsToken ? "Add token" : "Get a code"
        case .aws, .agent: "Add"
        }
    }

    private var canSubmit: Bool {
        switch kind {
        case .github: showsToken ? canStore && SecretKind.githubToken.isAcceptable(token) : true
        case .aws: pick != nil && SettingsCheck.regionProblem(region) == nil && SettingsCheck.cap(cap).problem == nil
            && !region.trimmingCharacters(in: .whitespaces).isEmpty
        case .agent: canStore && cli.secretKind.isAcceptable(apiKey)
        }
    }

    private func submit() {
        busy = true
        failure = nil
        Task {
            defer { busy = false }
            do {
                switch kind {
                case .github:
                    if showsToken {
                        try await store.addGithubToken(token)
                        token = ""
                    } else {
                        await store.startGithubSignIn()
                    }
                case .aws:
                    guard let pick else { return }
                    try await store.addConnectorChecked(NewConnector(
                        kind: .aws, name: pick, region: region.trimmingCharacters(in: .whitespaces),
                        monthlyCapUSD: SettingsCheck.cap(cap).value))
                    Task { await store.refreshConnectors() }
                case .agent:
                    try await store.addConnectorChecked(NewConnector(
                        kind: .agent, name: keyName.isEmpty ? cli.title : keyName, secret: apiKey))
                    apiKey = ""
                    Task { await store.refreshConnectors() }
                }
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
            if !store.connectorSetup.clientIdConfigured {
                CommandText(text: "This build uses the built-in placeholder OAuth client id: set `RATCHET_GITHUB_CLIENT_ID` to your OAuth app.", type: .caption)
            }
            DisclosureGroup(isExpanded: $showsToken) {
                VStack(alignment: .leading, spacing: Space.sm) {
                    if !canStore { keychainNotice("a token") }
                    FieldBlock("Token", problem: token.isEmpty ? nil : SecretKind.githubToken.problem(token)) {
                        SecureField("ghp_… or github_pat_…", text: $token)
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
                let available = profiles.available(excluding: store.connectors)
                if let note = profiles.emptyNote(excluding: store.connectors) {
                    CommandText(text: note + " Run `aws configure sso` in a terminal, then rescan.", type: .meta, color: Ink.textMuted)
                } else {
                    ScrollView {
                        VStack(spacing: Dim.cardGap) {
                            ForEach(available) { profile in
                                ChoiceCard(mark: .radio, isSelected: pick == profile.name) {
                                    pick = profile.name
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
                FieldBlock("Region", problem: SettingsCheck.regionProblem(region)) {
                    TextField("us-east-1", text: $region).kitField().accessibilityLabel("AWS region")
                }
                FieldBlock("Monthly cap (USD), optional", problem: SettingsCheck.cap(cap).problem) {
                    TextField("none", text: $cap).kitField().accessibilityLabel("Monthly cap in dollars")
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
                    let addable = DetectedAgent.addable(found, connected: store.connectors)
                    if addable.isEmpty {
                        Text(DetectedAgent.emptyNote(found)).kitType(.meta).foregroundStyle(Ink.textFaint)
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
                Picker("Agent", selection: $cli) {
                    ForEach(AgentCLI.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                FieldBlock("\(cli.vendor) API key", problem: apiKey.isEmpty ? nil : cli.secretKind.problem(apiKey)) {
                    SecureField("", text: $apiKey).kitField().disabled(!canStore).accessibilityLabel("API key")
                }
                FieldBlock("Name, optional") {
                    TextField(cli.title + " · team key", text: $keyName).kitField().accessibilityLabel("Name")
                }
            }
        }
        .task { found = try? await store.client.detectedAgents(redetect: false) }
    }

    private func addLogin(_ agent: DetectedAgent) {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await store.addConnectorChecked(NewConnector(kind: .agent, name: agent.cli.title))
                dismiss()
            } catch {
                failure = error.localizedDescription
            }
        }
    }

    private func redetect() {
        Task {
            do {
                found = try await store.client.detectedAgents(redetect: true)
                await store.refreshConnectors()
                failure = nil
            } catch {
                failure = error.localizedDescription
            }
        }
    }
}
