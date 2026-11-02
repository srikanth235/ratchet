import RatchetKit
import SwiftUI

/// "Yours | Discover" on the Connectors page.
enum ConnectorsTab: String, CaseIterable, Identifiable, Hashable {
    case yours, discover

    var id: String { rawValue }
    var title: String { self == .yours ? "Yours" : "Discover" }
}

/// What can be added, one row each: GitHub, AWS and the three agents' API keys. Each row has a 28 pt button that opens the Add
/// sheet on that kind (a real, labelled button, so the hit target is never smaller than 28 pt).
struct DiscoverConnectors: View {
    let add: (ConnectorSheetRequest) -> Void
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    private struct Entry: Identifiable {
        let id: String
        let title: String
        let detail: String
        let request: ConnectorSheetRequest
    }

    private var entries: [Entry] {
        var list = [
            Entry(id: "github", title: "GitHub", detail: "Ratchet pushes candidate branches and opens the pull request as you.",
                  request: ConnectorSheetRequest(kind: .github)),
            Entry(id: "aws", title: "AWS", detail: "The evaluator runs in a sandbox in your account, removed after each run.",
                  request: ConnectorSheetRequest(kind: .aws)),
        ]
        for harness in Harness.allCases {
            list.append(Entry(
                id: "key-" + harness.rawValue, title: harness.title + " API key",
                detail: "A \(harness.vendor) key can also run in the cloud, with your laptop closed.",
                request: ConnectorSheetRequest(kind: .agent, harness: harness)))
        }
        return list
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            Text("Add an account or a key").kitType(.section).foregroundStyle(Ink.text)
            HairlineList {
                ForEach(entries) { entry in
                    HairlineRow {
                        TitleStack(title: entry.title, subtitle: entry.detail, subtitleLines: 2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        KitIconButton(icon: .plus, label: "Add " + entry.title, small: true) { add(entry.request) }
                            .disabled(!actionsEnabled)
                    }
                }
            }
        }
    }
}
