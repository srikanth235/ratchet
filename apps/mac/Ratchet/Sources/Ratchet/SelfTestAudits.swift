import AppKit
import RatchetKit
import SwiftUI
import UserNotifications

/// The self-test's checks of the system's side of the app (test-mac-app skill: "the self-test skips system code"): the
/// menus against `Page.allCases` and the commands, every `KitIcon`, a notification's round trip, the `ratchet://` links,
/// and a short look at the event stream.
extension SelfTest {
    // MARK: Menus

    /// SwiftUI builds the menus a moment after the window appears: wait for the Go menu.
    static func auditMenus() async {
        var main: NSMenu?
        for _ in 0..<40 {
            main = NSApp.mainMenu
            if main?.items.contains(where: { $0.title == "Go" }) == true { break }
            try? await Task.sleep(for: .milliseconds(100))
        }
        let go = main?.items.first { $0.title == "Go" }?.submenu
        check("the Go menu exists", go != nil, "menus: " + (main?.items.map(\.title).joined(separator: ", ") ?? "none"))
        for page in Page.allCases {
            let item = go?.items.first { $0.title == page.title }
            check("the Go menu lists \(page.title)", item != nil)
            let key = String(page.shortcutKey.character)
            check(
                "\(page.title) is Command-\(key)",
                item?.keyEquivalent == key && item?.keyEquivalentModifierMask.contains(.command) == true,
                "key '\(item?.keyEquivalent ?? "none")'")
            check("\(page.title) is enabled", item?.isEnabled == true)
        }
        check("Refresh is Command-R", go?.items.first { $0.title == "Refresh" }?.keyEquivalent == "r")
        let file = main?.items.compactMap(\.submenu).first { $0.items.contains { $0.title == "New Optimization" } }
        let new = file?.items.first { $0.title == "New Optimization" }
        check("New Optimization is in a menu with Command-N", new?.keyEquivalent == "n" && new?.keyEquivalentModifierMask.contains(.command) == true)
        // No two commands of the Go menu share a key.
        let keys = (go?.items ?? []).map(\.keyEquivalent).filter { !$0.isEmpty }
        check("no two Go commands share a key", Set(keys).count == keys.count)
    }

    // MARK: Icons

    /// Every case of `KitIcon`. Adding a case to the icon set means adding it here, so it is drawn and checked.
    static let kitIcons: [KitIcon] = [.overview, .optimizations, .benchmarks, .connectors, .check, .back, .plus, .close, .alert]

    /// Each icon draws something visible, at the size of the sidebar and at twice that.
    static func auditIcons() {
        for icon in kitIcons {
            for size in [CGFloat(16), CGFloat(32)] {
                let renderer = ImageRenderer(content: KitIconView(icon: icon, size: size).foregroundStyle(Color.black).frame(width: size, height: size))
                renderer.scale = 2
                check("KitIcon \(icon) draws at \(Int(size)) pt", renderer.cgImage.map(hasInk) ?? false)
            }
        }
        for page in Page.allCases {
            check("\(page.title) has an icon in the set", kitIcons.contains { "\($0)" == "\(page.icon)" })
        }
    }

    private static func hasInk(_ image: CGImage) -> Bool {
        let rep = NSBitmapImageRep(cgImage: image)
        for y in 0..<rep.pixelsHigh {
            for x in 0..<rep.pixelsWide where (rep.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0.05 { return true }
        }
        return false
    }

    // MARK: Notifications and links

    /// A notification's content carries its destination, a click (and a link) reaches the router, and the app registers
    /// the `ratchet://` scheme.
    static func auditNotifications(store: AppStore, router: AppRouter, notifier: AppNotifier) async {
        let requests = [
            NotificationRequest(id: "selftest-win", category: .winReady, title: "Win", body: "Ready", destination: .optimization("opt-checkout")),
            NotificationRequest(id: "selftest-conn", category: .connectorExpired, title: "Expired", body: "Sign in", destination: .connectors),
            NotificationRequest(id: "selftest-draft", category: .environmentDrafted, title: "Drafted", body: "Approve", destination: .newOptimization),
            NotificationRequest(id: "selftest-done", category: .runFinished, title: "Done", body: "Finished", destination: .overview),
        ]
        for request in requests {
            let content = AppNotifier.content(for: request)
            check(
                "a \(request.category.rawValue) notification carries its destination",
                AppNotifier.destination(of: content) == request.destination && content.title == request.title
                    && content.body == request.body && content.threadIdentifier == request.category.rawValue)
        }
        check("a foreign notification has no destination", AppNotifier.destination(of: UNMutableNotificationContent()) == nil)

        // A click reaches the router, through the handler the window set.
        router.show(.overview)
        notifier.open(.connectors)
        check("a click on a connector notification opens Connectors", await settles { router.page == .connectors })
        notifier.open(.optimization("opt-checkout"))
        check(
            "a click on a win opens the optimization",
            await settles { router.page == .optimizations && router.optimizationID == "opt-checkout" })
        notifier.open(.newOptimization)
        check("a click on an environment draft opens the flow", await settles { router.wizard != nil })
        router.closeWizard()
        router.show(.overview)

        // Links: the same destinations, and every page by name.
        let pages = Page.allCases.map(\.rawValue)
        for page in Page.allCases {
            let url = AppLink.url(opening: page.rawValue)
            check(
                "ratchet://open/\(page.rawValue) opens \(page.title)",
                url.flatMap(AppLink.init(url:))?.resolve(pages: pages) == .page(page.rawValue))
        }
        check(
            "ratchet://open/<id> opens an optimization",
            AppLink.url(opening: "opt-checkout").flatMap(AppLink.init(url:))?.resolve(pages: pages) == .destination(.optimization("opt-checkout")))
        let types = Bundle.main.object(forInfoDictionaryKey: "CFBundleURLTypes") as? [[String: Any]] ?? []
        let schemes = types.flatMap { $0["CFBundleURLSchemes"] as? [String] ?? [] }
        check("the app registers the ratchet:// scheme", schemes.contains(AppLink.scheme), "schemes: \(schemes)")
    }

    /// True once `condition` holds, polling for up to a second (the click is handled on the next turns of the run loop).
    private static func settles(_ condition: () -> Bool) async -> Bool {
        for _ in 0..<20 {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return condition()
    }

    // MARK: Event stream

    /// A short smoke test: the client's stream delivers an event within three seconds of the sample world moving.
    static func auditEventStream(store: AppStore) async {
        guard let mock = store.client as? MockRatchetClient else {
            check("the event stream can be smoke-tested on the sample client", false)
            return
        }
        let stream = await mock.events()
        let collector = Task { () -> Int in
            var seen = 0
            for await _ in stream {
                seen += 1
                if seen >= 1 { break }
            }
            return seen
        }
        _ = await mock.advance()
        let timeout = Task {
            try? await Task.sleep(for: .seconds(3))
            collector.cancel()
        }
        let seen = await collector.value
        timeout.cancel()
        check("the event stream delivers an event after the world moves", seen >= 1)
    }
}
