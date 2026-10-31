import Foundation
import HTTPTypes
import OpenAPIRuntime
import Testing
@testable import RatchetKit

/// A stand-in for the daemon: answers by "METHOD /path" (without the query) with canned JSON and records what it was asked.
final class FakeDaemon: ClientTransport, @unchecked Sendable {
    struct Reply {
        var status: Int
        var body: String
    }

    struct Request {
        var key: String
        var body: String
        var headers: HTTPFields
    }

    private let lock = NSLock()
    private var routes: [String: Reply] = [:]
    private var eventStreams: [String] = []
    private var log: [Request] = []
    /// When set, every request fails with this error before it is answered (a daemon that is not there).
    var failure: (any Error)?

    func route(_ key: String, _ status: Int = 200, _ body: String = "") {
        lock.lock()
        routes[key] = Reply(status: status, body: body)
        lock.unlock()
    }

    /// The bodies the next connections to `/v1/events` receive; the last one repeats.
    func streams(_ bodies: [String]) {
        lock.lock()
        eventStreams = bodies
        lock.unlock()
    }

    var requests: [Request] {
        lock.lock()
        defer { lock.unlock() }
        return log
    }

    func count(_ key: String) -> Int {
        requests.filter { $0.key == key }.count
    }

    private func record(_ request: Request) -> (Reply?, String?) {
        lock.withLock {
            log.append(request)
            var stream: String?
            if request.key == "GET /v1/events", !eventStreams.isEmpty {
                stream = eventStreams.count > 1 ? eventStreams.removeFirst() : eventStreams[0]
            }
            return (routes[request.key], stream)
        }
    }

