import AppKit
import RatchetKit

/// Self-test for agents: runs the app's real store, router and wizard on the sample client (fixed clock), prints PASS
/// or FAIL per check, and quits with exit code 0 or 1.
///   APP_STORE=/tmp/Ratchet-self.store ~/Applications/Ratchet.app/Contents/MacOS/Ratchet --selftest
/// (tools/selftest.sh does this.) Add a check for every story with logic.
@MainActor
enum SelfTest {
    static var requested: Bool { CommandLine.arguments.contains("--selftest") || live }
    /// `--selftest-live`: the same app against a real `ratchetd` (the simulation) instead of the sample client.
    static var live: Bool { CommandLine.arguments.contains("--selftest-live") }
    /// Read by AppDelegate.applicationWillTerminate.
    static var exitCode: Int32 = 0
    private static var failures = 0
    private static var count = 0

    static func runIfRequested(store: AppStore, router: AppRouter, daemon: DaemonController) {
        guard requested else { return }
        // Print each line at once, so a crash keeps the lines before it.
        setvbuf(stdout, nil, _IOLBF, 0)
        if live {
            Task {
                await runLive(store: store, daemon: daemon)
                print(failures == 0 ? "SELFTEST PASS \(count) checks" : "SELFTEST FAIL \(failures) of \(count) checks")
                finish(code: failures == 0 ? 0 : 1)
            }
            return
        }
        guard Store.isOverridden else {
            print("SELFTEST refused: set APP_STORE to a file in /tmp")
            finish(code: 1)
            return
        }
        Task {
            await run(store: store, router: router)
            print(failures == 0 ? "SELFTEST PASS \(count) checks" : "SELFTEST FAIL \(failures) of \(count) checks")
            finish(code: failures == 0 ? 0 : 1)
        }
    }

