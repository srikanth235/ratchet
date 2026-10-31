import Foundation
import Testing
@testable import RatchetKit

/// Talks to a real `ratchetd`. Skipped unless RATCHET_LIVE_HOME names the directory of a running daemon:
///
///     RATCHET_HOME=/tmp/rhome RATCHET_DB=:memory: cargo run -p ratchetd -- serve --tick-ms 1000 &
///     RATCHET_LIVE_HOME=/tmp/rhome swift test --filter LiveDaemon
@Suite("Live daemon", .enabled(if: ProcessInfo.processInfo.environment["RATCHET_LIVE_HOME"] != nil))
struct LiveDaemonTests {
    private func client() -> DaemonRatchetClient {
        let home = ProcessInfo.processInfo.environment["RATCHET_LIVE_HOME"] ?? "/tmp/rhome"
        let location = DaemonLocation.standard(environment: ["RATCHET_HOME": home])
        return DaemonRatchetClient(location: location, fallback: MockRatchetClient(now: Date(), live: false))
    }

    @Test func readsEverythingTheAppShows() async throws {
        let client = client()
        let overview = try await client.overview(range: .days90)
        #expect(overview.series.count >= 2)
        let optimizations = try await client.optimizations()
        #expect(!optimizations.isEmpty)
        #expect(!(try await client.templates()).isEmpty)
        _ = try await client.connectors()
        _ = try await client.benchmarks()
        _ = try await client.sandboxStatus()
        _ = try await client.sandboxReceipt()
        _ = try await client.spend(range: .all)
        for optimization in optimizations {
            let detail = try await client.optimization(id: optimization.id)
            #expect(detail.optimization.id == optimization.id)
            print("LIVE", optimization.title, optimization.status, detail.agents.count, detail.candidates.count, detail.moves.count, detail.filesChanged)
        }
        let missing = try? await client.optimization(id: "nope")
        #expect(missing == nil)
    }

    @Test func eventsArrive() async throws {
        let client = client()
        let stream = await client.events()
        var seen = 0
        let deadline = Date().addingTimeInterval(20)
        for await event in stream {
            print("LIVE EVENT", event)
            seen += 1
            if seen >= 3 || Date() > deadline { break }
        }
        #expect(seen >= 1)
    }

    @Test func actsOnTheDaemon() async throws {
        let client = client()
        // A demo run: created, started, and visible in the table.
        let request = StartRequest(
            repoID: "acme/demo", branch: "main", goal: "Make the search faster", templateID: "demo-search", draftID: nil,
            selection: ConnectionSelection(repoID: "acme/demo"), budgetUSD: 40, timeLimitHours: 2)
        let started = try await client.startOptimization(request)
        print("LIVE started", started)
        #expect(started.isLive || started.status == .waiting)
        #expect(try await client.optimizations().contains { $0.id == started.id })

        // Agents: pause and resume the first one of a running optimization.
        let detail = try await client.optimization(id: started.id)
        if let agent = detail.agents.first {
            let paused = try await client.pauseAgent(id: agent.id)
            #expect(paused.state == .paused)
            let resumed = try await client.resumeAgent(id: agent.id)
            #expect(resumed.state != .paused)
            try await client.messageAgent(id: agent.id, text: "Try caching the tokenizer")
        }

        // Environment draft: drafted by an agent, revised, approved.
        let draft = try await client.createDraft(DraftRequest(repoID: "acme/shop-api", goal: "Make checkout p95 faster"))
        print("LIVE draft", draft)
        #expect(draft.state == .draft && !draft.resources.isEmpty)
        let revised = try await client.reviseDraft(id: draft.id, instruction: "Use a smaller setup")
        #expect(revised.revision > draft.revision)
        let approved = try await client.approveDraft(id: draft.id)
        #expect(approved.state == .approved)

        // Connectors.
        let connectors = try await client.connectors()
        print("LIVE connectors", connectors.map(\.name))
        if let other = connectors.first(where: { $0.kind == .aws && !$0.isDefault }) {
            let list = try await client.setDefaultConnector(id: other.id)
            #expect(list.first { $0.id == other.id }?.isDefault == true)
        }
        if let expired = connectors.first(where: { $0.state == .expired }) {
            let again = try await client.signInConnector(id: expired.id)
            print("LIVE signed in", again)
        }
        var selection = ConnectionSelection(
            repoID: "acme/shop-api", awsID: connectors.first { $0.kind == .aws }?.id,
            githubID: connectors.first { $0.kind == .github && $0.isDefault }?.id)
        if let agent = connectors.first(where: { $0.kind == .agent }) { selection.agents[agent.id] = .local }
        try await client.saveConnectionSelection(selection)
        #expect(try await client.connectionSelection(repoID: "acme/shop-api") == selection)

        // Repos and the harness.
        let repos = try await client.repos()
        print("LIVE repos", repos.count)
        let harness = try await client.detectHarness(repoID: "acme/shop-api")
        print("LIVE harness", harness)

        // The sandbox.
        let sandbox = try await client.sandboxStatus()
        print("LIVE sandbox", sandbox)
    }
}