    func send(_ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String) async throws -> (HTTPResponse, HTTPBody?) {
        if let failure { throw failure }
        var text = ""
        if let body {
            for try await chunk in body { text += String(decoding: chunk, as: UTF8.self) }
        }
        let path = request.path ?? ""
        let key = "\(request.method.rawValue) \(path.split(separator: "?").first.map(String.init) ?? path)"
        let (reply, stream) = record(Request(key: key, body: text, headers: request.headerFields))

        if let stream {
            var response = HTTPResponse(status: .ok)
            response.headerFields[.contentType] = "text/event-stream"
            return (response, HTTPBody(stream))
        }
        guard let reply else {
            var response = HTTPResponse(status: .init(code: 404))
            response.headerFields[.contentType] = "application/json"
            return (response, HTTPBody(#"{"code":"not_found","message":"no such route"}"#))
        }
        var response = HTTPResponse(status: .init(code: reply.status))
        response.headerFields[.contentType] = "application/json"
        return (response, reply.body.isEmpty ? nil : HTTPBody(reply.body))
    }
}

private let unsupported = #"{"code":"unsupported","message":"overview is not available in this engine yet (provisional API)"}"#

@Suite("Daemon client")
struct DaemonClientTests {
    private let now = Date(timeIntervalSince1970: 1_793_452_800)

    private func make(_ daemon: FakeDaemon) -> DaemonRatchetClient {
        DaemonRatchetClient(transport: daemon, fallback: MockRatchetClient(now: now, live: false))
    }

    private func daemonWithData() -> FakeDaemon {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/optimizations", 200, Fixtures.optimizations)
        daemon.route("GET /v1/optimizations/opt_demo_search", 200, Fixtures.detailSearch)
        daemon.route("GET /v1/optimizations/opt_demo_ingest", 200, Fixtures.detailIngest)
        daemon.route("GET /v1/optimizations/opt_demo_ingest/candidates", 200, Fixtures.candidatesIngest)
        daemon.route("GET /v1/optimizations/opt_demo_ingest/ratchet", 200, Fixtures.ratchetIngest)
        daemon.route("GET /v1/optimizations/opt_demo_ingest/winning-change", 200, Fixtures.winningIngest)
        daemon.route("GET /v1/overview", 200, Fixtures.overview)
        daemon.route("GET /v1/connectors", 200, Fixtures.connectors)
        daemon.route("GET /v1/environments", 200, Fixtures.environments)
        daemon.route("GET /v1/templates", 200, Fixtures.templates)
        return daemon
    }

    // MARK: Reading

    @Test func theOverviewComesFromTheDaemon() async throws {
        let daemon = daemonWithData()
        let overview = try await make(daemon).overview(range: .days90)
        #expect(overview.budgetUSD == 500)
        #expect(overview.needsYou.count == 3)
        #expect(daemon.requests.first { $0.key == "GET /v1/overview" } != nil)
    }

    @Test func aProvisionalGroupFallsBackToSampleDataQuietly() async throws {
        let daemon = daemonWithData()
        daemon.route("GET /v1/overview", 501, unsupported)
        daemon.route("GET /v1/connectors", 501, unsupported)
        let client = make(daemon)
        let sample = MockRatchetClient(now: now, live: false)
        let overview = try await client.overview(range: .days90)
        #expect(overview == (try await sample.overview(range: .days90)))
        let connectors = try await client.connectors()
        #expect(connectors == (try await sample.connectors()))
        // The daemon is not asked again for a minute.
        _ = try await client.overview(range: .days30)
        _ = try await client.connectors()
        #expect(daemon.count("GET /v1/overview") == 1)
        #expect(daemon.count("GET /v1/connectors") == 1)
    }

    @Test func connectorActionsAlsoFallBack() async throws {
        let daemon = daemonWithData()
        daemon.route("GET /v1/connectors", 501, unsupported)
        daemon.route("POST /v1/connectors", 501, unsupported)
        let client = make(daemon)
        let added = try await client.addConnector(NewConnector(kind: .github, name: "octo"))
        #expect(added.name == "octo")
        #expect(try await client.connectors().contains { $0.id == added.id })
        #expect(try await client.connectionSelection(repoID: "none/none") == nil)
    }

    @Test func theTableGetsTheClockFromTheDetails() async throws {
        let daemon = daemonWithData()
        let list = try await make(daemon).optimizations()
        #expect(list.map(\.id) == ["opt_demo_search", "opt_demo_ingest", "opt_demo_web", "opt_demo_edge"])
        let search = try #require(list.first)
        #expect(search.status == .running)
        #expect((search.secondsLeft ?? 0) > 0)
        #expect(search.startedAt != nil)
        // The two without a detail (the daemon said 404) still show, from the list alone.
        #expect(list[2].status == .waiting)
        #expect(list[3].status == .done)
    }

    @Test func oneOptimizationWithEverythingTheDetailScreenShows() async throws {
        let daemon = daemonWithData()
        let detail = try await make(daemon).optimization(id: "opt_demo_ingest")
        #expect(detail.optimization.id == "opt_demo_ingest")
        #expect(detail.optimization.status == .done)
        #expect(detail.agents.map(\.name) == ["Codex"])
        #expect(detail.candidates.map(\.id) == ["67b05fe", "67b05fd", "67b05fc"])  // newest first
        #expect(detail.moves.count == 2)
        #expect(detail.moves.allSatisfy { $0.at > (detail.optimization.startedAt ?? .distantFuture) })
        #expect(detail.filesChanged == 0)
        #expect(detail.receipt?.id == "sbx-demo-0")
    }

    @Test func spendAddsUpTheOptimizations() async throws {
        let summary = try await make(daemonWithData()).spend(range: .all)
        #expect(summary.totalUSD > 0)
        #expect(summary.lines.count == 3)
        #expect(abs(summary.lines.reduce(0) { $0 + $1.usd } - summary.totalUSD) < 1e-9)
    }

    @Test func benchmarksAreTheOptimizationsThatMeasuredSomething() async throws {
        let runs = try await make(daemonWithData()).benchmarks()
        #expect(runs.count == 3)  // the draft has no baseline
        #expect(runs.first { $0.id == "bench-opt_demo_ingest" }?.title == "Benchmark")
    }

    @Test func theTableAndTheBenchmarksShareOneLoad() async throws {
        let daemon = daemonWithData()
        let client = make(daemon)
        _ = try await client.optimizations()
        _ = try await client.benchmarks()
        #expect(daemon.count("GET /v1/optimizations") == 1)
        #expect(daemon.count("GET /v1/optimizations/opt_demo_search") == 1)
    }

    @MainActor
    @Test func theStoreRefreshesFromTheDaemon() async throws {
        let daemon = daemonWithData()
        daemon.route("GET /v1/github/repos", 409, #"{"code":"not_ready","message":"sign in to GitHub first"}"#)
        let store = AppStore(client: make(daemon))
        await store.refresh()
        #expect(store.lastError == nil)
        #expect(store.isLoaded)
        #expect(store.optimizations.count == 4)
        #expect(store.needsYouCount == 3)
        #expect(store.connectors.count == 8)
        #expect(store.sandbox.isOn)
        #expect(store.benchmarks.count == 3)
        #expect(store.repos.isEmpty)
        await store.loadDetail(id: "opt_demo_ingest")
        #expect(store.details["opt_demo_ingest"]?.moves.count == 2)
    }

    @Test func theSandboxAndItsReceipt() async throws {
        let client = make(daemonWithData())
        let status = try await client.sandboxStatus()
        #expect(status.isOn && status.account == "us-east-1")
        let receipt = try #require(try await client.sandboxReceipt())
        #expect(receipt.id == "sbx-demo-0")
        #expect(receipt.startedAt < receipt.endedAt)  // from the run it covered
    }

    // MARK: Failures

    @Test func failuresAreTranslated() async throws {
        let daemon = daemonWithData()
        daemon.route("POST /v1/agents/a1/pause", 409, #"{"code":"conflict","message":"the agent is not running"}"#)
        let client = make(daemon)
        await #expect(throws: RatchetClientError.notFound("no such route")) {
            _ = try await client.optimization(id: "missing")
        }
        await #expect(throws: RatchetClientError.invalid("the agent is not running")) {
            _ = try await client.pauseAgent(id: "a1")
        }
    }

    @Test func aDaemonThatIsNotThereIsUnavailable() async {
        let daemon = FakeDaemon()
        daemon.failure = UnixSocketError.cannotConnect(path: "/x/ratchetd.sock", code: 2)
        do {
            _ = try await make(daemon).optimizations()
            Issue.record("expected a failure")
        } catch let error as RatchetClientError {
            guard case .unavailable(let why) = error else { Issue.record("wrong error \(error)"); return }
            #expect(why.contains("/x/ratchetd.sock"))
        } catch {
            Issue.record("wrong error \(error)")
        }
    }

    @Test func notSignedInToGitHubMeansNoRepos() async throws {
        let daemon = FakeDaemon()
        daemon.route("GET /v1/github/repos", 409, #"{"code":"not_ready","message":"sign in to GitHub first"}"#)
        daemon.route("POST /v1/repos/inspections", 409, #"{"code":"not_ready","message":"sign in to GitHub first"}"#)
        let client = make(daemon)
        #expect(try await client.repos().isEmpty)
        let harness = try await client.detectHarness(repoID: "acme/x")
        #expect(!harness.found && harness.summary == "sign in to GitHub first")
    }

    @Test func reposAndTemplatesMap() async throws {
        let daemon = daemonWithData()
        daemon.route("GET /v1/github/repos", 200, Fixtures.repos)
        daemon.route("POST /v1/repos/inspections", 200, Fixtures.inspect)
        let client = make(daemon)
        #expect(try await client.repos().first?.id == "acme-org/shop-api")
        #expect(try await client.templates().count > 3)
        #expect(try await client.detectHarness(repoID: "acme/search-api").found)
    }

    // MARK: Acting

    @Test func startingADemoSendsWhatTheWizardChose() async throws {
        let daemon = daemonWithData()
        daemon.route("GET /v1/connectors", 200, Fixtures.connectors)
        daemon.route("POST /v1/optimizations", 201, #"{"id":"opt_demo_search"}"#)
        daemon.route("POST /v1/optimizations/opt_demo_search/start", 204)
        var selection = ConnectionSelection(repoID: "acme/demo", awsID: "conn_aws_prod")
        selection.agents["conn_agent_cc_login"] = .local
        let request = StartRequest(
            repoID: "acme/demo", branch: "main", goal: " Make search faster ", templateID: "demo-search",
            draftID: "mock-draft", selection: selection, budgetUSD: 40, timeLimitHours: 2)
        let started = try await make(daemon).startOptimization(request)
        #expect(started.goal == "Make search faster")
        let sent = try #require(daemon.requests.first { $0.key == "POST /v1/optimizations" })
        let json = try #require(JSONSerialization.jsonObject(with: Data(sent.body.utf8)) as? [String: Any])
        #expect(json["demo"] as? String == "demo-search")
        #expect(json["repoUrl"] == nil)
        #expect(json["budgetUsd"] as? Double == 40)
        #expect(json["timeLimitSeconds"] as? Int == 7200)
        #expect(json["agents"] as? [String] == ["claude-code"])
        #expect(json["agentsWhere"] as? String == "local")
        #expect(json["environmentDraftId"] == nil)  // a sample draft is not the daemon's
        #expect(json["name"] as? String == "Make search faster")
        #expect(daemon.count("POST /v1/optimizations/opt_demo_search/start") == 1)
    }

    @Test func startingARepoRunSendsTheRepo() async throws {
        let daemon = daemonWithData()
        daemon.route("GET /v1/connectors", 200, Fixtures.connectors)
        daemon.route("POST /v1/optimizations", 201, #"{"id":"opt_demo_search"}"#)
        daemon.route("POST /v1/optimizations/opt_demo_search/start", 204)
        let request = StartRequest(
            repoID: "acme/shop-api", branch: "develop", goal: "p95", selection: ConnectionSelection(repoID: "acme/shop-api"),
            budgetUSD: 50, timeLimitHours: 3)
        _ = try await make(daemon).startOptimization(request)
        let sent = try #require(daemon.requests.first { $0.key == "POST /v1/optimizations" })
        let json = try #require(JSONSerialization.jsonObject(with: Data(sent.body.utf8)) as? [String: Any])
        #expect(json["repoUrl"] as? String == "acme/shop-api")
        #expect(json["baseRef"] as? String == "develop")
        #expect(json["demo"] == nil)
    }

    @Test func pausingAnAgentReturnsItAsTheDaemonNowReportsIt() async throws {
        let daemon = daemonWithData()
        daemon.route("POST /v1/agents/opt_demo_search_agt_0/pause", 204)
        daemon.route("POST /v1/agents/opt_demo_search_agt_0/resume", 204)
        daemon.route("POST /v1/agents/opt_demo_search_agt_0/messages", 204)
        let client = make(daemon)
        let agent = try await client.pauseAgent(id: "opt_demo_search_agt_0")
        #expect(agent.id == "opt_demo_search_agt_0")
        _ = try await client.resumeAgent(id: "opt_demo_search_agt_0")
        try await client.messageAgent(id: "opt_demo_search_agt_0", text: "Try caching")
        let sent = try #require(daemon.requests.first { $0.key.hasSuffix("/messages") })
        #expect(sent.body.contains("Try caching"))
    }

    @Test(.timeLimit(.minutes(1))) func aDraftIsAwaitedWhileTheAgentWritesIt() async throws {
        let daemon = daemonWithData()
        let drafting = Fixtures.draft.replacingOccurrences(of: "\"status\":\"ready\"", with: "\"status\":\"drafting\"")
            .replacingOccurrences(of: #"\"draft\":{"#, with: #"\"draft\":null,\"was\":{"#)
        daemon.route("POST /v1/environment-drafts", 201, drafting)
        daemon.route("GET /v1/environment-drafts/env-0001", 200, Fixtures.draft)
        daemon.route("POST /v1/environment-drafts/env-0001/approval", 200, Fixtures.draft.replacingOccurrences(of: "\"status\":\"ready\"", with: "\"status\":\"approved\""))
        let client = make(daemon)
        let draft = try await client.createDraft(DraftRequest(repoID: "acme/shop-api", goal: "Make checkout p95 faster"))
        #expect(draft.state == .draft && draft.resources.count == 4)
        #expect(daemon.count("GET /v1/environment-drafts/env-0001") == 1)
        #expect(try await client.approveDraft(id: "env-0001").state == .approved)
    }

    @Test func aFailedDraftSaysWhy() async {
        let daemon = daemonWithData()
        let failed = Fixtures.draft.replacingOccurrences(of: "\"status\":\"ready\"", with: "\"status\":\"failed\",\"error\":\"claude is not logged in\"")
        daemon.route("POST /v1/environment-drafts", 201, failed)
        await #expect(throws: RatchetClientError.self) {
            _ = try await make(daemon).createDraft(DraftRequest(repoID: "acme/shop-api", goal: "x"))
        }
    }

    // MARK: Events

    private let hello = #"event: hello\#ndata: {"type":"hello","lastEventId":6,"serverTime":1,"version":"0"}\#n\#n"#

    @Test(.timeLimit(.minutes(1))) func eventsBecomeModelsAndReconnectWhereTheyLeftOff() async throws {
        let daemon = daemonWithData()
        let first = hello
            + #"event: optimization\#nid: 7\#ndata: {"type":"optimization","optimizationId":"opt_demo_search","status":"running"}\#n\#n"#
        let second = hello
            + #"event: spend\#nid: 8\#ndata: {"type":"spend","optimizationId":"opt_demo_search","totalUsd":50}\#n\#n"#
        daemon.streams([first, second])
        let stream = await make(daemon).events()
        var events: [RatchetEvent] = []
        var updates = 0
        for await event in stream {
            events.append(event)
            if case .optimizationUpdated = event { updates += 1 }
            if updates == 2 { break }
        }
        #expect(events.contains { if case .needsYouChanged(let items) = $0 { items.count == 3 } else { false } })
        #expect(events.contains { if case .agentUpdated(let id, _) = $0 { id == "opt_demo_search" } else { false } })
        let connections = daemon.requests.filter { $0.key == "GET /v1/events" }
        #expect(connections.count >= 2)
        let lastEventID = HTTPField.Name("Last-Event-ID").flatMap { connections[1].headers[$0] }
        #expect(lastEventID == "7")
    }

    @Test(.timeLimit(.minutes(1))) func aResyncEventAsksTheStoreToReload() async throws {
        let daemon = daemonWithData()
        daemon.streams([hello + #"event: resync\#ndata: {"type":"resync","reason":"restarted"}\#n\#n"#])
        let stream = await make(daemon).events()
        for await event in stream {
            #expect(event == .resync)
            break
        }
    }

    @Test(.timeLimit(.minutes(1))) func candidatesRatchetsAndSandboxesAreRefetched() async throws {
        let daemon = daemonWithData()
        let body = hello
            + #"event: candidate\#nid: 1\#ndata: {"type":"candidate","optimizationId":"opt_demo_ingest","candidateId":"cand_2880e44afc9543b3","verdict":"rejected"}\#n\#n"#
            + #"event: ratchet\#nid: 2\#ndata: {"type":"ratchet","optimizationId":"opt_demo_ingest","from":49.8,"to":58.9}\#n\#n"#
            + #"event: sandbox\#nid: 3\#ndata: {"type":"sandbox","sandboxId":"sbx-sim-1","phase":"ready","text":"up"}\#n\#n"#
        daemon.streams([body])
        var seen: [RatchetEvent] = []
        for await event in await make(daemon).events() {
            seen.append(event)
            if case .receiptIssued = event { break }
        }
        let candidate = seen.compactMap { event -> Candidate? in
            if case .candidateUpdated(_, let candidate) = event { candidate } else { nil }
        }
        #expect(candidate.map(\.id) == ["67b05fd"])
        // The moves come before the optimization update, so the store can date the win on the staircase.
        let moveAt = seen.firstIndex { if case .moveAdded = $0 { true } else { false } }
        let updateAt = seen.firstIndex { if case .optimizationUpdated = $0 { true } else { false } }
        #expect(moveAt != nil && updateAt != nil && (moveAt ?? 0) < (updateAt ?? 0))
        #expect(seen.contains { if case .sandboxChanged(let status) = $0 { status.isOn } else { false } })
    }
}
