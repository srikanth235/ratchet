import RatchetKit
import SwiftUI

/// The Settings window (Command-Comma): General, and the same Connectors as the sidebar page.
struct SettingsView: View {
    @Environment(AppRouter.self) private var router

    var body: some View {
        @Bindable var router = router
        TabView(selection: $router.settingsTab) {
            ForEach(SettingsTab.allCases) { tab in
                page(tab)
                    .tabItem { Label(tab.title, systemImage: tab.symbol) }
                    .tag(tab)
            }
        }
        .noInitialFocus()
    }

    @ViewBuilder
    private func page(_ tab: SettingsTab) -> some View {
        switch tab {
        case .general:
            Form { GeneralSettings() }
                .formStyle(.grouped)
                .fitsContent()
                .frame(width: 560)
        case .connectors:
            ConnectorsPage(embedded: true)
                .frame(width: 800, height: 600)
        }
    }
}

/// The Settings tabs. Snapshot mode walks through every case.
enum SettingsTab: String, CaseIterable, Identifiable {
    case general, connectors

    var id: String { rawValue }

    var title: String {
        switch self {
        case .general: "General"
        case .connectors: "Connectors"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gearshape"
        case .connectors: "link"
        }
    }
}

struct GeneralSettings: View {
    @AppStorage("menuBarItem") private var showMenuBarItem = true
    @Environment(AppStore.self) private var store

    var body: some View {
        Section("Menu bar") {
            Toggle("Show Ratchet in the menu bar", isOn: $showMenuBarItem)
            Text("Running optimizations, what needs you and the sandbox, one click away. Closing the window keeps Ratchet running while this is on.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        Section("Data") {
            LabeledContent("Source", value: "Sample data (ratchetd is not connected yet)")
            LabeledContent("Optimizations", value: "\(store.optimizations.count)")
        }
        Section("Shortcuts") {
            LabeledContent("New optimization", value: "Command-N")
            LabeledContent("Overview, Optimizations, Benchmarks, Connectors", value: "Command-1 to 4")
            LabeledContent("Refresh", value: "Command-R")
        }
    }
}
