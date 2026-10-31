import Observation
import RatchetKit
import SwiftUI

/// The sidebar pages. Snapshot mode walks through every case.
enum Page: String, CaseIterable, Identifiable {
    case overview, optimizations, benchmarks, connectors

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .optimizations: "Optimizations"
        case .benchmarks: "Benchmarks"
        case .connectors: "Connectors"
        }
    }

    var icon: KitIcon {
        switch self {
        case .overview: .overview
        case .optimizations: .optimizations
        case .benchmarks: .benchmarks
        case .connectors: .connectors
        }
    }

    /// Command-1 to Command-4.
    var shortcutKey: KeyEquivalent {
        switch self {
        case .overview: KeyEquivalent(Character("1"))
        case .optimizations: KeyEquivalent(Character("2"))
        case .benchmarks: KeyEquivalent(Character("3"))
        case .connectors: KeyEquivalent(Character("4"))
        }
    }
}

/// Where the window is: the page, the open optimization, the open wizard. Menus, the menu bar item and the
/// snapshot mode all drive it.
@Observable
final class AppRouter {
    var page: Page = .overview
    var optimizationID: String?
    var wizard: NewOptimizationFlow?
    /// Settings reopens on the last tab. Test and snapshot runs leave it alone.
    var settingsTab: SettingsTab = SettingsTab(rawValue: UserDefaults.standard.string(forKey: "settings.tab") ?? "") ?? .general {
        didSet { if !Store.isOverridden { UserDefaults.standard.set(settingsTab.rawValue, forKey: "settings.tab") } }
    }

    /// A page from the sidebar. Choosing the page you are on goes back to its list.
    func show(_ target: Page) {
        if target == page { optimizationID = nil }
        page = target
        wizard = nil
        if target != .optimizations { optimizationID = nil }
    }

    func open(optimization id: String) {
        page = .optimizations
        optimizationID = id
        wizard = nil
    }

    func startWizard(client: any RatchetClient, templateID: String? = nil) {
        let flow = NewOptimizationFlow(client: client)
        wizard = flow
        page = .optimizations
        optimizationID = nil
        Task {
            await flow.load()
            if let templateID { flow.useTemplate(templateID) }
        }
    }

    /// "Review" on an optimization that waits for its environment: the wizard, on the Environment step.
    func reviewEnvironment(of optimization: Optimization, client: any RatchetClient) {
        let flow = NewOptimizationFlow(client: client)
        wizard = flow
        page = .optimizations
        optimizationID = nil
        Task {
            await flow.load()
            await flow.selectRepo(optimization.repo)
            flow.goal = optimization.goal
            await flow.go(to: .environment)
        }
    }

    func closeWizard() {
        wizard = nil
    }
}
