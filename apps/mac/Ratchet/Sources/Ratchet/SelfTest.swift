import AppKit
import RatchetKit

/// Self-test for agents: runs the app's real store, router and wizard on the sample client (fixed clock), prints PASS
/// or FAIL per check, and quits with exit code 0 or 1.
///   APP_STORE=/tmp/Ratchet-self.store ~/Applications/Ratchet.app/Contents/MacOS/Ratchet --selftest
/// (tools/selftest.sh does this.) Add a check for every story with logic.
@MainActor
enum SelfTest {
    static var requested: Bool { CommandLine.arguments.contains("--selftest") }
    /// Read by AppDelegate.applicationWillTerminate.
    static var exitCode: Int32 = 0
    private static var failures = 0
    private static var count = 0

    static func runIfRequested(store: AppStore, router: AppRouter) {
        guard requested else { return }
        // Print each line at once, so a crash keeps the lines before it.
        setvbuf(stdout, nil, _IOLBF, 0)
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
