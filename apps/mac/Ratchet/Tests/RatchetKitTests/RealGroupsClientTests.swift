import Foundation
import Testing
@testable import RatchetKit

/// The groups that used to fall back to sample data (connectors, the GitHub device flow, environment drafts, the
/// overview) now go to the daemon only; these tests pin down what the client sends and what it makes of the answers.
@Suite("Real groups")
struct RealGroupsClientTests {
    private let flowJSON = #"{"state":"pending","userCode":"WDJB-MJHT","verificationUri":"https://github.com/login/device","expiresAt":1793453700000,"message":null}"#
    private let hello = #"event: hello\#ndata: {"type":"hello","lastEventId":0,"serverTime":1,"version":"0"}\#n\#n"#

    private func connectorsJSON(flow: Bool) -> String {
        let tail = #","encryptionAvailable":true,"clientIdConfigured":true}"#
        return #"{"connectors":[],"githubFlow":"# + (flow ? flowJSON : "null") + tail
    }

    private func make(_ daemon: FakeDaemon) -> DaemonRatchetClient { DaemonRatchetClient(transport: daemon) }

    @Test func theGithubDeviceFlowIsReadFromTheConnectors() async throws {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/connectors", 200, connectorsJSON(flow: true))
        let flow = try #require(try await make(daemon).githubSignInFlow())
        #expect(flow.state == .pending)
        #expect(flow.userCode == "WDJB-MJHT")
        #expect(flow.verificationURL == "https://github.com/login/device")
        #expect(flow.expiresAt == Date(timeIntervalSince1970: 1_793_453_700))
        daemon.route("GET /v1/connectors", 200, connectorsJSON(flow: false))
        #expect(try await make(daemon).githubSignInFlow() == nil)
    }

    @Test func signingInAgainNamesTheConnector() async throws {
        let daemon = FakeDaemon()
        daemon.route("POST /v1/connectors/github/sign-in", 200, connectorsJSON(flow: true))
        let flow = try await make(daemon).startGithubSignIn(connectorID: "github-f07d9ca7")
        #expect(flow?.userCode == "WDJB-MJHT")
        let sent = try #require(daemon.requests.first { $0.key == "POST /v1/connectors/github/sign-in" })
        #expect(sent.body.contains("github-f07d9ca7"))
    }

    @Test func cancellingTheFlowCallsTheDaemon() async throws {
        let daemon = FakeDaemon()
        daemon.route("DELETE /v1/connectors/github/sign-in", 200, connectorsJSON(flow: false))
        try await make(daemon).cancelGithubSignIn()
        #expect(daemon.count("DELETE /v1/connectors/github/sign-in") == 1)
    }

    @Test func anAgentKeyGoesToTheDaemonOnceAndOnlyAsApiKey() async throws {
        let daemon = FakeDaemon()
        daemon.route("POST /v1/connectors/agents", 200, Fixtures.connectors)
        _ = try await make(daemon).addConnector(NewConnector(kind: .agent, name: "Claude Code", secret: "  sk-ant-test  "))
        let sent = try #require(daemon.requests.first { $0.key == "POST /v1/connectors/agents" })
        let json = try #require(JSONSerialization.jsonObject(with: Data(sent.body.utf8)) as? [String: Any])
        #expect(json["source"] as? String == "api-key")
        #expect(json["apiKey"] as? String == "sk-ant-test")
        #expect(json["agent"] as? String == "claude-code")
    }

    @Test func anAgentWithoutAKeyAdoptsTheCliLogin() async throws {
        let daemon = FakeDaemon()
        daemon.route("POST /v1/connectors/agents", 200, Fixtures.connectors)
        _ = try await make(daemon).addConnector(NewConnector(kind: .agent, name: "Codex", secret: "   "))
        let sent = try #require(daemon.requests.first { $0.key == "POST /v1/connectors/agents" })
        let json = try #require(JSONSerialization.jsonObject(with: Data(sent.body.utf8)) as? [String: Any])
        #expect(json["source"] as? String == "login")
        #expect(json["apiKey"] == nil)
        #expect(json["agent"] as? String == "codex")
    }

    @Test func aGrokAgentWithoutAKeyAdoptsTheCliLogin() async throws {
        let daemon = FakeDaemon()
        daemon.route("POST /v1/connectors/agents", 200, Fixtures.connectors)
        _ = try await make(daemon).addConnector(NewConnector(kind: .agent, name: "Grok", secret: nil))
        let sent = try #require(daemon.requests.first { $0.key == "POST /v1/connectors/agents" })
        let json = try #require(JSONSerialization.jsonObject(with: Data(sent.body.utf8)) as? [String: Any])
        #expect(json["source"] as? String == "login")
        #expect(json["agent"] as? String == "grok")
    }

