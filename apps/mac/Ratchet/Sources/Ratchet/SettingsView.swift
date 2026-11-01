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
    @Environment(DaemonController.self) private var daemon
    @Environment(AppNotifier.self) private var notifier
    @State private var installNote: String?

    var body: some View {
        Section("Menu bar") {
            Toggle("Show Ratchet in the menu bar", isOn: $showMenuBarItem)
            Text("Running optimizations, what needs you and the sandbox, one click away. Closing the window keeps Ratchet running while this is on.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        Section("Background service") {
            LabeledContent("Status", value: statusText)
            if daemon.management == .managed {
                Toggle("Demo mode", isOn: Binding(
                    get: { daemon.mode == .demo },
                    set: { on in
                        daemon.setMode(on ? .demo : .real)
                        Task { await store.useSource(isDemoData: on, cacheURL: Store.cacheURL(demo: on)) }
                    }))
                Text("Demo mode runs ratchetd on made-up data, with no accounts. Your real optimizations are untouched and come back when you switch it off.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("Restart") { daemon.restart() }
                    if daemon.phase == .stopped {
                        Button("Start") { daemon.startAgain() }
                    } else {
                        Button("Stop") { daemon.stop() }
                    }
                    Button("Open log") { DaemonLog.reveal() }
                }
                Text("ratchetd keeps optimizations running after you quit Ratchet. Stopping it pauses them. Login Items in System Settings lists it as Ratchet.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        Section("Notifications") {
            switch notifier.authorization {
            case .allowed:
                ForEach(NotificationCategory.allCases) { category in
                    NotificationSwitch(category: category)
                }
            case .denied:
                Text("Notifications for Ratchet are off in System Settings.").font(.caption).foregroundStyle(.secondary)
                Button("Open Notification Settings") { notifier.openSystemSettings() }
            case .notDetermined, .unknown:
                Text("Ratchet can tell you when a win is ready to merge, an environment waits for approval, a budget is reached or a run finishes.")
                    .font(.caption).foregroundStyle(.secondary)
                Button("Turn on notifications") { Task { await notifier.requestPermission() } }
            }
        }
        Section("Command line") {
            LabeledContent("ratchet", value: CommandLineTool.installed ? "Installed in /usr/local/bin" : "Not installed")
            if CommandLineTool.available {
                Button(CommandLineTool.installed ? "Remove command line tool" : "Install command line tool…") {
                    installNote = CommandLineTool.installed ? CommandLineTool.uninstall() : CommandLineTool.install()
                }
                if let installNote { Text(installNote).font(.caption).foregroundStyle(.secondary) }
            }
            Text("Adds a ratchet command to your Terminal, for scripts and agents. Only with your OK; it is a link to the copy inside this app.")
                .font(.caption).foregroundStyle(.secondary)
        }
        Section("Data") {
            LabeledContent("Source", value: sourceText)
            LabeledContent("Optimizations", value: "\(store.optimizations.count)")
        }
        AdvancedSettingsSection()
        Section("Shortcuts") {
            LabeledContent("New optimization", value: "Command-N")
            LabeledContent("Overview, Optimizations, Benchmarks, Connectors", value: "Command-1 to 4")
            LabeledContent("Refresh", value: "Command-R")
        }
    }

    private var statusText: String {
        switch daemon.phase {
        case .running: "Running"
        case .starting, .checking: "Starting…"
        case .needsApproval: "Waiting for your OK in Login Items"
        case .stopped: "Stopped"
        case .failed(let why): "Failed: " + why
        }
    }

    private var sourceText: String {
        if Store.isOverridden { return "Sample data (test mode)" }
        if daemon.management == .none { return "Sample data" }
        return store.isDemoData ? "ratchetd, demo mode" : "ratchetd"
    }
}

private struct NotificationSwitch: View {
    let category: NotificationCategory
    @AppStorage private var isOn: Bool

    init(category: NotificationCategory) {
        self.category = category
        _isOn = AppStorage(wrappedValue: true, category.defaultsKey)
    }

    var body: some View {
        Toggle(isOn: $isOn) {
            VStack(alignment: .leading) {
                Text(category.title)
                Text(category.explanation).font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}
