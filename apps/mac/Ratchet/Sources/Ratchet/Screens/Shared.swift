import RatchetKit
import SwiftUI

// Small pieces shared by several screens: status tones for the domain types, the result cell, the optimization table.

extension OptimizationStatus {
    var tone: StatusTone {
        switch self {
        case .running: .run
        case .waiting: .wait
        case .merged, .done: .ok
        case .noWin: .off
        }
    }
}

extension Verdict {
    var tone: StatusTone {
        switch self {
        case .evaluating: .run
        case .verified: .ok
        case .noise: .off
        case .blocked: .bad
        }
    }
}

extension ConnectorState {
    var tone: StatusTone {
        switch self {
        case .connected, .signedIn, .valid: .ok
        case .expired: .wait
        case .signedOut: .off
        }
    }
}

extension AgentState {
    var tone: StatusTone {
        switch self {
        case .working: .run
        case .paused: .wait
        case .idle: .off
        }
    }
}

/// "−30%  412 → 288 ms", or why there is no result.
struct ResultCell: View {
    let optimization: Optimization

    var body: some View {
        if let change = optimization.change, let text = optimization.resultText {
            HStack(spacing: Space.sm) {
                Text(Percent.signed(change))
                    .kitType(.mono)
                    .foregroundStyle(optimization.isImprovement ? Ink.ok : Ink.bad)
                Text(text).kitType(.meta).foregroundStyle(Ink.textFaint).lineLimit(1)
            }
        } else if optimization.status == .noWin {
            let noise = optimization.noise.map { " (" + Percent.noise($0) + ")" } ?? ""
            Text("nothing beat the noise" + noise).kitType(.meta).foregroundStyle(Ink.textFaint).lineLimit(1)
        } else {
            Text("no win yet").kitType(.meta).foregroundStyle(Ink.textFaint)
        }
    }
}

/// "$38 / 50" in mono, the budget quieter.
struct SpentCell: View {
    let optimization: Optimization

    var body: some View {
        let parts = Figure.spentOf(optimization.spentUSD, budget: optimization.budgetUSD)
        HStack(spacing: 0) {
            Text(parts.spent).kitType(.mono).foregroundStyle(Ink.text)
            if let budget = parts.budget {
                Text(" / " + budget).kitType(.mono).foregroundStyle(Ink.textFaint)
            }
        }
    }
}

enum TableColumns {
    static let status: CGFloat = 128
    static let result: CGFloat = 176
    static let spent: CGFloat = 88
    static let value: CGFloat = 72
}

extension Optimization {
    // Sort keys of the table's columns: the columns whose value is not Comparable as it stands.
    var statusRank: Int {
        switch status {
        case .running: 0
        case .waiting: 1
        case .merged: 2
        case .done: 3
        case .noWin: 4
        }
    }

    var changeSort: Double { change ?? 0 }
    var valueSort: Double { valuePerYearUSD ?? 0 }
}

/// The optimizations table of the Overview and the Optimizations page: a native `Table` with selection (one or several rows),
/// sorting by any column, a context menu (Open, Run Now, Stop…, Open Pull Request, Copy Link), a drag that carries the pull
/// request's link, and Return or a double click to open. The selection is published to the menu bar (`optimizationSelection`).
struct OptimizationTable: View {
    let optimizations: [Optimization]
    /// True inside a scrolling page (the Overview): the table takes the height its rows need and scrolls with the page. False:
    /// it fills the space it is given and scrolls itself.
    var sizesToRows = false
    @Environment(AppRouter.self) private var router
    @State private var selection = Set<Optimization.ID>()
    @State private var sortOrder: [KeyPathComparator<Optimization>] = []

    /// A two-line row and the header, with room to spare: the table has no way to say how tall its rows came out.
    private static let rowHeight: CGFloat = 56
    private static let headerHeight: CGFloat = 32

    var body: some View {
        let rows = sortOrder.isEmpty ? optimizations : optimizations.sorted(using: sortOrder)
        Table(of: Optimization.self, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Optimization", value: \.title) { optimization in
                TitleStack(title: optimization.title, subtitle: optimization.repo)
            }
            TableColumn("Status", value: \.statusRank) { optimization in
                StatusLabel(tone: optimization.status.tone, text: optimization.status.title, caption: Self.caption(optimization))
            }
            .width(min: 112, ideal: 128, max: 160)
            TableColumn("Result", value: \.changeSort) { optimization in
                ResultCell(optimization: optimization)
            }
            .width(min: 144, ideal: 176)
            TableColumn("Spent", value: \.spentUSD) { optimization in
                SpentCell(optimization: optimization).frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(min: 72, ideal: 88, max: 120)
            TableColumn("Value / yr", value: \.valueSort) { optimization in
                Text(optimization.valuePerYearUSD.map(Money.short) ?? "—")
                    .kitType(.mono)
                    .foregroundStyle(optimization.valuePerYearUSD == nil ? Ink.textFaint : Ink.text)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(min: 72, ideal: 96, max: 120)
        } rows: {
            ForEach(rows) { optimization in
                TableRow(optimization)
                    .draggable(OptimizationAction.link(for: optimization))
            }
        }
        .tableStyle(.inset)
        .scrollContentBackground(.hidden)
        .contextMenu(forSelectionType: Optimization.ID.self) { ids in
            OptimizationMenuItems(ids: Array(ids))
        } primaryAction: { ids in
            if ids.count == 1, let id = ids.first { router.open(optimization: id) }
        }
        .focusedSceneValue(\.optimizationSelection, Array(selection))
        .frame(height: sizesToRows ? CGFloat(rows.count) * Self.rowHeight + Self.headerHeight : nil)
        .scrollDisabled(sizesToRows)
        .accessibilityLabel("Optimizations")
    }

    private static func caption(_ optimization: Optimization) -> String? {
        guard optimization.status == .running, let left = optimization.secondsLeft else { return nil }
        return Elapsed.hm(left) + " left"
    }
}
