import Foundation
import Observation

/// "Use template": choose a repo, then Ratchet opens a pull request that adds the template's files. Nothing changes in
/// the repo until the pull request is merged.
@MainActor @Observable
public final class TemplateProposalFlow: Identifiable {
    public let id = UUID()
    public let template: Template
    public var query = ""
    public private(set) var repos: [Repo] = []
    public private(set) var selected: Repo?
    public private(set) var proposal: TemplateProposal?
    public private(set) var error: String?
    public private(set) var isBusy = false
    public private(set) var isSignedIn = true
    public private(set) var isLoaded = false

    private let client: any RatchetClient

    public init(template: Template, client: any RatchetClient) {
        self.template = template
        self.client = client
    }

    public func load() async {
        do {
            isSignedIn = try await client.connectors().contains { $0.kind == .github && $0.state.isUsable }
            if isSignedIn {
                repos = try await client.searchRepos(query: "")
            } else {
                repos = []
            }
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        isLoaded = true
    }

    /// The repos whose name contains the query (the first 60).
    public var shown: [Repo] {
        let text = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let list = text.isEmpty ? repos : repos.filter { $0.id.lowercased().contains(text) || ($0.summary ?? "").lowercased().contains(text) }
        return Array(list.prefix(60))
    }

    public func select(_ repo: Repo) {
        selected = repo
        proposal = nil
        error = nil
    }

    /// "Target: owner/name (main)", or what to do first.
    public var targetLine: String {
        guard let selected else { return "Choose a repo." }
        return "Target: \(selected.id) (\(selected.defaultBranch))"
    }

    public var canOpen: Bool { selected != nil && !isBusy && proposal == nil && template.kind == .template }

    public func openPullRequest() async {
        guard let selected, canOpen else { return }
        isBusy = true
        error = nil
        defer { isBusy = false }
        do {
            proposal = try await client.proposeTemplate(
                TemplateProposalRequest(repo: selected.id, baseRef: selected.defaultBranch, templateID: template.id))
        } catch {
            self.error = error.localizedDescription
        }
    }
}
