import Foundation

/// What is at the place where the `ratchet` command line tool is linked (`~/.local/bin/ratchet`, or
/// `/usr/local/bin/ratchet`), compared with the copy inside this app. Pure, so the stale and broken cases are tested.
public enum CommandLineLinkState: Equatable, Sendable {
    /// Nothing there.
    case missing
    /// A link to this app's copy, which exists.
    case linked
    /// A link whose target is gone (the app moved or was deleted).
    case broken(target: String)
    /// A link to another copy of the app that still exists (an older install, a build in another folder).
    case otherCopy(target: String)
    /// Something that is not a link (a real file, or a script the user put there). Never replaced without asking.
    case notALink

    /// `isSymlink` and `target` come from `lstat` / `readlink` (nil target: not a link); `targetExists` from `stat`.
    public static func classify(
        present: Bool, isSymlink: Bool, target: String?, targetExists: Bool, bundled: String
    ) -> CommandLineLinkState {
        guard present || isSymlink else { return .missing }
        guard isSymlink, let target else { return .notALink }
        if !targetExists { return .broken(target: target) }
        return target == bundled ? .linked : .otherCopy(target: target)
    }

    public var isInstalled: Bool { self == .linked }

    /// True when installing would replace something that is ours to replace (a stale or broken link, or nothing).
    public var canReplaceSilently: Bool {
        switch self {
        case .missing, .linked, .broken, .otherCopy: true
        case .notALink: false
        }
    }

    /// One sentence for Settings.
    public func sentence(directory: String) -> String {
        switch self {
        case .missing: "Not installed"
        case .linked: "Installed in \(directory)"
        case .broken: "The link in \(directory) points to a copy of Ratchet that is gone"
        case .otherCopy: "The link in \(directory) points to another copy of Ratchet"
        case .notALink: "\(directory)/ratchet is a file Ratchet did not put there"
        }
    }
}

/// Where the command line tool goes, and whether the shell will find it there.
public enum CommandLineInstall {
    /// The user's own folder, tried first: no password, no root-owned folder.
    public static func userDirectory(home: String) -> String { home + "/.local/bin" }
    /// For every user of the Mac; writing there needs an administrator, so it is offered only on request.
    public static let systemDirectory = "/usr/local/bin"

    /// True when `directory` is one of the entries of a `PATH` string.
    public static func pathContains(_ directory: String, path: String?) -> Bool {
        guard let path, !path.isEmpty else { return false }
        let wanted = normalized(directory)
        return path.split(separator: ":").contains { normalized(String($0)) == wanted }
    }

    private static func normalized(_ directory: String) -> String {
        var d = directory
        while d.count > 1, d.hasSuffix("/") { d.removeLast() }
        return d
    }

    /// The line to add to the shell profile when the folder is not on `PATH`.
    public static func pathHint(directory: String, home: String) -> String {
        let shown: String = directory.hasPrefix(home) ? "$HOME" + String(directory.dropFirst(home.count)) : directory
        return "echo 'export PATH=\"\(shown):$PATH\"' >> ~/.zshrc"
    }
}
