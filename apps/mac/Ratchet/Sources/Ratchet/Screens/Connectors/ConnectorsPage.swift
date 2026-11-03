import AppKit
import RatchetKit
import SwiftUI

/// GitHub, AWS and agent connectors. Refresh All and Add… are in the toolbar, search filters the list.
struct ConnectorsPage: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ConnectorsPageState.self) private var connectorsState
    @Environment(\.daemonActionsEnabled) private var actionsEnabled
    @State private var sheet: ConnectorSheetRequest?
    @State private var refreshing = false
    @State private var query = ""
    @State private var tab: ConnectorsTab = .yours
    @State private var focus = SnapshotFocus.shared
    @FocusState private var searching: Bool

    private let areaName = "connectors"

    private func matches(_ connector: Connector) -> Bool {
        let words = query.split(whereSeparator: \.isWhitespace).map(String.init)
        return words.allSatisfy { connector.name.localizedStandardContains($0) || connector.detail.localizedStandardContains($0) }
    }

    var body: some View {
        KitPage {
            PageHeader("Connectors", subtitle: "Accounts each optimization can use. Secrets stay in your keychain.")
                .gridProbe("header", area: areaName)
            // Yours | Discover sits on its own row, under the title. (The Agents page has no such switch.)
            KitSegmented(items: ConnectorsTab.allCases, selection: $tab, title: { $0.title },
                         help: { $0 == .yours ? "Accounts you have connected" : "Accounts and agents you can add" })
                .fixedSize()
                .dimsWhenInactive()
                .gridProbe("tabs", area: areaName)
            if tab == .discover {
                DiscoverConnectors { request in sheet = request }
                    .gridProbe("discover", area: areaName)
            } else {
                yours
            }
        }
        .gridArea(areaName, inset: Space.pageX)
        .task(id: focus.connectorsTab) { tab = (Store.isOverridden && focus.connectorsTab == "discover") ? .discover : .yours }
        .sheet(item: $sheet) { AddConnectorSheet(request: $0) }
        .onChange(of: connectorsState.sheet) { _, request in takeRequest(request) }
        .onAppear { takeRequest(connectorsState.sheet) }
        .toolbar {
            ToolbarItemGroup {
                Button(refreshing ? "Checking…" : "Refresh All", systemImage: "arrow.clockwise") {
                    Task {
                        refreshing = true
                        await store.connectorStore.refreshConnectors()
                        refreshing = false
                    }
                }
                .disabled(refreshing || !actionsEnabled)
                .help("Check every connector with its provider")
                Button("Add…", systemImage: "plus") { sheet = ConnectorSheetRequest(kind: .github) }
                    .disabled(!actionsEnabled)
                    .help("Add a GitHub account, an AWS profile or an agent (Shift-Command-N)")
            }
        }
        .searchable(text: $query, placement: .toolbar, prompt: "Search connectors")
        .searchFocused($searching)
        .focusedSceneValue(\.findAction, FindAction { searching = true })
    }

    /// The connectors that were added, by kind.
    @ViewBuilder
    private var yours: some View {
            if !store.connectorStore.connectorSetup.encryptionAvailable {
                NoteLine(
                    tone: .wait,
                    text: "The keychain is unavailable, so tokens and API keys cannot be stored. `GITHUB_TOKEN`, `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` from the environment are still honoured.")
                    .setupPanel()
                    .gridProbe("keychain", area: areaName)
            }
            if let flow = store.connectorStore.githubFlow {
                GithubFlowCard(flow: flow)
                    .gridProbe("github-flow", area: areaName)
            }
            ForEach(ConnectorKind.allCases) { kind in
                KitSection(kind.title) {
                    let all = store.connectorStore.connectors(of: kind)
                    let list = all.filter(matches)
                    if all.isEmpty {
                        Text(Self.emptyText(kind)).kitType(.meta).foregroundStyle(Ink.textFaint)
                    } else if list.isEmpty {
                        Text("No \(kind.title) connector matches “\(query)”.").kitType(.meta).foregroundStyle(Ink.textFaint)
                    } else {
                        HairlineList {
                            ForEach(list) { connector in
                                ConnectorRow(connector: connector)
                            }
                        }
                    }
                }
                .gridProbe(kind.rawValue, area: areaName)
                .id("connectors-" + kind.rawValue)
            }
    }

    /// "Add Connector…" from the menu bar, or a snapshot scene, asks for the sheet through `ConnectorsPageState`.
    private func takeRequest(_ request: ConnectorSheetRequest?) {
        guard let request else { return }
        sheet = request
        connectorsState.sheet = nil
    }

    private static func emptyText(_ kind: ConnectorKind) -> String {
        switch kind {
        case .github: "No GitHub account yet. Ratchet pushes candidate branches and opens the pull request as you."
        case .aws: "No AWS profile yet. The evaluator runs in a sandbox in your AWS account, removed after each run."
        case .agent: "No agent yet. Install Claude Code, Codex or Grok on this Mac, or add an API key."
        }
    }
}