    @Test func aSecretIsNeverEncodedOrCached() throws {
        let data = try JSONEncoder().encode(NewConnector(kind: .agent, name: "Claude Code", secret: "sk-ant-test"))
        #expect(!String(decoding: data, as: UTF8.self).contains("sk-ant"))
        let back = try JSONDecoder().decode(NewConnector.self, from: data)
        #expect(back.secret == nil)
    }

    @Test func aReadyDraftEventBecomesANotificationEvent() async throws {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/environment-drafts/env-0001", 200, Fixtures.draft)
        daemon.route("GET /v1/overview", 200, Fixtures.overview)
        daemon.streams([hello + #"event: environment-draft\#nid: 1\#ndata: {"type":"environment-draft","draftId":"env-0001","status":"ready"}\#n\#n"#])
        var found: RatchetEvent?
        for await event in await make(daemon).events() {
            if case .environmentDraftReady = event {
                found = event
                break
            }
        }
        #expect(found == .environmentDraftReady(draftID: "env-0001", repoID: "acme/shop-api"))
    }

    @Test func aDraftThatIsStillBeingMadeIsNotAnnounced() async throws {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/overview", 200, Fixtures.overview)
        daemon.streams([
            hello + #"event: environment-draft\#nid: 1\#ndata: {"type":"environment-draft","draftId":"env-0001","status":"drafting"}\#n\#n"#
                + #"event: resync\#ndata: {"type":"resync","reason":"x"}\#n\#n"#
        ])
        var seen: [RatchetEvent] = []
        for await event in await make(daemon).events() {
            seen.append(event)
            if event == .resync { break }
        }
        #expect(!seen.contains { if case .environmentDraftReady = $0 { true } else { false } })
        #expect(daemon.count("GET /v1/environment-drafts/env-0001") == 0)
    }

    @Test func aConnectorEventCarriesTheFlowAlong() async throws {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/connectors", 200, connectorsJSON(flow: true))
        daemon.route("GET /v1/overview", 200, Fixtures.overview)
        daemon.streams([hello + #"event: connector\#nid: 1\#ndata: {"type":"connector","connectorId":"github-1"}\#n\#n"#])
        var flows: [GithubSignInFlow?] = []
        for await event in await make(daemon).events() {
            if case .githubFlowChanged(let flow) = event {
                flows.append(flow)
                break
            }
        }
        #expect(flows.first??.userCode == "WDJB-MJHT")
    }
}

@MainActor
@Suite("Store notifications")
struct StoreNotificationTests {
    @Test func theStoreShowsTheFlowAndOnlyNotifiesWhenLoaded() async {
        let store = AppStore(client: MockRatchetClient(now: testNow))
        var posted: [NotificationRequest] = []
        store.onNotification = { posted.append($0) }
        // Not loaded yet: nothing is news.
        store.apply(.environmentDraftReady(draftID: "d0", repoID: "acme/shop"))
        #expect(posted.isEmpty)
        await store.refresh()
        store.apply(.githubFlowChanged(GithubSignInFlow(
            state: .pending, userCode: "ABCD-EFGH", verificationURL: "https://github.com/login/device", expiresAt: Date())))
        #expect(store.githubFlow?.userCode == "ABCD-EFGH")
        store.apply(.environmentDraftReady(draftID: "d1", repoID: "acme/shop"))
        #expect(posted.map(\.id) == ["draft-d1"])
        // A switched-off category posts nothing.
        store.enabledNotifications = { [] }
        store.apply(.environmentDraftReady(draftID: "d2", repoID: "acme/shop"))
        #expect(posted.count == 1)
    }

    @Test func aWinThatArrivesWhileLoadedNotifies() async {
        let store = AppStore(client: MockRatchetClient(now: testNow))
        await store.refresh()
        var posted: [NotificationRequest] = []
        store.onNotification = { posted.append($0) }
        let item = NeedsYouItem(id: "ship-1", kind: .mergeWins, title: "3 wins ready", detail: "d", actionTitle: "Open", optimizationID: "opt-checkout")
        store.apply(.needsYouChanged(store.needsYou + [item]))
        #expect(posted.map(\.category) == [.winReady])
        #expect(posted.first?.destination == .optimization("opt-checkout"))
        // The same list again is not news.
        store.apply(.needsYouChanged(store.needsYou))
        #expect(posted.count == 1)
    }

    @Test func useSourceForgetsOneSourcesDataBeforeLoadingTheNext() async {
        let store = AppStore(client: MockRatchetClient(now: testNow))
        await store.refresh()
        #expect(!store.optimizations.isEmpty)
        await store.useSource(isDemoData: true, cacheURL: nil)
        #expect(store.isDemoData)
        #expect(store.isLoaded)
    }
}
