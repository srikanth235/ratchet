import AppKit
import RatchetKit
import SwiftUI

/// GitHub, AWS and agent connectors. Also the Connectors tab of Settings (`embedded`).
struct ConnectorsPage: View {
    var embedded = false
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ConnectorsPageState.self) private var connectorsState
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
                            await store.connectorStore.refreshConnectors()
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
            if !store.connectorStore.connectorSetup.encryptionAvailable {
                NoteLine(
                    tone: .wait,
                    text: "The keychain is unavailable, so tokens and API keys cannot be stored. `GITHUB_TOKEN`, `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` from the environment are still honoured.")
                    .setupPanel()
                    .gridProbe(probePrefix + "keychain", area: areaName)
            }
            if let flow = store.connectorStore.githubFlow {
                GithubFlowCard(flow: flow)
                    .gridProbe(probePrefix + "github-flow", area: areaName)
            }
            ForEach(ConnectorKind.allCases) { kind in
                KitSection(kind.title) {
                    let list = store.connectorStore.connectors(of: kind)
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
        .onChange(of: connectorsState.sheet) { _, request in
            guard !embedded, let request else { return }
            sheet = request
            connectorsState.sheet = nil
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
