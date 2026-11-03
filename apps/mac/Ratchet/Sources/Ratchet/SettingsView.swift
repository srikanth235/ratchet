import RatchetKit
import SwiftUI

/// The Settings window (Command-Comma): four tabs, each one grouped `Form`, every tab 520 wide so the window never changes
/// width. Native layout and controls; text through `.kitType`, colour through `Ink`. Settings apply at once. Templates, Connectors
/// and agents live in the window's pages, not here.
struct SettingsView: View {
    @Environment(AppRouter.self) private var router
    @Environment(DaemonController.self) private var daemon
    @Environment(AppNotifier.self) private var notifier
    @Environment(LoginItem.self) private var loginItem

    /// A permission is missing and the person may be fixing it in System Settings right now.
    private var watchingPermissions: Bool {
        notifier.authorization == .denied || notifier.authorization == .notDetermined
            || loginItem.state == .needsApproval || daemon.phase == .needsApproval
    }

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
        // While Settings shows a missing permission, look again every two seconds: the answer changes in System
        // Settings, and the app does not always get a notification when it does.
        .task(id: watchingPermissions) {
            guard watchingPermissions else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                await notifier.refreshAuthorization()
                loginItem.refresh()
                daemon.recheck()
                CommandLineTool.shared.refresh()
            }
        }
    }

    @ViewBuilder
    private func page(_ tab: SettingsTab) -> some View {
        Form {
            switch tab {
            case .general: GeneralSettings()
            case .notifications: NotificationSettings()
            case .thisMac: ThisMacSettings()
            case .advanced: AdvancedSettingsPane()
            }
        }
        .formStyle(.grouped)
        .fitsContent()
        .frame(width: Layout.settingsWidth)
    }
}

/// The Settings tabs. Snapshot mode walks through every case.
enum SettingsTab: String, CaseIterable, Identifiable {
    case general, notifications
    case thisMac = "this-mac"
    case advanced

    var id: String { rawValue }

    var title: String {
        switch self {
        case .general: "General"
        case .notifications: "Notifications"
        case .thisMac: "This Mac"
        case .advanced: "Advanced"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gearshape"
        case .notifications: "bell"
        case .thisMac: "desktopcomputer"
        case .advanced: "gearshape.2"
        }
    }
}

// MARK: - General

struct GeneralSettings: View {
    @AppStorage("menuBarItem") private var showMenuBarItem = true
    @Environment(AppRouter.self) private var router
    @Environment(\.openWindow) private var openWindow

    /// The order optimizations try the agents in, as the Agents page keeps it.
    private var agentOrder: String {
        let order = UserDefaultsHarnessPreferences().optimizationOrder
        return order.isEmpty ? "Automatic" : order.map(\.title).joined(separator: " → ")
    }

    var body: some View {
        Section {
            Toggle("Show Ratchet in the menu bar", isOn: $showMenuBarItem)
                .controlSize(.mini)
        } header: {
            Text("Menu bar").kitType(.microStrong).foregroundStyle(Ink.textFaint)
        } footer: {
            Text("Running optimizations, what needs you and the sandbox, one click away. Closing the window keeps Ratchet running while this is on.")
                .kitType(.caption).foregroundStyle(Ink.textFaint)
        }
        Section {
            LabeledContent("Agents", value: agentOrder)
            Button("Change on the Agents Page") {
                LaunchContext.revealWindow { openWindow(id: "main") }
                router.show(.agents)
            }
        } header: {
            Text("Default for new optimizations").kitType(.microStrong).foregroundStyle(Ink.textFaint)
        } footer: {
            Text("A new optimization starts from these. Each optimization shows “Using default” until you change it there.")
                .kitType(.caption).foregroundStyle(Ink.textFaint)
        }
    }
}

// MARK: - Notifications

struct NotificationSettings: View {
    @Environment(AppNotifier.self) private var notifier

    var body: some View {
        Section {
            switch notifier.authorization {
            case .allowed:
                ForEach(NotificationCategory.allCases) { category in
                    NotificationSwitch(category: category)
                }
            case .denied:
                LabeledContent("Notifications", value: "Off in System Settings")
                Button("Open Notification Settings") { notifier.openSystemSettings() }
            case .notDetermined, .unknown:
                LabeledContent("Notifications", value: "Not turned on yet")
                Button("Turn on notifications") { Task { await notifier.requestPermission() } }
            }
        } header: {
            Text("Tell me when").kitType(.microStrong).foregroundStyle(Ink.textFaint)
        } footer: {
            Text(footer).kitType(.caption).foregroundStyle(Ink.textFaint)
        }
    }

    private var footer: String {
        switch notifier.authorization {
        case .allowed: "Posted only while Ratchet is in the background."
        case .denied: "Notifications for Ratchet are off in System Settings, so nothing can reach you."
        case .notDetermined, .unknown:
            "Ratchet can tell you when a win is ready to merge, a budget is reached or a run finishes. It never asks anything else."
        }
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
                Text(category.explanation).kitType(.caption).foregroundStyle(Ink.textFaint)
            }
        }
        .controlSize(.mini)
    }
}

// MARK: - This Mac

/// What belongs to this Mac: the background service, the login item and the command-line tool. Stop and Demo mode ask first.
struct ThisMacSettings: View {
    @AppStorage("menuBarItem") private var showMenuBarItem = true
    @Environment(AppStore.self) private var store
    @Environment(DaemonController.self) private var daemon
    @Environment(LoginItem.self) private var loginItem
    @State private var confirmingStop = false
    @State private var pendingDemo: Bool?
    private let cli = CommandLineTool.shared

