import Foundation
import Testing
@testable import RatchetKit

@Suite("Optimization search")
struct OptimizationSearchTests {
    private func sample() async throws -> [Optimization] {
        try await MockRatchetClient(now: Date(timeIntervalSince1970: 1_793_452_800)).optimizations()
    }

    @Test func anEmptyQueryKeepsEverythingInOrder() async throws {
        let all = try await sample()
        #expect(OptimizationSearch.filter(all, query: "").map(\.id) == all.map(\.id))
        #expect(OptimizationSearch.filter(all, query: "  \t ").map(\.id) == all.map(\.id))
    }

    @Test func aWordMatchesTheTitleIgnoringCase() async throws {
        let all = try await sample()
        let found = OptimizationSearch.filter(all, query: "CHECKOUT")
        #expect(!found.isEmpty)
        #expect(found.allSatisfy { $0.title.localizedCaseInsensitiveContains("checkout") || $0.goal.localizedCaseInsensitiveContains("checkout") || $0.repo.localizedCaseInsensitiveContains("checkout") })
    }

    @Test func everyWordMustMatchInAnyOrder() async throws {
        let all = try await sample()
        let one = OptimizationSearch.filter(all, query: "checkout")
        let two = OptimizationSearch.filter(all, query: "checkout nothing-like-this")
        #expect(two.isEmpty)
        #expect(OptimizationSearch.filter(all, query: "p95 checkout").map(\.id) == OptimizationSearch.filter(all, query: "checkout p95").map(\.id))
        #expect(one.count >= OptimizationSearch.filter(all, query: "checkout p95").count)
    }

    @Test func theStatusWordMatches() async throws {
        let all = try await sample()
        let merged = OptimizationSearch.filter(all, query: "merged")
        #expect(!merged.isEmpty)
        #expect(merged.allSatisfy { $0.status == .merged || $0.title.localizedCaseInsensitiveContains("merged") })
    }
}
