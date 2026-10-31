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
    static let status: CGFloat = 156
    static let result: CGFloat = 224
    static let spent: CGFloat = 96
    static let value: CGFloat = 80
}

/// The optimizations table of the Overview and the Optimizations page.
struct OptimizationTable: View {
    let optimizations: [Optimization]
    let onOpen: (Optimization) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Space.lg) {
                header("Optimization").frame(maxWidth: .infinity, alignment: .leading)
                header("Status").frame(width: TableColumns.status, alignment: .leading)
                header("Result").frame(width: TableColumns.result, alignment: .leading)
                header("Spent").frame(width: TableColumns.spent, alignment: .trailing)
                header("Value / yr").frame(width: TableColumns.value, alignment: .trailing)
            }
            .padding(.bottom, Space.sm + 2)
            .overlay(alignment: .bottom) { Hairline() }

            ForEach(optimizations) { optimization in
                Button { onOpen(optimization) } label: { row(optimization) }
                    .buttonStyle(TableRowButtonStyle())
                    .accessibilityHint("Opens the optimization")
            }
        }
    }

    private func header(_ title: String) -> some View {
        Text(title).kitType(.caption).foregroundStyle(Ink.textFaint)
    }

    private func row(_ optimization: Optimization) -> some View {
        HStack(spacing: Space.lg) {
            TitleStack(title: optimization.title, subtitle: optimization.repo)
                .frame(maxWidth: .infinity, alignment: .leading)
            StatusLabel(tone: optimization.status.tone, text: optimization.status.title, caption: caption(optimization))
                .frame(width: TableColumns.status, alignment: .leading)
            ResultCell(optimization: optimization)
                .frame(width: TableColumns.result, alignment: .leading)
            SpentCell(optimization: optimization)
                .frame(width: TableColumns.spent, alignment: .trailing)
            Text(optimization.valuePerYearUSD.map(Money.short) ?? "—")
                .kitType(.mono)
                .foregroundStyle(optimization.valuePerYearUSD == nil ? Ink.textFaint : Ink.text)
                .frame(width: TableColumns.value, alignment: .trailing)
        }
        .padding(.vertical, Dim.rowPadding)
    }

    private func caption(_ optimization: Optimization) -> String? {
        guard optimization.status == .running, let left = optimization.secondsLeft else { return nil }
        return Elapsed.hm(left) + " left"
    }
}
