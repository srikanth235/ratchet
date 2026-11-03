import Foundation

// What the sample world can say about itself. Pure: the client and the simulation both ask it.

extension MockWorld {
    func overview(_ range: TimeRange) -> Overview {
        let windowDays = Double(range.days ?? 120)
        let start = now.addingTimeInterval(-windowDays * MockWorld.day)
        let wins = winSeries.sorted { $0.date < $1.date }
        let before = wins.last { $0.date < start }?.valueUSD ?? 0
        let inside = wins.filter { $0.date >= start }
        var series = [ValuePoint(id: "start", date: start, valuePerYearUSD: before)]
        for win in inside {
            series.append(ValuePoint(
                id: "win-\(Int(win.date.timeIntervalSince1970))", date: win.date, valuePerYearUSD: win.valueUSD,
                win: win.title, optimizationTitle: win.optimizationTitle))
        }
        series.append(ValuePoint(id: "end", date: now, valuePerYearUSD: wins.last?.valueUSD ?? 0))
        let spent = optimizations
            .filter { ($0.startedAt ?? now) >= start }
            .reduce(0) { $0 + $1.spentUSD }
        return Overview(
            range: range, spentUSD: spent, budgetUSD: budgetUSD, valueLockedPerYearUSD: totalValue(),
            winsCount: inside.count, series: series, needsYou: needsYou())
    }

    func totalValue() -> Double {
        optimizations.reduce(0) { $0 + ($1.valuePerYearUSD ?? 0) }
    }

    func needsYou() -> [NeedsYouItem] {
        var items: [NeedsYouItem] = []
        for optimization in optimizations {
            if optimization.status == .done, let pr = optimization.prNumber {
                var parts = [optimization.title]
                if let change = optimization.change { parts.append(Percent.signed(change)) }
                parts.append("PR #\(pr)")
                items.append(NeedsYouItem(
                    id: "merge-" + optimization.id, kind: .mergeWins,
                    title: "Merge " + Figure.wins(optimization.winsCount),
                    detail: parts.joined(separator: " · "), actionTitle: "Open PR", optimizationID: optimization.id))
            }
        }
        return items
    }


    /// Called by the simulation when an agent starts or finishes a candidate.
    mutating func appendLog(
        agent: Agent, evalsBy: Int, winsBy: Int, activity: String?, events: inout [RatchetEvent], id: String
    ) {
        let text: String
        let kind: String
        if let activity {
            kind = "message"
            text = "Trying: " + activity
        } else if winsBy > 0 {
            kind = "verdict"
            text = "verified: a new best"
        } else if evalsBy > 0 {
            kind = "verdict"
            text = "not a win"
        } else {
            return
        }
        var log = agentLogs[agent.id] ?? []
        log.append(AgentLogLine(id: Int64((log.last?.id ?? 0) + 1), at: now, kind: kind, text: text))
        agentLogs[agent.id] = log
        events.append(.agentLogAppended(optimizationID: id, agentID: agent.id))
    }
}
