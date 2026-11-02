import Foundation

/// Where the window is, as plain values: the page (by name), the open optimization, and whether the New optimization
/// flow is open. The rules of the sidebar, the menus and the links, kept apart from the observable router.
public struct RouteState: Equatable, Sendable {
    public var page: String
    public var optimizationID: String?
    public var wizardOpen: Bool

    public init(page: String, optimizationID: String? = nil, wizardOpen: Bool = false) {
        self.page = page
        self.optimizationID = optimizationID
        self.wizardOpen = wizardOpen
    }
}

public enum RouteRules {
    /// The page that lists optimizations and shows one in place.
    public static let optimizationsPage = "optimizations"

    /// A page from the sidebar or a menu. Choosing the page you are on goes back to its list, and any page but
    /// Optimizations closes the open optimization. The flow always closes.
    public static func show(_ target: String, from state: RouteState) -> RouteState {
        var next = state
        if target == state.page { next.optimizationID = nil }
        next.page = target
        next.wizardOpen = false
        if target != optimizationsPage { next.optimizationID = nil }
        return next
    }

    /// One optimization, in place on the Optimizations page.
    public static func open(optimization id: String, from state: RouteState) -> RouteState {
        RouteState(page: optimizationsPage, optimizationID: id, wizardOpen: false)
    }

    /// The New optimization flow (also "Review" on an optimization that waits).
    public static func startWizard(from state: RouteState) -> RouteState {
        RouteState(page: optimizationsPage, optimizationID: nil, wizardOpen: true)
    }
}

/// A notification click or link that arrives before the window has said what to do with it (a cold launch: the system
/// starts the app because of the click, and the delegate is called before the first view exists). The newest one waits.
public struct PendingDestination: Equatable, Sendable {
    private var held: NotificationDestination?

    public init() {}

    public mutating func hold(_ destination: NotificationDestination) { held = destination }

    /// The waiting destination, once; nil when none waits.
    public mutating func take() -> NotificationDestination? {
        defer { held = nil }
        return held
    }

    public var isEmpty: Bool { held == nil }
}
