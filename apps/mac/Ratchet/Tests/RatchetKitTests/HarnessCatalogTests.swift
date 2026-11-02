import Foundation
import Testing
@testable import RatchetKit

@Suite("Harness catalog")
struct HarnessCatalogTests {
    private func model(_ id: String, _ title: String, _ harness: Harness) -> HarnessModel {
        HarnessModel(id: id, title: title, harness: harness)
    }

    private var catalog: HarnessCatalog {
        HarnessCatalog([
            DetectedHarness(harness: .claudeCode, installed: true, version: "2.1", auth: .signedOut),
            DetectedHarness(
                harness: .codex, installed: true, version: "0.166", auth: .apiKey,
                models: [model("gpt-5", "GPT-5", .codex), model("gpt-5-mini", "GPT-5 mini", .codex)], defaultModel: "gpt-5-mini"),
            DetectedHarness(harness: .grok, installed: false),
        ])
    }

    @Test func readyMeansInstalledAndSignedIn() {
        #expect(catalog.isReady(.codex))
        #expect(!catalog.isReady(.claudeCode))
        #expect(!catalog.isReady(.grok))
        #expect(catalog.ready == [.codex])
    }

    @Test func theDefaultIsTheFirstReadyHarnessWithItsDefaultModel() {
        let choice = catalog.defaultChoice
        #expect(choice?.harness == .codex)
        #expect(choice?.model?.id == "gpt-5-mini")
        #expect(choice?.title == "Codex · GPT-5 mini")
    }

    @Test func aHarnessWithoutModelsHasNoModelInItsChoice() {
        let choice = HarnessCatalog([DetectedHarness(harness: .grok, installed: true, auth: .signedIn)]).choice(for: .grok)
        #expect(choice.model == nil)
        #expect(choice.title == "Grok")
    }

    @Test func theEmptyCatalogHasNoDefault() {
        #expect(HarnessCatalog.empty.defaultChoice == nil)
        #expect(HarnessCatalog.empty.choices.isEmpty)
    }

    @Test func orderIsClaudeCodeThenCodexThenGrok() {
        let all = HarnessCatalog(Harness.allCases.map { DetectedHarness(harness: $0, installed: true, auth: .signedIn) })
        #expect(all.ready == [.claudeCode, .codex, .grok])
        #expect(all.choices.map(\.harness) == [.claudeCode, .codex, .grok])
    }
}
