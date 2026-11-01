import AppKit
import RatchetKit
import SwiftUI

// Ratchet
//   Ratchet                          the app, on sample data with a fake live event stream
//   APP_STORE=<file> Ratchet         test mode: fixed clock, no live ticks, no saved snapshot (tests, experiments)
//   APP_SNAPSHOT=<dir> Ratchet       save a PNG of every page, the new-optimization steps, the optimization detail
//                                    and every Settings tab in light and dark mode, then quit
//   Ratchet --selftest               run the self-test on the sample client and quit with 0 or 1 (tools/selftest.sh)
//   APP_GRID=1 Ratchet               draw the 8 pt grid and the layout probes over the window
//   APP_KIT_GALLERY=1 Ratchet        (not built yet) parts gallery
//
// Data comes through the RatchetClient protocol (RatchetKit). The app talks to ratchetd through DaemonRatchetClient: it
// registers the bundled daemon as a LaunchAgent (DaemonController), shows "Starting Ratchet…" until the socket answers,
// and follows its event stream. Test and snapshot mode (APP_STORE) use the built-in sample data (MockRatchetClient).
//   APP_DEMO=1 Ratchet               use the sample data without any daemon
//   RATCHET_HOME=<dir> Ratchet       talk to a daemon you started yourself (ratchetd serve --mode sim) and do not register one
//   Ratchet --selftest-live          run the self-test against that daemon (CI smoke step; see tools/smoke-daemon.sh)

@main
struct RatchetApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var store = Store.makeStore()
    @State private var router = AppRouter()
    @State private var daemon = DaemonController.make()
    @State private var notifier = AppNotifier(enabled: !Store.isOverridden)
    @AppStorage("menuBarItem") private var showMenuBarItem = true

    var body: some Scene {
        Window("Ratchet", id: "main") {
            ContentView()
                .environment(store)
                .environment(router)
                .environment(daemon)
                .environment(notifier)
                .overlay { if GridOverlay.enabled { GridOverlay() } }
                .frame(minWidth: 1080, minHeight: 640)
        }
        .defaultSize(width: 1280, height: 820)
        .windowToolbarStyle(.unified)
        .commands {
            SidebarCommands()
            CommandGroup(replacing: .newItem) {
                Button("New Optimization") { router.startWizard(client: store.client) }
                    .keyboardShortcut("n")
            }
            CommandMenu("Go") {
                ForEach(Page.allCases) { page in
                    Button(page.title) { router.show(page) }
                        .keyboardShortcut(page.shortcutKey, modifiers: .command)
                }
                Divider()
                Button("Refresh") { Task { await store.refresh() } }
                    .keyboardShortcut("r")
            }
        }

        Settings {
            SettingsView()
                .environment(store)
                .environment(router)
                .environment(daemon)
                .environment(notifier)
        }
        .windowResizability(.contentSize)

        MenuBarExtra(isInserted: $showMenuBarItem) {
            MenuBarView()
                .environment(store)
                .environment(router)
        } label: {
            MenuBarLabel()
                .environment(store)
        }
        .menuBarExtraStyle(.window)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationWillFinishLaunching(_ notification: Notification) {
        // The design's fonts, before the first view asks for one.
        _ = KitFonts.registered
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Started from a terminal, the process is not a foreground app yet.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
    }

    /// The menu bar item keeps Ratchet reachable, but closing the window still quits unless it is on.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !UserDefaults.standard.bool(forKey: "menuBarItem") && UserDefaults.standard.object(forKey: "menuBarItem") != nil
    }

    /// The self-test's result becomes the exit code, after a normal quit.
    func applicationWillTerminate(_ notification: Notification) {
        if SelfTest.requested { exit(SelfTest.exitCode) }
    }
}

/// Where the data comes from, and test mode.
enum Store {
    /// True when APP_STORE points somewhere else (tests, snapshots): sample data on a fixed clock, no saved snapshot.
    static var isOverridden: Bool {
        !(ProcessInfo.processInfo.environment["APP_STORE"] ?? "").isEmpty
    }

    /// The last snapshot of what the daemon said, shown at once on the next launch. The demo data has its own file, so
    /// sample numbers never flash up before a real daemon answers.
    static func cacheURL(demo: Bool) -> URL? {
        guard !isOverridden else { return nil }
        return URL.applicationSupportDirectory
            .appending(path: "Ratchet", directoryHint: .isDirectory)
            .appending(path: demo ? "demo-snapshot.json" : "last-snapshot.json")
    }

    /// 2026-10-31 13:20 UTC: the clock of every snapshot and test run.
    static let fixedNow = Date(timeIntervalSince1970: 1_793_452_800)

    /// The daemon client, always (the app starts the daemon itself and waits for it). The sample data only in test and
    /// snapshot mode and with `APP_DEMO=1`. Demo mode (a Settings switch) is the daemon's own simulation, not the sample.
    @MainActor
    static func makeStore() -> AppStore {
        if isOverridden {
            return AppStore(client: MockRatchetClient(now: fixedNow, live: false), cacheURL: nil, isDemoData: true)
        }
        if ProcessInfo.processInfo.environment["APP_DEMO"] == "1" {
            return AppStore(client: MockRatchetClient(now: Date(), live: true), cacheURL: cacheURL(demo: true), isDemoData: true)
        }
        let demoMode = UserDefaults.standard.string(forKey: DaemonController.modeKey) == DaemonMode.demo.rawValue
        return AppStore(
            client: DaemonRatchetClient(location: DaemonLocation.standard()), cacheURL: cacheURL(demo: demoMode),
            isDemoData: demoMode)
    }
}
