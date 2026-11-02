import Observation
import RatchetKit
import SwiftUI

/// Where the window is: the page, the open optimization, the open wizard. Menus, the menu bar item and the
/// snapshot mode all drive it.
@Observable
final class AppRouter {
    var page: Page = .overview
    var optimizationID: String?
    var wizard: NewOptimizationFlow?
    /// Settings > General > Advanced (development) is open. Collapsed every time the app starts.
    var advancedExpanded = false
    /// "Use template": the template whose pull request is being set up.
    var templateProposal: TemplateProposalFlow?
    /// Settings reopens on the last tab. Test and snapshot runs leave it alone.
    var settingsTab: SettingsTab = SettingsTab(rawValue: UserDefaults.standard.string(forKey: "settings.tab") ?? "") ?? .general {
        didSet { if !Store.isOverridden { UserDefaults.standard.set(settingsTab.rawValue, forKey: "settings.tab") } }
    }

    /// The wizard's loading work. It ends with the wizard: closing it or going elsewhere cancels what it was still doing.
    @ObservationIgnored private var wizardTask: Task<Void, Never>?

    /// A page from the sidebar. Choosing the page you are on goes back to its list.
    func show(_ target: Page) {
        if target == page { optimizationID = nil }
        page = target
        closeWizard()
        if target != .optimizations { optimizationID = nil }
    }

    func open(optimization id: String) {
        page = .optimizations
        optimizationID = id
        closeWizard()
    }

    func startWizard(client: any RatchetClient, templateID: String? = nil) {
        closeWizard()
        let flow = NewOptimizationFlow(client: client)
        wizard = flow
        page = .optimizations
        optimizationID = nil
        wizardTask = Task {
            await flow.load()
            if Task.isCancelled { return }
            if let templateID { flow.useTemplate(templateID) }
        }
    }

    /// "Review" on an optimization that waits for its environment: the wizard, on the Environment step.
    func reviewEnvironment(of optimization: Optimization, client: any RatchetClient) {
        closeWizard()
        let flow = NewOptimizationFlow(client: client)
        wizard = flow
        page = .optimizations
        optimizationID = nil
        wizardTask = Task {
            await flow.load()
            if Task.isCancelled { return }
            await flow.selectRepo(optimization.repo)
            if Task.isCancelled { return }
            flow.goal = optimization.goal
            await flow.go(to: .environment)
        }
    }

    func closeWizard() {
        wizardTask?.cancel()
        wizardTask = nil
        wizard = nil
    }
}
