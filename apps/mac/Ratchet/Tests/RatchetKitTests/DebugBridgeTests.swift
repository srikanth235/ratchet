import Foundation
import Testing
@testable import RatchetKit

@Suite("Debug bridge protocol")
struct DebugBridgeTests {
    @Test func commandsParseFromTheirLines() {
        #expect(BridgeCommand(line: "status\n") == .status)
        #expect(BridgeCommand(line: "selftest") == .selftest)
        #expect(BridgeCommand(line: "open:connectors") == .open("connectors"))
        #expect(BridgeCommand(line: "open: opt-checkout ") == .open("opt-checkout"))
        #expect(BridgeCommand(line: "appearance:dark") == .appearance(.dark))
        #expect(BridgeCommand(line: "appearance:light") == .appearance(.light))
        #expect(BridgeCommand(line: "snapshot:menubar") == .snapshot("menubar"))
    }

    @Test func badLinesAreRefused() {
        for line in ["", "open", "open:", "appearance:blue", "appearance:", "snapshot:", "frobnicate:1", "status:1"] {
            #expect(BridgeCommand(line: line) == nil, "\(line)")
        }
    }

    @Test func aCommandWritesTheLineItParsesFrom() {
        let commands: [BridgeCommand] = [.status, .selftest, .open("x"), .appearance(.dark), .snapshot("menubar")]
        for command in commands { #expect(BridgeCommand(line: command.line) == command) }
    }

    @Test func theChannelLivesUnderTheHome() {
        let channel = BridgeChannel(environment: ["RATCHET_HOME": "/tmp/r-home"])
        #expect(channel.inbox.path == "/tmp/r-home/bridge/inbox")
        #expect(channel.replyFile(id: "7").path == "/tmp/r-home/bridge/outbox/7.json")
        #expect(channel.statusFile.path == "/tmp/r-home/bridge/status.json")
        #expect(BridgeChannel(environment: [:]).home.path.hasSuffix("Library/Application Support/Ratchet"))
    }

    @Test func onlyCommandFilesAreCommands() {
        #expect(BridgeChannel.id(ofCommandFile: "12.cmd") == "12")
        #expect(BridgeChannel.id(ofCommandFile: ".12.cmd") == nil)
        #expect(BridgeChannel.id(ofCommandFile: "12.tmp") == nil)
        #expect(BridgeChannel.id(ofCommandFile: ".cmd") == nil)
    }

    @Test func aReplyIsJSONThatDecodesBack() throws {
        let status = BridgeStatus(
            page: "connectors", selectedOptimization: nil, wizardOpen: false, daemonPhase: DaemonPhase.running.bridgeName,
            notificationAuth: "allowed", appearance: "dark", optimizations: 4, running: 1, needsYou: 0, lastError: nil,
            testMode: true, pid: 42)
        let reply = BridgeReply(ok: true, command: "status", message: "ok", status: status)
        let back = try JSONDecoder().decode(BridgeReply.self, from: reply.json())
        #expect(back == reply)
        let object = try #require(JSONSerialization.jsonObject(with: reply.json()) as? [String: Any])
        #expect(object["ok"] as? Bool == true)
    }

    @Test func phasesHaveWordsForTheBridge() {
        #expect(DaemonPhase.needsApproval.bridgeName == "needsApproval")
        #expect(DaemonPhase.failed("boom").bridgeName == "failed")
        #expect(DaemonPhase.failed("boom").failureDetail == "boom")
        #expect(DaemonPhase.running.failureDetail == nil)
    }
}