    private static func run(store: AppStore, router: AppRouter) async {
        // Store: the first load.
        check("the store loaded", store.isLoaded)
        check("test mode runs on demo data", store.isDemoData)
        check("five optimizations", store.optimizations.count == 5)
        check("spent matches the design", store.overview?.spentUSD == 184, "\(String(describing: store.overview?.spentUSD))")
        check("value locked in matches the design", store.overview?.valueLockedPerYearUSD == 15_400)
        check("two things need you", store.needsYouCount == 2)
        check("money formats", Money.short(15_400) == "$15.4k" && Money.short(184) == "$184")
        check("percents format", Percent.signed(-0.301) == "−30%")

        // Range switch.
        await store.setRange(.days30)
        check("30 days has fewer wins", store.overview?.winsCount == 3)
        await store.setRange(.days90)

        // Navigation.
        router.show(.connectors)
        check("Connectors page opens", router.page == .connectors)
        router.open(optimization: "opt-checkout")
        check("an optimization opens in place", router.page == .optimizations && router.optimizationID == "opt-checkout")
        router.show(.optimizations)
        check("choosing Optimizations again goes back to the list", router.optimizationID == nil)
        check("every page has its own Command-number", Set(Page.allCases.map { String($0.shortcutKey.character) }).count == Page.allCases.count)

        // Live events through the store.
        await store.loadDetail(id: "opt-checkout")
        check("the live optimization has agents", store.details["opt-checkout"]?.agents.count == 2)
        if let mock = store.client as? MockRatchetClient {
            let before = store.optimization(id: "opt-checkout")?.spentUSD ?? 0
            // The store follows the event stream by itself; give it a moment to apply what the client emitted.
            for _ in 0..<2 { await mock.advance() }
            try? await Task.sleep(for: .milliseconds(400))
            let after = store.optimization(id: "opt-checkout")
            check("events spend money", (after?.spentUSD ?? 0) > before)
            check("a win becomes the new best", (after?.best ?? 999) < 288 && after?.winsCount == 3)
            check("the overview grows with the win", store.overview?.winsCount == 10)
        } else {
            check("the sample client is in use", false)
        }

        // Agents.
        await store.pauseAgent("agent-claude-login")
        check("pausing an agent", store.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-login" }?.state == .paused)
        await store.resumeAgent("agent-claude-login")
        check("resuming an agent", store.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-login" }?.state == .working)

        // Connectors.
        await store.addConnector(NewConnector(kind: .aws, name: "selftest-account", detail: "SSO · us-east-2"))
        check("adding a connector", store.connectors(of: .aws).contains { $0.name == "selftest-account" })
        if let added = store.connectors(of: .aws).first(where: { $0.name == "selftest-account" }) {
            await store.removeConnector(added.id)
            check("removing a connector", !store.connectors(of: .aws).contains { $0.id == added.id })
        }
        await store.signInConnector("aws-acme-staging")
        check("signing in an expired connector", store.connectors.first { $0.id == "aws-acme-staging" }?.state == .connected)

        // The wizard.
        let flow = NewOptimizationFlow(client: store.client)
        await flow.load()
        check("the wizard starts on Goal & repo", flow.step == .goal && !flow.canContinue)
        flow.goal = "Make checkout p95 faster"
        await flow.next()
        check("Connections remembers this repo", flow.step == .connections && flow.awsID == "aws-acme-prod" && flow.agents.count == 2)
        await flow.next()
        check("Environment drafts the full stack", flow.step == .environment && flow.draft?.scope == .full)
        check("the draft must be approved first", !flow.canContinue)
        await flow.approveDraft()
        await flow.next()
        check("Budget & time defaults to $50 and 2 h", flow.step == .budget && flow.budgetUSD == 50 && flow.timeLimitHours == 2)
        let started = await flow.start()
        check("starting adds a running optimization", started?.status == .running)
        if let started {
            await store.register(started)
            check("the new optimization is on screen", store.optimization(id: started.id) != nil)
        }

        // Sandbox.
        check("the sandbox is on", store.sandbox.isOn)
        await store.endSandbox()
        check("End now tears the sandbox down", !store.sandbox.isOn && store.receipt?.allGone == true)

        // Symbols used in the app exist.
        for name in ["chart.line.uptrend.xyaxis", "gearshape", "link"] {
            check("SF Symbol \(name) exists", NSImage(systemSymbolName: name, accessibilityDescription: nil) != nil)
        }
    }

    /// The real client path end to end: the generated client, the unix socket, the bearer token, the mapping, the event
    /// stream, against `ratchetd serve --mode sim`. Run by tools/smoke-daemon.sh; nothing here touches the sample data.
    private static func runLive(store: AppStore, daemon: DaemonController) async {
        check("the app uses the daemon, not the sample client", !(store.client is MockRatchetClient) && !Store.isOverridden)
        check("the daemon answers on the socket", daemon.phase.isRunning, "\(daemon.phase)")
        check("the store loaded from the daemon", store.isLoaded, store.lastError ?? "no error recorded")
        check("the overview comes from the daemon", store.overview != nil)
        check("the simulation seeded optimizations", !store.optimizations.isEmpty, "\(store.optimizations.count)")
        check("templates load", !store.templates.isEmpty)

        // Connectors, a real group now.
        do {
            let list = try await store.client.connectors()
            check("connectors answer", true, "\(list.count)")
            let flow = try await store.client.githubSignInFlow()
            check("the GitHub flow state is readable", flow == nil || flow?.userCode.isEmpty == false)
            _ = try await store.client.connectionSelection(repoID: "acme/search-api")
            check("a repo's connection selection answers", true)
        } catch {
            check("connectors answer", false, error.localizedDescription)
        }

        // An optimization's detail, and an action with its live event.
        guard let live = store.optimizations.first(where: { $0.status == .running }) ?? store.optimizations.first else { return }
        await store.loadDetail(id: live.id)
        check("an optimization's detail loads", store.details[live.id] != nil, store.lastError ?? "")
        if let agent = store.details[live.id]?.agents.first(where: { $0.state == .working }) {
            await store.pauseAgent(agent.id)
            var paused = false
            for _ in 0..<50 where !paused {
                paused = store.details[live.id]?.agents.first { $0.id == agent.id }?.state == .paused
                if !paused { try? await Task.sleep(for: .milliseconds(100)) }
            }
            check("pausing an agent shows up through the daemon", paused)
            await store.resumeAgent(agent.id)
        } else {
            print("SKIP pausing an agent: no working agent in the simulation")
        }

        // Environment drafts: draft, revise, approve through the wizard's own flow object.
        do {
            let draft = try await store.client.createDraft(DraftRequest(repoID: "acme/search-api", goal: "Make search faster"))
            check("an environment draft is made", !draft.id.isEmpty && draft.state == .draft)
            let revised = try await store.client.reviseDraft(id: draft.id, instruction: "Use one machine")
            check("a draft can be revised", revised.id == draft.id && revised.revision >= draft.revision)
            let approved = try await store.client.approveDraft(id: draft.id)
            check("a draft can be approved", approved.state == .approved)
        } catch {
            check("environment drafts work", false, error.localizedDescription)
        }
    }

    private static func check(_ name: String, _ ok: Bool, _ detail: String = "") {
        count += 1
        if !ok { failures += 1 }
        print(ok ? "PASS \(name)" : "FAIL \(name)\(detail.isEmpty ? "" : ": \(detail)")")
    }

    /// Quits through NSApp.terminate, never exit(), and closes sheets first:
    /// an open sheet refuses terminate, and the run would wait forever.
    static func finish(code: Int32) {
        exitCode = code
        for window in NSApp.windows { if let sheet = window.attachedSheet { window.endSheet(sheet) } }
        NSApp.terminate(nil)
    }
}
