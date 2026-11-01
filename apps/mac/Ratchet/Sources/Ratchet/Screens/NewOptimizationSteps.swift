import AppKit
import RatchetKit
import SwiftUI

/// Three cards to a row, as in the design.
private let cardColumns = Array(repeating: GridItem(.flexible(), spacing: Dim.cardGap, alignment: .top), count: 3)

// MARK: - Step 1: goal and repo

struct GoalStep: View {
    @Bindable var flow: NewOptimizationFlow
    @Environment(AppRouter.self) private var router
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        VStack(alignment: .leading, spacing: Space.block) {
            KitSection("What should get better?") {
                TextField("Make checkout p95 faster", text: $flow.goal)
                    .kitField()
                    .accessibilityLabel("Goal")
            }
            KitSection("Repo") { repoPicker }
            KitSection("Benchmark") { RepoReview(flow: flow) }
        }
    }

    // MARK: Repo search

    private var repoPicker: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            HStack(spacing: Space.md) {
                TextField(
                    flow.isSignedInToGitHub ? "Search your repositories" : "Sign in with GitHub to search your repositories",
                    text: $flow.repoQuery
                )
                .kitField()
                .disabled(!flow.isSignedInToGitHub)
                .accessibilityLabel("Search repositories")
                Button("Local folder…") { chooseFolder() }
                    .buttonStyle(.kit(.quiet))
            }
            if !flow.isSignedInToGitHub {
                HStack(spacing: Space.xs) {
                    Text("You are not signed in to GitHub.").kitType(.meta).foregroundStyle(Ink.textFaint)
                    Button("Open Connectors") {
                        router.settingsTab = .connectors
                        openSettings()
                    }
                    .buttonStyle(.kit(.link))
                    Text("to sign in, or choose a local folder.").kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
            if let folder = flow.localFolder {
                ChoiceCard(mark: .radio, isSelected: true, action: {}) {
                    Text(flow.inspection?.repo ?? "Local folder").kitType(.bodyStrong).foregroundStyle(Ink.text)
                    Text(folder).kitType(.meta).foregroundStyle(Ink.textFaint).lineLimit(1).truncationMode(.middle)
                }
            }
            LazyVGrid(columns: cardColumns, alignment: .leading, spacing: Dim.cardGap) {
                ForEach(flow.shownRepos) { repo in
                    ChoiceCard(mark: .radio, isSelected: flow.localFolder == nil && flow.repoID == repo.id) {
                        Task { await flow.selectRepo(repo.id) }
                    } content: {
                        HStack(spacing: Space.sm) {
                            Text(repo.name).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
                            if repo.isPrivate == true {
                                Text("Private").kitType(.caption).foregroundStyle(Ink.textFaint)
                            }
                        }
                        Text(repo.account + " · " + repo.defaultBranch).kitType(.meta).foregroundStyle(Ink.textFaint)
                        if let summary = repo.summary, !summary.isEmpty {
                            Text(summary).kitType(.caption).foregroundStyle(Ink.textFaint).lineLimit(1)
                        }
                    }
                }
            }
            if flow.shownRepos.isEmpty && flow.isSignedInToGitHub {
                Text(flow.searchResults == nil ? "No repositories on this account." : "No repository matches.")
                    .kitType(.meta).foregroundStyle(Ink.textFaint)
            }
        }
        .task(id: flow.repoQuery) {
            // Wait for a pause in typing before asking.
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            await flow.search()
        }
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        panel.message = "Choose the folder of the project to optimize"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        Task { await flow.chooseLocalFolder(url.path) }
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
            WhatsMissing(flow: flow)
        }
        .task(id: flow.preflightKey) { await flow.refreshPreflight() }
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
