import Foundation
import Testing
@testable import RatchetKit

@Suite("Daemon version check")
struct DaemonVersionTests {
    @Test func equalVersionsMatch() {
        #expect(DaemonVersionCheck.compare(running: "0.1.0", bundled: "0.1.0") == .matches)
        #expect(DaemonVersionCheck.compare(running: " 0.1.0\n", bundled: "0.1.0") == .matches)
    }

    @Test func differentVersionsAskForARestart() {
        let check = DaemonVersionCheck.compare(running: "0.0.9", bundled: "0.1.0")
        #expect(check == .mismatch(running: "0.0.9", bundled: "0.1.0"))
        #expect(check.needsRestart)
    }

    @Test func anUnreadableVersionNeverRestartsAnything() {
        #expect(DaemonVersionCheck.compare(running: nil, bundled: "0.1.0") == .unknown)
        #expect(DaemonVersionCheck.compare(running: "0.1.0", bundled: "") == .unknown)
        #expect(!DaemonVersionCheck.unknown.needsRestart)
        #expect(!DaemonVersionCheck.matches.needsRestart)
    }

    @Test func theVersionIsTheLastWordOfTheOutput() {
        #expect(DaemonVersionCheck.parse(versionOutput: "ratchetd 0.1.0\n") == "0.1.0")
        #expect(DaemonVersionCheck.parse(versionOutput: "") == nil)
    }
}
