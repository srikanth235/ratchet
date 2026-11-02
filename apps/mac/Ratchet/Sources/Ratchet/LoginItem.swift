import AppKit
import Observation
import ServiceManagement

/// "Launch at login": registers the app itself with `SMAppService.mainApp`, so notifications keep arriving after a quit
/// or a reboot (the app is what posts them). A login launch starts with only the menu bar item (`AppDelegate`).
///
/// The system owns the truth (the person can switch the item off in System Settings > General > Login Items), so
/// `refresh()` runs on launch and whenever the app becomes active. Test and snapshot mode never touch the system: they
/// would register the test binary.
@MainActor @Observable
final class LoginItem {
    enum State: Equatable {
        case off
        case on
        /// Registered, but the person has to allow it in System Settings.
        case needsApproval
        /// Not a proper app bundle (a `swift run` build): the system cannot register it.
        case unavailable

        init(_ status: SMAppService.Status) {
            switch status {
            case .enabled: self = .on
            case .requiresApproval: self = .needsApproval
            case .notRegistered: self = .off
            case .notFound: self = .unavailable
            @unknown default: self = .unavailable
            }
        }

        /// The switch reads "on" while the system waits for approval: the request is made, one step is left.
        var isRequested: Bool { self == .on || self == .needsApproval }
    }

    private(set) var state: State = .off
    @ObservationIgnored private let touchesSystem: Bool

    init(touchesSystem: Bool) {
        self.touchesSystem = touchesSystem
        refresh()
    }

    func refresh() {
        guard touchesSystem else { return }
        state = State(SMAppService.mainApp.status)
    }

    /// Turns the login item on or off. Returns the system's reason when it refuses, for the banner.
    func set(_ on: Bool) -> String? {
        guard touchesSystem else {
            state = on ? .on : .off
            return nil
        }
        do {
            if on {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            refresh()
            return nil
        } catch {
            refresh()
            return "Couldn't turn \(on ? "on" : "off") Launch at Login. \(error.localizedDescription)"
        }
    }

    func openSystemSettings() {
        if touchesSystem { SMAppService.openSystemSettingsLoginItems() }
    }

    /// Snapshot mode: show this state without asking the system.
    func preview(_ state: State) { self.state = state }
}

/// How this launch started. A login launch (the login item above) shows only the menu bar item; the window opens when
/// the person asks for it (the menu bar item, a notification, the Dock icon).
@MainActor
enum LaunchContext {
    /// The open-application event carries `keyAELaunchedAsLogInItem` ('lgit') when the system started the app at login.
    nonisolated static func isLoginLaunch(_ event: NSAppleEventDescriptor?) -> Bool {
        guard let event, event.eventID == AEEventID(kAEOpenApplication) else { return false }
        return event.paramDescriptor(forKeyword: AEKeyword(keyAEPropData))?.enumCodeValue == 0x6C67_6974
    }

    /// Set once, as the app finishes launching.
    private(set) static var launchedAtLogin = false

    static func record(_ event: NSAppleEventDescriptor?) { launchedAtLogin = isLoginLaunch(event) }

    /// True when this launch should keep the window closed: a login launch, with the menu bar item on (it is the only way
    /// back in), outside test and snapshot mode.
    static var startsInMenuBar: Bool {
        guard launchedAtLogin, !Store.isOverridden, !SelfTest.requested else { return false }
        return UserDefaults.standard.object(forKey: "menuBarItem") as? Bool ?? true
    }

    /// The app's main window (not Settings, not the menu bar panel).
    private static var mainWindows: [NSWindow] { MainWindow.window.map { [$0] } ?? [] }

    /// Hides the window and the Dock icon. The view stays alive, so the store keeps loading and notifications keep coming.
    static func hideWindow() {
        NSApp.setActivationPolicy(.accessory)
        for window in mainWindows { window.orderOut(nil) }
    }

    /// Brings the window back (and the Dock icon) for a person who asked for it.
    static func revealWindow(open: () -> Void) {
        NSApp.setActivationPolicy(.regular)
        open()
        for window in mainWindows { window.makeKeyAndOrderFront(nil) }
        NSApp.activate()
    }
}