    var body: some View {
        Section {
            statusRow
            if daemon.management == .managed {
                Toggle("Demo mode", isOn: Binding(get: { daemon.mode == .demo }, set: { pendingDemo = $0 }))
                    .controlSize(.mini)
                HStack {
                    Button("Restart") { daemon.restart() }
                    if daemon.phase == .stopped {
                        Button("Start") { daemon.startAgain() }
                    } else {
                        Button("Stop…") { confirmingStop = true }
                    }
                    Button("Open Log") { DaemonLog.reveal() }
                }
            }
        } header: {
            Text("Background service").kitType(.microStrong).foregroundStyle(Ink.textFaint)
        } footer: {
            Text(daemon.management == .managed
                 ? "ratchetd keeps optimizations running after you quit Ratchet. Stopping it pauses them. Login Items in System Settings lists it as Ratchet. Demo mode runs it on made-up data with no accounts; your real optimizations come back when you switch it off."
                 : "Ratchet is not managing the background service in this copy.")
                .kitType(.caption).foregroundStyle(Ink.textFaint)
        }
        Section {
            Toggle("Launch at login", isOn: Binding(
                get: { loginItem.state.isRequested },
                set: { on in
                    // Launching at login starts with only the menu bar item, so it needs the item to be on.
                    if on { showMenuBarItem = true }
                    if let problem = loginItem.set(on) { store.reportProblem(problem) }
                }))
                .controlSize(.mini)
                .disabled(loginItem.state == .unavailable)
            if loginItem.state == .needsApproval {
                Button("Open Login Items") { loginItem.openSystemSettings() }
            }
        } header: {
            Text("Login item").kitType(.microStrong).foregroundStyle(Ink.textFaint)
        } footer: {
            Text(loginItemNote).kitType(.caption).foregroundStyle(Ink.textFaint)
        }
        Section {
            LabeledContent("ratchet", value: commandLineStatus)
            if CommandLineTool.available {
                HStack {
                    if cli.isInstalled {
                        Button("Remove") { cli.uninstall() }
                    } else {
                        Button(cli.hasBrokenLink ? "Repair" : "Install") { cli.installForUser() }
                    }
                    if !cli.systemState.isInstalled {
                        Button("Install for All Users…") { cli.installForEveryone() }
                    }
                }
            }
        } header: {
            Text("Command-line tool").kitType(.microStrong).foregroundStyle(Ink.textFaint)
        } footer: {
            VStack(alignment: .leading, spacing: Space.xs) {
                if let note = cli.note { Text(note) }
                Text("Adds a ratchet command to your Terminal, for scripts and agents. It goes in ~/.local/bin, a link to the copy inside this app. “All users” puts one in /usr/local/bin and asks for your password.")
            }
            .kitType(.caption).foregroundStyle(Ink.textFaint)
        }
    }

    /// Stop and Demo mode change what is running: each asks once, says what happens, and can be cancelled.
    private var statusRow: some View {
        LabeledContent("Status", value: statusText)
        .confirmationDialog("Stop the background service?", isPresented: $confirmingStop, titleVisibility: .visible) {
            Button("Stop", role: .destructive) { daemon.stop() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Running optimizations pause until you start it again.")
        }
        .confirmationDialog(
            pendingDemo == true ? "Switch to demo mode?" : "Switch back to your real data?",
            isPresented: Binding(get: { pendingDemo != nil }, set: { if !$0 { pendingDemo = nil } }),
            titleVisibility: .visible, presenting: pendingDemo
        ) { on in
            Button("Switch", role: on ? .destructive : nil) { setDemo(on) }
            Button("Cancel", role: .cancel) {}
        } message: { on in
            Text(on
                 ? "Real runs pause until you switch back. Demo mode shows made-up data and uses no accounts."
                 : "Your real optimizations come back. The made-up data goes away.")
        }
    }

    private func setDemo(_ on: Bool) {
        daemon.setMode(on ? .demo : .real)
        Task { await store.useSource(isDemoData: on, cacheURL: Store.cacheURL(demo: on)) }
    }

    private var commandLineStatus: String {
        let tool = cli
        if tool.userState.isInstalled {
            let hint = tool.userFolderOnPath == false ? " (add it to PATH, see below)" : ""
            return "Installed in ~/.local/bin" + hint
        }
        if tool.systemState.isInstalled { return "Installed in /usr/local/bin" }
        for (state, folder) in [(tool.userState, "~/.local/bin"), (tool.systemState, "/usr/local/bin")] {
            switch state {
            case .broken, .otherCopy, .notALink: return state.sentence(directory: folder)
            case .missing, .linked: continue
            }
        }
        return "Not installed"
    }

    private var loginItemNote: String {
        switch loginItem.state {
        case .off: "Start Ratchet when you log in, with only the menu bar item, so notifications keep arriving after a restart. It is off now."
        case .on: "On. Ratchet starts with only the menu bar item when you log in. Turn it off here or in System Settings > General > Login Items."
        case .needsApproval: "Waiting for your OK: allow Ratchet in System Settings > General > Login Items."
        case .unavailable: "Not available in this build: Launch at login needs Ratchet to run from an installed app."
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
}

// MARK: - Advanced

struct AdvancedSettingsPane: View {
    @Environment(AppStore.self) private var store
    @Environment(DaemonController.self) private var daemon

    var body: some View {
        Section {
            LabeledContent("Source", value: sourceText)
            LabeledContent("Optimizations", value: "\(store.optimizationStore.optimizations.count)")
        } header: {
            Text("Data").kitType(.microStrong).foregroundStyle(Ink.textFaint)
        }
        AdvancedSettingsSection()
    }

    private var sourceText: String {
        if Store.isOverridden { return "Sample data (test mode)" }
        if daemon.management == .none { return "Sample data" }
        return store.isDemoData ? "ratchetd, demo mode" : "ratchetd"
    }
}
