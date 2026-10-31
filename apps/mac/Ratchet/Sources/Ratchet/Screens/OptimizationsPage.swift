import RatchetKit
import SwiftUI

/// The list of optimizations with an All / Running / Done filter; opens the detail in place.
struct OptimizationsPage: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @State private var filter: OptimizationFilter = .all

    var body: some View {
        if let id = router.optimizationID {
            OptimizationDetailView(id: id)
        } else {
            list
        }
    }

    private var list: some View {
        let visible = store.visibleOptimizations(filter)
        return KitPage {
            PageHeader("Optimizations", subtitle: "Every run, live or done. Open one to see its agents and result.") {
                KitSwitch(items: OptimizationFilter.allCases, selection: filter, title: { $0.title }) { filter = $0 }
                Button("New optimization") { router.startWizard(client: store.client) }
                    .buttonStyle(.kit(.primary))
            }
            if visible.isEmpty {
                ContentUnavailableView(
                    "No optimizations here",
                    systemImage: "chart.line.uptrend.xyaxis",
                    description: Text("Start one with New optimization."))
                    .frame(maxWidth: .infinity)
            } else {
                OptimizationTable(optimizations: visible) { router.open(optimization: $0.id) }
            }
        }
    }
}
