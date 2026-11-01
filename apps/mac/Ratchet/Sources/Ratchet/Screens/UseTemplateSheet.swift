import RatchetKit
import SwiftUI

/// "Use template": pick a repo, then Ratchet opens a pull request that adds the template's files.
struct UseTemplateSheet: View {
    @Bindable var flow: TemplateProposalFlow
    @Environment(\.dismiss) private var dismiss
    @Environment(AppRouter.self) private var router
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            VStack(alignment: .leading, spacing: Space.xs) {
                Text("Use “\(flow.template.title)”").kitType(.pageTitle).foregroundStyle(Ink.text).lineLimit(2)
                Text("Ratchet opens a pull request on the repo that adds the template's files. Nothing changes until you merge it.")
                    .kitType(.body).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
            }
            if flow.isLoaded && !flow.isSignedIn {
                HStack(spacing: Space.xs) {
                    Text("You are not signed in to GitHub.").kitType(.meta).foregroundStyle(Ink.textFaint)
                    Button("Open Connectors") {
                        router.settingsTab = .connectors
                        openSettings()
                    }
                    .buttonStyle(.kit(.link))
                }
            } else {
                TextField("Search your repositories", text: $flow.query)
                    .kitField()
                    .accessibilityLabel("Search repositories for the template")
                list
            }
            if let failure = flow.error {
                Text(failure).kitType(.meta).foregroundStyle(Ink.bad).fixedSize(horizontal: false, vertical: true)
            }
            if let proposal = flow.proposal {
                PullRequestResult(proposal: proposal)
            }
            HStack {
                Text(flow.targetLine).kitType(.meta).foregroundStyle(Ink.textFaint).lineLimit(1)
                Spacer()
                Button(flow.proposal == nil ? "Cancel" : "Done") { dismiss() }
                    .buttonStyle(.kit(.quiet))
                    .keyboardShortcut(.cancelAction)
                if flow.proposal == nil {
                    Button(flow.isBusy ? "Opening the pull request…" : "Open pull request") {
                        Task { await flow.openPullRequest() }
                    }
                    .buttonStyle(.kit(.primary))
                    .keyboardShortcut(.defaultAction)
                    .disabled(!flow.canOpen)
                }
            }
        }
        .padding(Space.xxl)
        .frame(width: 520)
        .background(Ink.canvas)
        .task { await flow.load() }
    }

    private var list: some View {
        ScrollView {
            VStack(spacing: Dim.cardGap) {
                ForEach(flow.shown) { repo in
                    ChoiceCard(mark: .radio, isSelected: flow.selected?.id == repo.id) {
                        flow.select(repo)
                    } content: {
                        Text(repo.id).kitType(.monoStrong).foregroundStyle(Ink.text).lineLimit(1)
                        Text(repo.defaultBranch + (repo.isPrivate == true ? " · private" : ""))
                            .kitType(.meta).foregroundStyle(Ink.textFaint)
                    }
                }
                if flow.shown.isEmpty && flow.isLoaded {
                    Text(flow.repos.isEmpty ? "No repositories on this account." : "No repository matches.")
                        .kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
        }
        .frame(maxHeight: 240)
    }
}
