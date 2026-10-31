import Foundation

/// The `ratchet` command line tool inside the app (`Contents/Helpers/ratchet`), and the optional link to it in
/// `/usr/local/bin`. The link is made only when the user asks (a button in Settings), never on its own.
enum CommandLineTool {
    static var bundled: URL {
        Bundle.main.bundleURL.appending(path: "Contents/Helpers/ratchet")
    }

    static let link = URL(fileURLWithPath: "/usr/local/bin/ratchet")

    static var available: Bool { FileManager.default.isExecutableFile(atPath: bundled.path) }

    /// True when `/usr/local/bin/ratchet` is a link to this app's copy.
    static var installed: Bool {
        guard let target = try? FileManager.default.destinationOfSymbolicLink(atPath: link.path) else { return false }
        return target == bundled.path
    }

    /// Returns a sentence about the result.
    static func install() -> String {
        let manager = FileManager.default
        do {
            try manager.createDirectory(at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
            if (try? manager.destinationOfSymbolicLink(atPath: link.path)) != nil { try manager.removeItem(at: link) }
            try manager.createSymbolicLink(at: link, withDestinationURL: bundled)
            return "Installed. Open a new Terminal window and run ratchet --help."
        } catch {
            // /usr/local/bin is often root's: show the one command that does it.
            return "macOS did not allow it here. In Terminal, run: sudo ln -sf \"\(bundled.path)\" \(link.path)"
        }
    }

    static func uninstall() -> String {
        do {
            if installed { try FileManager.default.removeItem(at: link) }
            return "Removed."
        } catch {
            return "Could not remove it: run sudo rm \(link.path)"
        }
    }
}
