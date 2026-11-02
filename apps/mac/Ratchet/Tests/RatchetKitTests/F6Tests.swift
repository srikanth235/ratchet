import Foundation
import Testing
@testable import RatchetKit

private let epoch = Date(timeIntervalSince1970: 1_793_452_800)

private func line(_ id: Int64, _ seconds: Double, _ kind: String, _ text: String) -> AgentLogLine {
    AgentLogLine(id: id, at: epoch.addingTimeInterval(seconds), kind: kind, text: text)
}

@Suite("F6: the run's Overview is one timeline")
struct RunTimelineTests {
    @Test func anEventTwoAgentsReportedIsShownOnce() {
        let shared = [line(1, 0, "status", "online · reading the repo"), line(2, 60, "verdict", "verified: 7c02e9d at 351 ms")]
        let entries = RunTimeline.overview(agents: [
            (name: "Claude Code · your login", lines: shared),
            (name: "Claude Code · team key", lines: shared.map { AgentLogLine(id: $0.id, at: $0.at.addingTimeInterval(1), kind: $0.kind, text: $0.text) }),
        ])
        #expect(entries.count == 2)
        // Both agents said it, so it belongs to the run: no agent name on it.
        if case .line(_, _, let agent, _, _) = entries[0] { #expect(agent == nil) } else { Issue.record("expected a line") }
    }

    @Test func aLineOnlyOneAgentSaidKeepsItsName() {
        let entries = RunTimeline.overview(agents: [
            (name: "Claude Code · your login", lines: [line(1, 0, "message", "Trying: Batch the cart price lookups")]),
            (name: "Claude Code · team key", lines: [line(1, 30, "message", "Trying: Compress the order payload")]),
        ])
        #expect(entries.count == 2)
        for entry in entries { if case .line(_, _, let agent, _, _) = entry { #expect(agent != nil) } }
    }

    @Test func aSingleAgentIsNeverNamed() {
        let entries = RunTimeline.overview(agents: [(name: "Claude Code", lines: [line(1, 0, "message", "Looking for the hot path.")])])
        if case .line(_, _, let agent, _, let isMessage) = entries[0] { #expect(agent == nil && isMessage) } else { Issue.record("expected a line") }
    }

    @Test func verdictsBecomeCardsAndNoiseStaysAQuietLine() {
        let entries = RunTimeline.overview(agents: [(name: "A", lines: [
            line(1, 0, "verdict", "verified: 7c02e9d at 351 ms"),
            line(2, 10, "verdict", "within noise: 19f4aa3, not a win"),
            line(3, 20, "verdict", "blocked: protected path changed in b20e6d9"),
            line(4, 30, "error", "the sandbox lost its network"),
            line(5, 40, "tool", "git commit -m x"),
        ])])
        #expect(entries.count == 4)
        if case .verified(_, _, _, let text) = entries[0] { #expect(text == "7c02e9d at 351 ms") } else { Issue.record("verified") }
        if case .noise = entries[1] {} else { Issue.record("noise") }
        if case .blocked(_, _, _, let text) = entries[2] { #expect(text == "Protected path changed in b20e6d9") } else { Issue.record("blocked") }
        if case .failure = entries[3] {} else { Issue.record("failure") }
    }

    @Test func theHarnessChangesAreMarkedAndEntriesComeInTimeOrder() {
        let entries = RunTimeline.overview(agents: [
            (name: "A", lines: [line(2, 100, "message", "later"), line(1, 50, "status", "paused: every agent is signed out; checking again in 5 min")]),
        ])
        #expect(entries.map(\.at) == entries.map(\.at).sorted())
        if case .harness = entries[0] {} else { Issue.record("harness event first") }
    }

    @Test func theSampleRunReadsOncePerEvent() async throws {
        let client = MockRatchetClient()
        let detail = try await client.optimization(id: "opt-checkout")
        var logs: [(name: String, lines: [AgentLogLine])] = []
        for agent in detail.agents {
            let page = try await client.agentLog(agentID: agent.id, after: nil, limit: 200)
            logs.append((name: agent.name, lines: page.lines))
        }
        let entries = RunTimeline.overview(agents: logs)
        let events = entries.compactMap { entry -> (String, Date)? in
            switch entry {
            case .line(_, let at, _, let text, _), .verified(_, let at, _, let text), .blocked(_, let at, _, let text),
                .noise(_, let at, _, let text), .failure(_, let at, _, let text):
                (text, at)
            case .harness: nil
            }
        }
        var twice = 0
        for (index, first) in events.enumerated() {
            for second in events[(index + 1)...] where first.0 == second.0 && abs(first.1.timeIntervalSince(second.1)) <= RunTimeline.sameEventWindow {
                twice += 1
            }
        }
        #expect(twice == 0, "no event twice")
        #expect(entries.contains { if case .verified = $0 { true } else { false } })
    }
}

@MainActor
@Suite("F6: replies, drafts")
struct F6ModelTests {
    @Test func aDefaultModelIsNamedFromTheCatalog() {
        let catalog = HarnessCatalog([
            DetectedHarness(
                harness: .claudeCode, installed: true, auth: .signedIn, models: [HarnessModel(id: "opus", title: "Opus", harness: .claudeCode)],
                defaultModel: "opus")
        ])
        let message = ChatMessage(seq: 1, role: .assistant, text: "x", at: epoch, harness: .claudeCode, model: "default")
        #expect(ReplyLabel(message, catalog: catalog)?.text == "Written by Claude Code · Opus")
    }

    @Test func theFootnoteLinkOpensTheOptimizationWithAPullRequest() async throws {
        let client = MockRatchetClient(now: epoch)
        let all = try await client.optimizations()
        let target = try #require(ValueStoryModel.valueRowTarget(in: all))
        #expect(target.prNumber != nil)
        #expect(ValueStoryModel.valueRowTarget(in: []) == nil)
    }

    private func settle(_ condition: @MainActor () -> Bool) async {
        for _ in 0..<400 {
            if condition() { return }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    @Test func everyDraftSaysItsRepositoryAndItsCloud() async throws {
        let store = ChatStore(client: MockChatClient(now: epoch), coalescing: .zero, pause: { _ in })
        let id = try #require(await store.start(NewChat(repo: "acme/shop")))
        let follow = Task { await store.follow(id) }
        let card = DraftCardModel(store: store, chatID: id)
        card.cloudAccounts = ["acme-prod", "personal-dev"]
        await store.send(id, text: "Make checkout p95 faster")
        await settle { card.card != nil && !card.isDrafting }
        follow.cancel()
        let keys = card.rows.map(\.row.key)
        // Agents, then Repository, then Cloud, as in the prototype.
        #expect(keys.firstIndex(of: "repo") == (keys.firstIndex(of: "agents") ?? -2) + 1)
        #expect(keys.firstIndex(of: "cloud") == (keys.firstIndex(of: "repo") ?? -2) + 1)
        let repo = try #require(card.rows.first { $0.row.key == "repo" })
        #expect(repo.row.value == "acme/shop" && !repo.isEditable)
        let cloud = try #require(card.rows.first { $0.row.key == "cloud" })
        #expect(cloud.row.value == "None · agents run on this Mac" && cloud.isEditable && cloud.changeTitle == "Change cloud")
        card.cloudAccount = "acme-prod"
        #expect(card.rows.first { $0.row.key == "cloud" }?.row.value == "acme-prod · AWS")
        // The cloud row opens the same one-choice popover as any option row.
        if case .option(_, let option)? = card.editor(for: "cloud") { #expect(option.label == "acme-prod") } else { Issue.record("an option editor") }
    }

    @Test func aQuestionAboutARunIsAnsweredAndAnInstructionBecomesANote() async throws {
        let store = ChatStore(client: MockChatClient(now: epoch), coalescing: .zero, pause: { _ in })
        let id = try #require(await store.start(NewChat(optimization: "opt-checkout", harness: .claudeCode)))
        let follow = Task { await store.follow(id) }
        await store.send(id, text: "Why was the last candidate blocked?")
        await settle { store.session(id)?.isReplying == false && store.session(id)?.messages.contains { $0.role == .assistant } == true }
        #expect(store.session(id)?.cards.contains { $0.kind == .note } == false)
        await store.send(id, text: "focus on database queries")
        await settle { store.session(id)?.cards.contains { $0.kind == .note } == true }
        follow.cancel()
        #expect(store.session(id)?.cards.contains { $0.kind == .note } == true)
    }
}
