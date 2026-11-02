import Foundation

public struct TemplateProposalRequest: Hashable, Sendable {
    public var repo: String
    /// The branch the pull request targets.
    public var baseRef: String
    public var templateID: String

    public init(repo: String, baseRef: String, templateID: String) {
        self.repo = repo
        self.baseRef = baseRef
        self.templateID = templateID
    }
}

/// The pull request that adds a template to a repo.
public struct TemplateProposal: Hashable, Sendable {
    public var prURL: String
    public var branch: String
    /// Repo paths the commit adds.
    public var files: [String]
    public var templateID: String

    public init(prURL: String, branch: String, files: [String], templateID: String) {
        self.prURL = prURL
        self.branch = branch
        self.files = files
        self.templateID = templateID
    }

    public var url: URL? { URL(string: prURL) }

    /// "Adds ratchet.yaml, bench/k6.js on ratchet/add-template. Merge it, then pick the repository again."
    public var summary: String {
        let list = files.isEmpty ? "the template" : files.joined(separator: ", ")
        return "Adds \(list) on \(branch). Merge it, then choose the repository again."
    }
}
