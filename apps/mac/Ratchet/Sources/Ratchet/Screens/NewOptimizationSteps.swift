import RatchetKit
import SwiftUI

/// Three cards to a row, as in the design.
private let cardColumns = Array(repeating: GridItem(.flexible(), spacing: Dim.cardGap, alignment: .top), count: 3)

// MARK: - Step 1: goal and repo

struct GoalStep: View {
    @Bindable var flow: NewOptimizationFlow

    var body: some View {
        VStack(alignment: .leading, spacing: Space.block) {
            KitSection("What should get better?") {
                TextField("Make checkout p95 faster", text: $flow.goal)
                    .kitField()
                    .accessibilityLabel("Goal")
            }
            KitSection("Repo") {
                LazyVGrid(columns: cardColumns, alignment: .leading, spacing: Dim.cardGap) {
                    ForEach(flow.repos) { repo in
                        ChoiceCard(mark: .radio, isSelected: flow.repoID == repo.id) {
                            Task { await flow.selectRepo(repo.id) }
                        } content: {
                            Text(repo.name).kitType(.bodyStrong).foregroundStyle(Ink.text)
                            Text(repo.account + " · " + repo.defaultBranch).kitType(.meta).foregroundStyle(Ink.textFaint)
                        }
                    }
                }
            }
            KitSection("Benchmark") { harness }
        }
    }

    @ViewBuilder
    private var harness: some View {
        if let detection = flow.harness {
            VStack(alignment: .leading, spacing: Space.sm) {
                HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
                    StatusDot(tone: detection.found ? .ok : .wait)
                    Text(detection.summary).kitType(.body).foregroundStyle(Ink.textMuted)
                }
                if let id = detection.suggestedTemplateID, let template = flow.templates.first(where: { $0.id == id }) {
                    if flow.templateID == id {
                        Text("Starting from the " + template.title + " template.").kitType(.meta).foregroundStyle(Ink.textFaint)
                    } else {
                        let verb = detection.found ? "Use the " : "Start from the "
                        let suffix = detection.found ? " template instead" : " template"
                        Button(verb + template.title + suffix) { flow.useTemplate(id) }
                            .buttonStyle(.kit(.link))
                    }
                }
            }
        } else {
            Text("Choose a repo to look for a benchmark.").kitType(.body).foregroundStyle(Ink.textFaint)
        }
    }
}

// MARK: - Step 2: connections

struct ConnectionsStep: View {
    @Bindable var flow: NewOptimizationFlow

    var body: some View {
        VStack(alignment: .leading, spacing: Space.block) {
            KitSection("AWS account") {
                LazyVGrid(columns: cardColumns, alignment: .leading, spacing: Dim.cardGap) {
                    ForEach(flow.connectors.filter { $0.kind == .aws }) { connector in
                        ChoiceCard(mark: .radio, isSelected: flow.awsID == connector.id, isEnabled: connector.state.isUsable) {
                            flow.selectAWS(connector.id)
                        } content: {
                            Text(connector.name).kitType(.bodyStrong).foregroundStyle(Ink.text)
                            Text(awsLine(connector)).kitType(.meta).foregroundStyle(Ink.textFaint)
                        }
                    }
                }
            }
            KitSection("Agents") {
                LazyVGrid(columns: cardColumns, alignment: .leading, spacing: Dim.cardGap) {
                    ForEach(flow.connectors.filter { $0.kind == .agent }) { connector in
                        ChoiceCard(mark: .checkbox, isSelected: flow.agents[connector.id] != nil, isEnabled: connector.state.isUsable) {
                            flow.toggleAgent(connector.id)
                        } content: {
                            Text(connector.name).kitType(.bodyStrong).foregroundStyle(Ink.text)
                            Text(agentLine(connector)).kitType(.meta).foregroundStyle(Ink.textFaint)
                        } footer: {
                            if let placement = flow.agents[connector.id] {
                                placementLine(connector, placement)
                            }
                        }
                    }
                }
            }
        }
    }

    private func awsLine(_ connector: Connector) -> String {
        guard connector.state.isUsable else { return connector.state.title }
        return connector.leftThisMonthUSD.map { Money.short($0) + " left this month" } ?? connector.detail
    }

    private func agentLine(_ connector: Connector) -> String {
        guard connector.state.isUsable else { return connector.state.title }
        return connector.canRunInCloud ? "Billed to the key" : "Free while this Mac is awake"
    }

    private func placementLine(_ connector: Connector, _ placement: Placement) -> some View {
        HStack(spacing: Space.xs) {
            Text("Runs on").kitType(.caption).foregroundStyle(Ink.textFaint)
            Text(placement.title).kitType(.captionStrong).foregroundStyle(Ink.text)
            if connector.canRunInCloud {
                Text("·").kitType(.caption).foregroundStyle(Ink.textFaint)
                Button("change") { flow.togglePlacement(connector.id) }
                    .buttonStyle(.kit(.link, small: true))
            }
        }
        .kitType(.caption)
    }
}
