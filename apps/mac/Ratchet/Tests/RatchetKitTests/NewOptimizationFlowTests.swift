import Foundation
import Testing
@testable import RatchetKit

@MainActor
@Suite("New optimization")
struct NewOptimizationFlowTests {
    private func flow() async -> NewOptimizationFlow {
        let flow = NewOptimizationFlow(client: MockRatchetClient(now: testNow))
        await flow.load()
        return flow
    }

    @Test func startsOnTheFirstRepoWithItsHarness() async {
        let flow = await flow()
        #expect(flow.step == .goal)
        #expect(flow.repoID == "acme-org/shop-api")
        #expect(flow.harness?.found == true)
        #expect(flow.harness?.suggestedTemplateID == "http-latency")
        #expect(flow.title == "New optimization")
        #expect(!flow.canContinue)
        #expect(flow.budgetUSD == 50)
        #expect(flow.timeLimitHours == 2)
    }

    @Test func stepsHaveCaptions() {
        #expect(WizardStep.connections.caption == "Step 2 of 4 · Connections")
        #expect(WizardStep.allCases.map(\.title) == ["Goal & repo", "Connections", "Environment", "Budget & time"])
    }

    @Test func aGoalIsNeededToContinue() async {
        let flow = await flow()
        flow.goal = "   "
        #expect(!flow.canContinue)
        flow.goal = "Make checkout p95 faster"
        #expect(flow.canContinue)
        #expect(flow.title == "Make checkout p95 faster")
        #expect(flow.subtitle == "acme-org/shop-api · main")
    }

    @Test func choosingARepoRefreshesTheHarness() async {
        let flow = await flow()
        await flow.selectRepo("acme-org/web-app")
        #expect(flow.harness?.found == false)
        #expect(flow.harness?.suggestedTemplateID == "build-size")
        await flow.selectRepo("srikanth235/notes-sync")
        #expect(flow.branch == "trunk")
    }

    @Test func aTemplateFillsAnEmptyGoal() async {
        let flow = await flow()
        flow.useTemplate("cli-cold-start")
        #expect(flow.templateID == "cli-cold-start")
        #expect(flow.goal == "Improve CLI cold start")
        flow.goal = "Mine"
        flow.useTemplate("http-latency")
        #expect(flow.goal == "Mine")
    }

    @Test func connectionsComeFromWhatThisRepoRemembered() async {
        let flow = await flow()
        flow.goal = "Make checkout p95 faster"
        await flow.next()
        #expect(flow.step == .connections)
        #expect(flow.awsID == "aws-acme-prod")
        #expect(flow.agents == ["agent-claude-login": .local, "agent-claude-team": .cloud])
        #expect(flow.canContinue)
    }

    @Test func aNewRepoGetsTheDefaults() async {
        let flow = await flow()
        await flow.selectRepo("acme-org/web-app")
        flow.goal = "Shrink the bundle"
        await flow.next()
        #expect(flow.awsID == "aws-acme-prod")
        #expect(flow.agents == ["agent-claude-login": .local])
    }

    @Test func expiredOrSignedOutConnectorsCannotBePicked() async {
        let flow = await flow()
        flow.goal = "Go"
        await flow.next()
        flow.selectAWS("aws-acme-staging")
        #expect(flow.awsID == "aws-acme-prod")
        flow.toggleAgent("agent-codex-login")
        #expect(flow.agents["agent-codex-login"] == nil)
        flow.selectAWS("aws-personal-dev")
        #expect(flow.awsID == "aws-personal-dev")
    }

    @Test func agentsToggleAndMoveBetweenThisMacAndTheCloud() async {
        let flow = await flow()
        flow.goal = "Go"
        await flow.next()
        flow.togglePlacement("agent-claude-team")
        #expect(flow.agents["agent-claude-team"] == .local)
        flow.togglePlacement("agent-claude-team")
        #expect(flow.agents["agent-claude-team"] == .cloud)
        flow.togglePlacement("agent-claude-login")
        #expect(flow.agents["agent-claude-login"] == .local)
        flow.toggleAgent("agent-claude-team")
        flow.toggleAgent("agent-claude-login")
        #expect(flow.agents.isEmpty)
        #expect(!flow.canContinue)
    }

    @Test func walkingAllFourStepsStartsARun() async {
        let flow = await flow()
        flow.goal = "Make checkout p95 faster"
        await flow.next()
        await flow.next()
        #expect(flow.step == .environment)
        #expect(flow.draft?.scope == .full)
        #expect(!flow.canContinue)
        await flow.next()
        #expect(flow.step == .environment)
        await flow.approveDraft()
        #expect(flow.canContinue)
        await flow.next()
        #expect(flow.step == .budget)
        #expect(flow.isLastStep)
        #expect(abs(flow.estimatedSandboxUSD - 1.18) < 0.0001)
        let started = await flow.start()
        #expect(started?.status == .running)
        #expect(started?.budgetUSD == 50)
        #expect(started?.timeLimitHours == 2)
        #expect(started?.repo == "acme-org/shop-api")
        #expect(flow.started?.id == started?.id)
    }

    @Test func aRevisionNeedsApprovingAgain() async {
        let flow = await flow()
        flow.goal = "Make checkout p95 faster"
        await flow.go(to: .environment)
        await flow.approveDraft()
        #expect(flow.draft?.state == .approved)
        await flow.sendRevision("use RDS instead of a Postgres container")
        #expect(flow.draft?.state == .draft)
        #expect(flow.draft?.revision == 2)
        #expect(!flow.canContinue)
        await flow.sendRevision("   ")
        #expect(flow.draft?.revision == 2)
    }

    @Test func goingBackKeepsTheChoices() async {
        let flow = await flow()
        flow.goal = "Make checkout p95 faster"
        await flow.go(to: .environment)
        await flow.back()
        #expect(flow.step == .connections)
        await flow.back()
        #expect(flow.step == .goal)
        await flow.back()
        #expect(flow.step == .goal)
        #expect(flow.draft != nil)
        #expect(flow.awsID == "aws-acme-prod")
    }

    @Test func startingNeedsTheLastStepAndValidLimits() async {
        let flow = await flow()
        flow.goal = "Make checkout p95 faster"
        let early = await flow.start()
        #expect(early == nil)
        await flow.go(to: .budget)
        flow.budgetUSD = 0
        #expect(!flow.canContinue)
        let none = await flow.start()
        #expect(none == nil)
    }
}
