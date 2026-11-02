import Foundation
import Testing
@testable import RatchetKit

/// One contract, two clients. Every test here runs the same checks against `MockRatchetClient` (the sample world) and
/// `DaemonRatchetClient` (on a fake daemon that answers with recorded responses), so a rule the mock invents and the daemon
/// does not keep (or the other way round) fails here instead of in the app. The checks are about what any client must keep
/// true, not about particular sample values.
@Suite("Client contract")
struct ClientContractTests {
    private static let now = Date(timeIntervalSince1970: 1_793_452_800)

    private struct Subject {
        var name: String
        var client: any RatchetClient
        /// An optimization with candidates, moves and agents, for the deep checks.
        var richOptimization: String
    }

    private func mock() -> Subject {
        Subject(name: "mock", client: MockRatchetClient(now: Self.now), richOptimization: "opt-forecast")
    }

    private func daemon() -> Subject {
        let fake = FakeDaemon()
        fake.route("GET /v1/optimizations", 200, Fixtures.optimizations)
        fake.route("GET /v1/optimizations/opt_demo_search", 200, Fixtures.detailSearch)
        fake.route("GET /v1/optimizations/opt_demo_ingest", 200, Fixtures.detailIngest)
        fake.route("GET /v1/optimizations/opt_demo_ingest/candidates", 200, Fixtures.candidatesIngest)
        fake.route("GET /v1/optimizations/opt_demo_ingest/ratchet", 200, Fixtures.ratchetIngest)
        fake.route("GET /v1/optimizations/opt_demo_ingest/winning-change", 200, Fixtures.winningIngest)
        fake.route("GET /v1/overview", 200, Fixtures.overview)
        fake.route("GET /v1/connectors", 200, Fixtures.connectors)
        fake.route("GET /v1/environments", 200, Fixtures.environments)
        fake.route("GET /v1/templates", 200, Fixtures.templates)
        return Subject(name: "daemon", client: DaemonRatchetClient(transport: fake), richOptimization: "opt_demo_ingest")
    }

    private var subjects: [Subject] { [mock(), daemon()] }

    // MARK: Optimizations

    @Test func theListHasUniqueIDsAndEveryRowOpens() async throws {
        for subject in subjects {
            let list = try await subject.client.optimizations()
            #expect(!list.isEmpty, "\(subject.name) lists optimizations")
            #expect(Set(list.map(\.id)).count == list.count, "\(subject.name) ids are unique")
            let rich = try #require(list.first { $0.id == subject.richOptimization }, "\(subject.name) knows its rich optimization")
            let detail = try await subject.client.optimization(id: rich.id)
            #expect(detail.optimization.id == rich.id, "\(subject.name) detail is the row")
            #expect(detail.optimization.title == rich.title, "\(subject.name) detail and row agree on the title")
            #expect(detail.optimization.status == rich.status, "\(subject.name) detail and row agree on the status")
        }
    }

    @Test func aFinishedOptimizationIsNotLive() async throws {
        for subject in subjects {
            for optimization in try await subject.client.optimizations() where optimization.status.isFinished {
                #expect(!optimization.isLive, "\(subject.name) \(optimization.id)")
                #expect(optimization.secondsLeft == nil, "\(subject.name) \(optimization.id) has no clock left")
            }
        }
    }

    @Test func theDetailAgreesWithTheSeparateReads() async throws {
        for subject in subjects {
            let id = subject.richOptimization
            let detail = try await subject.client.optimization(id: id)
            let candidates = try await subject.client.candidates(optimizationID: id)
            #expect(Set(candidates.map(\.id)) == Set(detail.candidates.map(\.id)), "\(subject.name) candidates")
            let agents = try await subject.client.agents(optimizationID: id)
            #expect(Set(agents.map(\.id)) == Set(detail.agents.map(\.id)), "\(subject.name) agents")
            let moves = try await subject.client.ratchetHistory(optimizationID: id)
            #expect(moves.map(\.id) == detail.moves.map(\.id), "\(subject.name) moves")
            // Moves climb in time.
            #expect(moves == moves.sorted { $0.at < $1.at }, "\(subject.name) moves are in order")
        }
    }

    @Test func anUnknownOptimizationIsAnErrorOfTheProtocol() async throws {
        for subject in subjects {
            let client = subject.client
            await #expect(throws: RatchetClientError.self, "\(subject.name) detail") { try await client.optimization(id: "nope") }
            await #expect(throws: RatchetClientError.self, "\(subject.name) stop") { try await client.stopOptimization(id: "nope") }
            await #expect(throws: RatchetClientError.self, "\(subject.name) pause") { _ = try await client.pauseAgent(id: "nope") }
            await #expect(throws: RatchetClientError.self, "\(subject.name) resume") { _ = try await client.resumeAgent(id: "nope") }
        }
    }

    // MARK: Overview

    @Test func theOverviewIsInOrderAndItsNeedsAreUnique() async throws {
        for subject in subjects {
            let overview = try await subject.client.overview(range: .days90)
            #expect(overview.range == .days90, "\(subject.name) range")
            #expect(overview.budgetUSD >= 0 && overview.spentUSD >= 0, "\(subject.name) money is not negative")
            #expect(overview.series.map(\.date) == overview.series.map(\.date).sorted(), "\(subject.name) series is in time order")
            #expect(Set(overview.needsYou.map(\.id)).count == overview.needsYou.count, "\(subject.name) needs-you ids are unique")
        }
    }

    // MARK: Connectors and the sandbox

    @Test func eachKindHasAtMostOneDefault() async throws {
        for subject in subjects {
            let list = try await subject.client.connectors()
            #expect(Set(list.map(\.id)).count == list.count, "\(subject.name) connector ids are unique")
            for kind in [ConnectorKind.github, .aws, .agent] {
                let defaults = list.filter { $0.kind == kind && $0.isDefault }
                #expect(defaults.count <= 1, "\(subject.name) \(kind) defaults")
            }
        }
    }

    @Test func aSandboxThatIsOnNamesItsAccount() async throws {
        for subject in subjects {
            let status = try await subject.client.sandboxStatus()
            if status.isOn { #expect(status.account != nil, "\(subject.name) account") }
            let receipt = try await subject.client.sandboxReceipt()
            if let receipt { #expect(receipt.endedAt >= receipt.startedAt, "\(subject.name) receipt dates") }
        }
    }

    @Test func templatesHaveUniqueIDs() async throws {
        for subject in subjects {
            let templates = try await subject.client.templates()
            #expect(!templates.isEmpty, "\(subject.name) templates")
            #expect(Set(templates.map(\.id)).count == templates.count, "\(subject.name) template ids are unique")
        }
    }

    // MARK: Events

    @Test(.timeLimit(.minutes(1))) func anEventStreamCanBeAbandoned() async throws {
        for subject in subjects {
            let client = subject.client
            let reader = Task {
                for await _ in await client.events() { break }
            }
            try await Task.sleep(for: .milliseconds(150))
            reader.cancel()
            await reader.value
        }
    }
}
