import Observation
import RatchetKit
import SwiftUI

/// Where the window is: the page, the open optimization, the open creation conversation. Menus, the menu bar item and the
/// snapshot mode all drive it.
@Observable
final class AppRouter {
    var page: Page = .overview
    var optimizationID: String?
    /// The chat that is drafting a new optimization with the person (the creation conversation). It takes over Home's place;
    /// nil when Home shows the composer. The old four-step wizard is gone (ADR 0001): its logic stays in RatchetKit.
    var creationChatID: String?
    /// Settings > General > Advanced (development) is open. Collapsed every time the app starts.
    var advancedExpanded = false
    /// Stop… from the header, the table, the context menu or the menu bar: the optimization the one confirmation asks about.
    var pendingStop: String?
    /// Archive… from the optimization page's menu: the optimization the one confirmation asks about.
    var pendingArchive: String?
    /// Settings reopens on the last tab. Test and snapshot runs leave it alone.
    var settingsTab: SettingsTab = SettingsTab(rawValue: UserDefaults.standard.string(forKey: "settings.tab") ?? "") ?? .general {
        didSet { if !Store.isOverridden { UserDefaults.standard.set(settingsTab.rawValue, forKey: "settings.tab") } }
    }
    /// The optimization page's tab: the run, or its settings. Opening an optimization starts on the run.
    var detailTab: DetailTab = .run

    /// Where the window is, as plain values, for `RouteRules` (RatchetKit, tested).
    private var route: RouteState {
        get { RouteState(page: page.rawValue, optimizationID: optimizationID, wizardOpen: creationChatID != nil) }
        set {
            page = Page(rawValue: newValue.page) ?? page
            optimizationID = newValue.optimizationID
            if !newValue.wizardOpen { creationChatID = nil }
        }
    }

    /// A page from the sidebar. Choosing the page you are on goes back to its list.
    func show(_ target: Page) {
        route = RouteRules.show(target.rawValue, from: route)
    }

    func open(optimization id: String) {
        if optimizationID != id { detailTab = .run }
        route = RouteRules.open(optimization: id, from: route)
    }

    /// "New Optimization": Home with the composer ready. Whatever conversation was open is left (it is still in the chat store).
    func newOptimization() {
        route = RouteRules.show(Page.home.rawValue, from: RouteState(page: "", optimizationID: nil, wizardOpen: false))
    }

    /// A chat that drafts an optimization is on screen.
    func openConversation(_ chatID: String) {
        route = RouteRules.startConversation(from: route)
        creationChatID = chatID
    }

    func closeConversation() {
        creationChatID = nil
    }
}

/// The optimization page has two tabs: the run (header, log, steering) and the settings rows.
enum DetailTab: String, CaseIterable, Identifiable {
    case run, settings

    var id: String { rawValue }
    var title: String { self == .run ? "Run" : "Settings" }
}
