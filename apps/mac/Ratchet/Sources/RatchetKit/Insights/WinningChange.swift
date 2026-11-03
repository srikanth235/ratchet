import Foundation

public enum DiffLineKind: String, Hashable, Sendable {
    case add, del, ctx, hunk
}

public struct DiffLine: Hashable, Sendable {
    public var kind: DiffLineKind
    public var text: String

    public init(kind: DiffLineKind, text: String) {
        self.kind = kind
        self.text = text
    }
}

public struct DiffFile: Identifiable, Hashable, Sendable {
    public var path: String
    public var additions: Int
    public var deletions: Int
    public var binary: Bool

    public var id: String { path }

    public init(path: String, additions: Int, deletions: Int, binary: Bool) {
        self.path = path
        self.additions = additions
        self.deletions = deletions
        self.binary = binary
    }
}

/// The squashed result as the pull request will show it: the best verified commit against the base branch.
public struct WinningChange: Hashable, Sendable {
    public var commits: Int
    public var sha: String?
    public var baseRef: String
    public var files: [DiffFile]
    public var additions: Int
    public var deletions: Int
    /// The first hunk of the most-changed file.
    public var excerptPath: String?
    public var excerpt: [DiffLine]
    /// Why there is no diff (no win yet, no git access, git failed); nil when `files` is real.
    public var unavailable: String?

    public init(
        commits: Int, sha: String?, baseRef: String, files: [DiffFile] = [], additions: Int = 0, deletions: Int = 0,
        excerptPath: String? = nil, excerpt: [DiffLine] = [], unavailable: String? = nil
    ) {
        self.commits = commits
        self.sha = sha
        self.baseRef = baseRef
        self.files = files
        self.additions = additions
        self.deletions = deletions
        self.excerptPath = excerptPath
        self.excerpt = excerpt
        self.unavailable = unavailable
    }

    /// "5 verified commits, squashed · a91f3c2 vs main", or "nothing to squash yet".
    public var headline: String {
        var text = commits > 0 ? Plural.count(commits, "verified commit") + ", squashed" : "nothing to squash yet"
        if let sha { text += " · \(sha) vs \(baseRef)" }
        return text
    }

    /// "2 files · +40 −2".
    public var statLine: String {
        Plural.count(files.count, "file") + " · +\(additions) −\(deletions)"
    }
}

extension DiffLine {
    /// How a line reads in the excerpt: a hunk header keeps its `@@ … @@` and names the file, the rest get a "+", "-" or " ".
    public func rendered(path: String) -> String {
        switch kind {
        case .hunk:
            // "@@ -1,3 +1,4 @@ function name" keeps only the ranges.
            if text.hasPrefix("@@"), let close = text.dropFirst(2).range(of: "@@") {
                return String(text[text.startIndex..<close.upperBound]) + " " + path
            }
            return text + " " + path
        case .add: return "+ " + text
        case .del: return "- " + text
        case .ctx: return "  " + text
        }
    }
}
