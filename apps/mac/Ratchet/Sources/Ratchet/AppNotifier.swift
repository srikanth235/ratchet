import AppKit
import Observation
import RatchetKit
import UserNotifications

/// Posts the notifications the store asks for (`NotificationRouting`) and sends a click to the right screen.
///
/// Permission is asked at a sensible moment, never on launch: after the first optimization starts, or from the button in
/// Settings. A notification is posted only while the app is in the background; with the window in front the user sees it.
@MainActor @Observable
final class AppNotifier: NSObject, UNUserNotificationCenterDelegate {
    enum Authorization: Equatable { case unknown, notDetermined, denied, allowed }

    private(set) var authorization: Authorization = .unknown
    @ObservationIgnored var onOpen: ((NotificationDestination) -> Void)?
    private let enabled: Bool
    private static let askedKey = "notifications.asked"
    /// The user answered the in-app card (Turn on, or Not now); it does not come back.
    private(set) var promptDismissed = UserDefaults.standard.bool(forKey: "notifications.asked")

    /// False in test and snapshot mode and when the process is not an app bundle (`swift run`).
    init(enabled: Bool) {
        self.enabled = enabled && Bundle.main.bundleIdentifier != nil
        super.init()
        guard self.enabled else { return }
        UNUserNotificationCenter.current().delegate = self
        Task { await refreshAuthorization() }
    }

    /// Snapshot mode: show this state in the Settings and the prompt.
    func preview(_ state: Authorization, prompt: Bool) {
        authorization = state
        promptDismissed = !prompt
    }

    var isAllowed: Bool { authorization == .allowed }

    /// The prompt card shows once, while permission was never asked, and the user has not said "Not now".
    var shouldOfferPrompt: Bool {
        authorization == .notDetermined && !promptDismissed
    }

    func dismissPrompt() {
        UserDefaults.standard.set(true, forKey: Self.askedKey)
        promptDismissed = true
    }

    func refreshAuthorization() async {
        guard enabled else { return }
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        switch settings.authorizationStatus {
        case .notDetermined: authorization = .notDetermined
        case .denied: authorization = .denied
        default: authorization = .allowed
        }
    }

    /// Shows the system's permission dialog (once; the system remembers the answer).
    func requestPermission() async {
        guard enabled else { return }
        dismissPrompt()
        _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
        await refreshAuthorization()
    }

    func openSystemSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension") {
            NSWorkspace.shared.open(url)
        }
    }

    /// Posts one notification when the app is in the background and the user allowed them.
    func post(_ request: NotificationRequest) {
        guard enabled, isAllowed, !NSApp.isActive else { return }
        let content = UNMutableNotificationContent()
        content.title = request.title
        content.body = request.body
        content.sound = .default
        content.threadIdentifier = request.category.rawValue
        content.userInfo = ["destination": request.destination.token]
        let note = UNNotificationRequest(identifier: request.id, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(note)
    }

    // MARK: UNUserNotificationCenterDelegate

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let token = response.notification.request.content.userInfo["destination"] as? String
        await MainActor.run {
            NSApp.activate()
            if let token, let destination = NotificationDestination(token: token) { onOpen?(destination) }
        }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        // Posted only in the background; if the app came forward meanwhile, stay quiet.
        []
    }
}

/// The per-category switches of Settings (on unless turned off).
enum NotificationPreferences {
    static func isOn(_ category: NotificationCategory) -> Bool {
        UserDefaults.standard.object(forKey: category.defaultsKey) as? Bool ?? true
    }

    static var enabled: Set<NotificationCategory> {
        Set(NotificationCategory.allCases.filter(isOn))
    }
}
