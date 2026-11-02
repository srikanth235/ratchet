import Foundation
import Testing
@testable import RatchetKit

@Suite("Add connector form")
struct AddConnectorFormTests {
    private let goodToken = "ghp_" + String(repeating: "a", count: 36)
    private let goodKey = "sk-ant-" + String(repeating: "x", count: 40)

    @Test func theGithubDeviceFlowNeedsNothing() {
        let form = AddConnectorForm(kind: .github)
        #expect(form.primaryTitle == "Get a code")
        #expect(form.canSubmit(canStore: false))
        #expect(form.submission(canStore: false) == .githubDeviceFlow)
    }

    @Test func aPastedTokenNeedsTheKeychainAndALookalike() {
        var form = AddConnectorForm(kind: .github, showsToken: true)
        #expect(form.primaryTitle == "Add token")
        #expect(!form.canSubmit(canStore: true) && form.tokenProblem == nil)
        form.token = "not a token"
        #expect(form.tokenProblem != nil && !form.canSubmit(canStore: true))
        form.token = goodToken
        #expect(form.tokenProblem == nil && form.canSubmit(canStore: true))
        #expect(!form.canSubmit(canStore: false))
        #expect(form.submission(canStore: true) == .githubToken(goodToken))
        #expect(form.submission(canStore: false) == nil)
        form.clearSecrets()
        #expect(form.token.isEmpty)
    }

    @Test func anAwsProfileNeedsAPickAndAGoodRegionAndCap() {
        var form = AddConnectorForm(kind: .aws)
        #expect(form.primaryTitle == "Add")
        #expect(!form.canSubmit(canStore: true))
        form.awsProfile = "sandbox-eu"
        #expect(form.canSubmit(canStore: false))
        form.region = " eu-west-1 "
        form.cap = "250"
        guard case .connector(let new)? = form.submission(canStore: false) else {
            Issue.record("no submission")
            return
        }
        #expect(new.kind == .aws && new.name == "sandbox-eu" && new.region == "eu-west-1" && new.monthlyCapUSD == 250)
        form.cap = "free"
        #expect(form.capProblem != nil && !form.canSubmit(canStore: false))
        form.cap = ""
        #expect(form.submission(canStore: false) != nil)
        form.region = ""
        #expect(!form.canSubmit(canStore: false))
    }

    @Test func anAgentKeyNeedsTheKeychainAndTheRightVendorsKey() {
        var form = AddConnectorForm(kind: .agent)
        #expect(form.keyProblem == nil && !form.canSubmit(canStore: true))
        form.apiKey = goodKey
        #expect(form.canSubmit(canStore: true) && !form.canSubmit(canStore: false))
        guard case .connector(let named)? = form.submission(canStore: true) else {
            Issue.record("no submission")
            return
        }
        #expect(named.kind == .agent && named.name == "Claude Code" && named.secret == goodKey)
        form.keyName = " team key "
        guard case .connector(let renamed)? = form.submission(canStore: true) else {
            Issue.record("no submission")
            return
        }
        #expect(renamed.name == "team key")
        // The same key is the wrong key for another vendor.
        form.harness = .codex
        #expect(form.keyProblem != nil && !form.canSubmit(canStore: true))
        form.clearSecrets()
        #expect(form.apiKey.isEmpty)
    }
}
