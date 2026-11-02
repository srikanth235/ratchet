import AppKit
import RatchetKit
import SwiftUI

extension OptimizationDetailView {
    // MARK: Live

    @ViewBuilder
    func liveBody(_ optimization: Optimization) -> some View {
        HeroRow {
            HeroNumber(
                label: "Best so far", value: optimization.best.map(optimization.metric.text) ?? "—",
                note: bestNote(optimization))
        } second: {
            HeroNumber(
                label: "Spent", value: Money.short(optimization.spentUSD),
                note: optimization.budgetUSD.map { "of " + Money.short($0) + " budget" } ?? "no budget set",
                progress: optimization.budgetFraction)
        } third: {
            HeroNumber(
                label: "Time left", value: optimization.secondsLeft.map(Elapsed.hm) ?? "—",
                note: "of " + String(format: "%g", optimization.timeLimitHours) + " h limit")
        }
        // The agents sit beside the chart when there is room, and under it when there is not.
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: Space.pageX) {
                KitSection("Best so far") { chart(optimization) }
                    .frame(minWidth: Space.section * 12, idealWidth: Space.section * 12, maxWidth: .infinity, alignment: .leading)
                KitSection("Agents") { agentList }
                    .frame(width: Layout.sideColumn, alignment: .leading)
            }
            VStack(alignment: .leading, spacing: Space.block) {
                KitSection("Best so far") { chart(optimization) }
                    .frame(maxWidth: .infinity, alignment: .leading)
                KitSection("Agents") { agentList }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        RunLogSection(optimizationID: id, agents: detail?.agents ?? [])
        KitSection("Candidates") { CandidateTable(optimization: optimization, candidates: detail?.candidates ?? []) }
    }

    func bestNote(_ optimization: Optimization) -> String {
        var parts: [String] = []
        if let change = optimization.change { parts.append(Percent.signed(change)) }
        parts.append("baseline " + optimization.metric.text(optimization.baseline))
        if let noise = optimization.noise { parts.append("noise " + Percent.noise(noise)) }
        return parts.joined(separator: " · ")
    }

    var agentList: some View {
        HairlineList {
            ForEach(detail?.agents ?? []) { agent in
                HairlineRow {
                    StatusDot(tone: agent.state.tone)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(agent.name).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
                        Text(agent.state.title + " · " + agent.activity).kitType(.meta).foregroundStyle(Ink.textMuted).lineLimit(2)
                        Text("Runs on \(agent.placement.title) · " + Plural.count(agent.evals, "eval") + " · " + Plural.count(agent.wins, "win"))
                            .kitType(.caption).foregroundStyle(Ink.textFaint)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    AgentActions(agent: agent)
                }
            }
        }
    }
}
