import AppKit
import Foundation
import Observation
import RatchetKit

/// The `ratchet` command line tool inside the app (`Contents/Helpers/ratchet`), and the links to it.
///
/// "Install command line tool" links it into `~/.local/bin` first: the user's own folder, no password, nothing owned by
/// root. The shell may not look there, so the result says so and gives the one line that adds it to `PATH`. A link in
/// `/usr/local/bin` (for every user of the Mac) is made only when the user asks for it, with the system's administrator
/// prompt. The state of each link (missing, linked, broken, another copy) is `CommandLineLinkState` (RatchetKit).
@MainActor @Observable
final class CommandLineTool {
    static let shared = CommandLineTool()

    nonisolated static var bundled: URL {
        Bundle.main.bundleURL.appending(path: "Contents/Helpers/ratchet")
    }

    static var available: Bool { FileManager.default.isExecutableFile(atPath: bundled.path) }

    nonisolated static var userLink: URL {
        URL(fileURLWithPath: CommandLineInstall.userDirectory(home: NSHomeDirectory())).appending(path: "ratchet")
    }

    nonisolated static let systemLink = URL(fileURLWithPath: CommandLineInstall.systemDirectory).appending(path: "ratchet")

    private(set) var userState: CommandLineLinkState = .missing
    private(set) var systemState: CommandLineLinkState = .missing
    /// Whether the user's shell finds `~/.local/bin`; nil until asked.
    private(set) var userFolderOnPath: Bool?
    /// A sentence about the last install or removal.
    private(set) var note: String?

    private init() { refresh() }

    /// True when a working link exists in either place.
    var isInstalled: Bool { userState.isInstalled || systemState.isInstalled }

    /// A link that exists but does not work (the app moved): worth telling the user.
    var hasBrokenLink: Bool {
        for state in [userState, systemState] { if case .broken = state { return true } }
        return false
    }

    func refresh() {
        userState = Self.state(of: Self.userLink)
        systemState = Self.state(of: Self.systemLink)
        if userState.isInstalled, userFolderOnPath == nil {
            Task {
                let path = await Task.detached { Self.loginShellPath() }.value
                userFolderOnPath = CommandLineInstall.pathContains(CommandLineInstall.userDirectory(home: NSHomeDirectory()), path: path)
            }
        }
    }

    /// What is at `link`, compared with this app's copy. `lstat` sees a link whose target is gone; `stat` does not.
    nonisolated static func state(of link: URL) -> CommandLineLinkState {
        let manager = FileManager.default
        let target = try? manager.destinationOfSymbolicLink(atPath: link.path)
        var info = stat()
        let present = lstat(link.path, &info) == 0
        let isSymlink = target != nil
        let targetExists = target.map { manager.fileExists(atPath: $0) } ?? false
        return .classify(present: present, isSymlink: isSymlink, target: target, targetExists: targetExists, bundled: bundled.path)
    }

    /// Links the tool into `~/.local/bin`. Sets `note`.
    func installForUser() {
        let manager = FileManager.default
        guard userState.canReplaceSilently else {
            note = "\(Self.userLink.path) is a file Ratchet did not put there, so it was left alone."
            return
        }
        do {
            try manager.createDirectory(at: Self.userLink.deletingLastPathComponent(), withIntermediateDirectories: true)
            if (try? manager.destinationOfSymbolicLink(atPath: Self.userLink.path)) != nil { try manager.removeItem(at: Self.userLink) }
            try manager.createSymbolicLink(at: Self.userLink, withDestinationURL: Self.bundled)
            userFolderOnPath = nil
            refresh()
            let hint = CommandLineInstall.pathHint(directory: CommandLineInstall.userDirectory(home: NSHomeDirectory()), home: NSHomeDirectory())
            note = "Installed in ~/.local/bin. If a new Terminal window says \"command not found\", run: \(hint)"
        } catch {
            note = "Could not link it into ~/.local/bin: \(error.localizedDescription)"
        }
    }

    /// Links the tool into `/usr/local/bin` through the system's administrator prompt. Only when the user asks.
    func installForEveryone() {
        let target = Self.bundled.path.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        let source = "do shell script \"mkdir -p /usr/local/bin && ln -sf \" & quoted form of \"\(target)\" & \" /usr/local/bin/ratchet\" with administrator privileges"
        var failure: NSDictionary?
        _ = NSAppleScript(source: source)?.executeAndReturnError(&failure)
        refresh()
        if failure == nil {
            note = "Installed in /usr/local/bin."
        } else if (failure?[NSAppleScript.errorNumber] as? Int) == -128 {
            note = "Cancelled. Nothing was changed."
        } else {
            note = "macOS did not allow it. In Terminal, run: sudo ln -sf \"\(Self.bundled.path)\" /usr/local/bin/ratchet"
        }
    }

    /// Removes the links this app made (only links, never files that are not ours).
    func uninstall() {
        var problems: [String] = []
        for (link, state) in [(Self.userLink, userState), (Self.systemLink, systemState)] where state != .missing {
            guard state != .notALink else { continue }
            do { try FileManager.default.removeItem(at: link) } catch {
                problems.append(link.path == Self.systemLink.path ? "sudo rm \(link.path)" : link.path)
            }
        }
        refresh()
        note = problems.isEmpty ? "Removed." : "Could not remove everything. In Terminal, run: " + problems.joined(separator: "; ")
    }

    /// The `PATH` a login shell sees (the app's own `PATH` is launchd's, not the user's). Gives up after two seconds.
    nonisolated static func loginShellPath() -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/zsh")
        process.arguments = ["-lc", "printf %s \"$PATH\""]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let watchdog = DispatchWorkItem { if process.isRunning { process.terminate() } }
        DispatchQueue.global().asyncAfter(deadline: .now() + 2, execute: watchdog)
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        watchdog.cancel()
        return process.terminationStatus == 0 ? String(decoding: data, as: UTF8.self) : nil
    }
}
