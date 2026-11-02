import RatchetKit
import SwiftUI

/// The list of optimizations: an All / Running / Done filter and a page-scoped search in the toolbar, one `Table` that fills the
/// page; opens the detail in place.
struct OptimizationsPage: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @State private var filter: OptimizationFilter = .all
    @State private var query = ""
    @FocusState private var searching: Bool

    var body: some View {
        if let id = router.optimizationID {
            OptimizationDetailView(id: id)
        } else {
            list
        }
    }

    private var list: some View {
        let visible = OptimizationSearch.filter(store.optimizationStore.visibleOptimizations(filter), query: query)
        return VStack(alignment: .leading, spacing: Space.block) {
            PageHeader("Optimizations", subtitle: "Every run, live or done. Open one to see its agents and result.")
                .gridProbe("header", area: "optimizations")
                .padding(.horizontal, Space.pageX)
                .padding(.top, Space.pageTop)
            Group {
                if store.isLoaded && store.optimizationStore.optimizations.isEmpty {
                    EmptyOptimizations()
                } else if visible.isEmpty {
                    if query.isEmpty {
                        ContentUnavailableView("No optimizations here", systemImage: "chart.line.uptrend.xyaxis",
                                               description: Text("Nothing matches this filter."))
                    } else {
                        ContentUnavailableView.search(text: query)
                    }
                } else {
                    OptimizationTable(optimizations: visible)
                        .gridProbe("table", area: "optimizations")
                        .padding(.horizontal, Space.pageX)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: Layout.contentWidth + Space.pageX * 2, maxHeight: .infinity, alignment: .topLeading)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Ink.canvas)
        .gridArea("optimizations", inset: Space.pageX)
        .toolbar {
            ToolbarItem {
                Picker("Show", selection: $filter) {
                    ForEach(OptimizationFilter.allCases) { filter in Text(filter.title).tag(filter) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .help("Show all optimizations, the running ones or the finished ones")
            }
        }
        .searchable(text: $query, placement: .toolbar, prompt: "Search optimizations")
        .searchFocused($searching)
        .focusedSceneValue(\.findAction, FindAction { searching = true })
    }
}

/// Nothing to list yet.
struct EmptyOptimizations: View {
    @Environment(AppRouter.self) private var router
    @Environment(AppStore.self) private var store
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    var body: some View {
        VStack(spacing: Space.lg) {
            RatchetMarkView(height: Dim.markLarge)
            Text("No optimizations yet").kitType(.pageTitle).foregroundStyle(Ink.text)
            Text("Start your first optimization.").kitType(.body).foregroundStyle(Ink.textMuted)
            Button("New Optimization") { router.newOptimization() }
                .buttonStyle(.kit(.primary))
                .disabled(!actionsEnabled)
        }
        .frame(maxWidth: .infinity)
    }
}
