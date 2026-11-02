import Foundation
import Testing
@testable import RatchetKit

@Suite("Command line tool link")
struct CommandLineLinkTests {
    private let bundled = "/Users/me/Applications/Ratchet.app/Contents/Helpers/ratchet"

    private func state(present: Bool = true, symlink: Bool = true, target: String?, exists: Bool = true) -> CommandLineLinkState {
        .classify(present: present, isSymlink: symlink, target: target, targetExists: exists, bundled: bundled)
    }

    @Test func eachStateIsTold() {
        #expect(state(present: false, symlink: false, target: nil) == .missing)
        #expect(state(target: bundled) == .linked)
        #expect(state(target: bundled, exists: false) == .broken(target: bundled))
        #expect(state(target: "/Applications/Ratchet.app/Contents/Helpers/ratchet") == .otherCopy(target: "/Applications/Ratchet.app/Contents/Helpers/ratchet"))
        #expect(state(symlink: false, target: nil) == .notALink)
    }

    @Test func aBrokenLinkIsPresentEvenThoughItsTargetIsNot() {
        // lstat sees the link; stat does not see through it.
        #expect(state(present: false, symlink: true, target: "/gone", exists: false) == .broken(target: "/gone"))
    }

    @Test func onlyOurOwnLinksAreReplacedWithoutAsking() {
        #expect(CommandLineLinkState.missing.canReplaceSilently)
        #expect(CommandLineLinkState.broken(target: "x").canReplaceSilently)
        #expect(CommandLineLinkState.otherCopy(target: "x").canReplaceSilently)
        #expect(!CommandLineLinkState.notALink.canReplaceSilently)
        #expect(CommandLineLinkState.linked.isInstalled)
        #expect(!CommandLineLinkState.broken(target: "x").isInstalled)
    }

    @Test func thePathIsReadEntryByEntry() {
        let path = "/usr/bin:/Users/me/.local/bin/:/bin"
        #expect(CommandLineInstall.pathContains("/Users/me/.local/bin", path: path))
        #expect(!CommandLineInstall.pathContains("/Users/me/.local", path: path))
        #expect(!CommandLineInstall.pathContains("/Users/me/.local/bin", path: nil))
        #expect(!CommandLineInstall.pathContains("/Users/me/.local/bin", path: ""))
    }

    @Test func theUserFolderComesFirstAndTheHintUsesHome() {
        let dir = CommandLineInstall.userDirectory(home: "/Users/me")
        #expect(dir == "/Users/me/.local/bin")
        #expect(CommandLineInstall.pathHint(directory: dir, home: "/Users/me").contains("$HOME/.local/bin"))
        #expect(CommandLineInstall.systemDirectory == "/usr/local/bin")
    }
}
