import AppKit
import RatchetKit
import SwiftUI

/// An AWS connector's region and monthly cap, edited where it stands.
struct AwsEditForm: View {
    let connector: Connector
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ConnectorsPageState.self) private var connectorsState
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
                        Button("Cancel") { connectorsState.rowModes[connector.id] = nil }
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
                try await store.connectorStore.updateConnector(connector.id, update)
                connectorsState.rowModes[connector.id] = nil
            } catch {
                failure = error.localizedDescription
            }
        }
    }
}
