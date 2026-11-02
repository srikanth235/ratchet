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
        check("five optimizations", store.optimizationStore.optimizations.count == 5)
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
        check("every page in the sidebar has its own Command-number", Set(Page.sidebar.compactMap(\.shortcutNumber)).count == Page.sidebar.count)

        // Live events through the store.
        await store.optimizationStore.loadDetail(id: "opt-checkout")
        check("the live optimization has agents", store.optimizationStore.details["opt-checkout"]?.agents.count == 2)
        if let mock = store.scenario {
            let before = store.optimizationStore.optimization(id: "opt-checkout")?.spentUSD ?? 0
            // The store follows the event stream by itself; give it a moment to apply what the client emitted.
            for _ in 0..<2 { await mock.advance() }
            try? await Task.sleep(for: .milliseconds(400))
            let after = store.optimizationStore.optimization(id: "opt-checkout")
            check("events spend money", (after?.spentUSD ?? 0) > before)
            check("a win becomes the new best", (after?.best ?? 999) < 288 && after?.winsCount == 3)
            check("the overview grows with the win", store.overview?.winsCount == 10)
        } else {
            check("the sample client is in use", false)
        }

        // Agents.
        await store.optimizationStore.pauseAgent("agent-claude-login")
        check("pausing an agent", store.optimizationStore.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-login" }?.state == .paused)
        await store.optimizationStore.resumeAgent("agent-claude-login")
        check("resuming an agent", store.optimizationStore.details["opt-checkout"]?.agents.first { $0.id == "agent-claude-login" }?.state == .working)

        // Connectors.
        await store.connectorStore.addConnector(NewConnector(kind: .aws, name: "selftest-account", detail: "SSO · us-east-2"))
        check("adding a connector", store.connectorStore.connectors(of: .aws).contains { $0.name == "selftest-account" })
        if let added = store.connectorStore.connectors(of: .aws).first(where: { $0.name == "selftest-account" }) {
            await store.connectorStore.removeConnector(added.id)
            check("removing a connector", !store.connectorStore.connectors(of: .aws).contains { $0.id == added.id })
        }
        await store.connectorStore.signInConnector("aws-acme-staging")
        check("signing in an expired connector", store.connectorStore.connectors.first { $0.id == "aws-acme-staging" }?.state == .connected)

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
            check("the new optimization is on screen", store.optimizationStore.optimization(id: started.id) != nil)
        }

        // Setup: repo search, ratchet.yaml review, template pull request, connector editing, drafts, overrides.
        let setup = NewOptimizationFlow(client: store.client)
        await setup.load()
        setup.repoQuery = "search"
        await setup.search()
        check("searching repos narrows the list", setup.shownRepos.map(\.id) == ["acme-org/search-svc"])
        await setup.selectRepo("acme-org/search-svc")
        check("an invalid ratchet.yaml says why", setup.inspection?.outcome == .invalid && !(setup.inspection?.problems.isEmpty ?? true) && !setup.repoIsUsable)
        await setup.selectRepo("acme-org/web-app")
        await setup.proposeTemplate("build-size")
        check("a template opens a pull request", setup.proposal?.prURL.contains("/pull/") == true)
        await setup.chooseLocalFolder("/Users/dev/shop-api")
        check("a local folder is read through its remote", setup.inspection?.repo == "srikanth235/shop-api" && setup.repoIsUsable)
        await setup.chooseLocalFolder("/Users/dev/no-remote")
        check("a folder without a remote cannot be used", !setup.repoIsUsable)
        do {
            try await store.connectorStore.updateConnector("aws-personal-dev", ConnectorUpdate(name: "dev", region: "eu-west-2", cap: .remove))
            let edited = store.connectorStore.connectors.first { $0.id == "aws-personal-dev" }
            check("an AWS connector is edited in place", edited?.name == "dev" && edited?.region == "eu-west-2" && edited?.monthlyCapUSD == nil)
            let checked = try await store.connectorStore.checkConnector("gh-srikanth235")
            check("Check connection answers in a line", checked.ok && !checked.detail.isEmpty)
            let saved = try await store.client.saveAdvancedSettings(AdvancedSettings(benchAMI: "ami-0123456789abcdef0"))
            check("the development overrides are saved", saved.benchAMI == "ami-0123456789abcdef0")
        } catch {
            check("connector and settings forms work", false, error.localizedDescription)
        }
        let drafted = NewOptimizationFlow(client: store.client)
        await drafted.load()
        drafted.goal = "Go"
        await drafted.go(to: .environment)
        await drafted.discardDraft()
        check("a draft can be discarded", drafted.draft == nil && drafted.step == .connections)

        // Sandbox.
        check("the sandbox is on", store.sandboxStore.sandbox.isOn)
        await store.sandboxStore.endSandbox()
        check("End now tears the sandbox down", !store.sandboxStore.sandbox.isOn && store.sandboxStore.receipt?.allGone == true)

        // Agent log, Result details and the benchmark run.
        await store.insight.loadAgentLog("agent-claude-team")
        let log = store.insight.log("agent-claude-team")
        check("an agent's log loads", (log?.lines.count ?? 0) > 2 && log?.error == nil)
        let cursor = log?.cursor
        await store.insight.loadAgentLog("agent-claude-team")
        check("reading the log again adds nothing", store.insight.log("agent-claude-team")?.lines.count == log?.lines.count && cursor == store.insight.log("agent-claude-team")?.cursor)
        await store.insight.loadInsights("opt-cli")
        let insights = store.insight.insights(of: "opt-cli")
        check("the result has spans, compare rows and a diff", insights?.spans?.isEmpty == false && insights?.compare.isEmpty == false && insights?.change?.files.isEmpty == false)
        check("the spend adds up per agent", insights?.spend.map { abs($0.agents.reduce(0) { $0 + $1.usd } - $0.totalUSD) < 0.05 } == true)
        await store.insight.loadBenchmark("opt-checkout")
        check("the benchmark has a metric switch", store.insight.series(of: "opt-checkout")?.hasMetricSwitch == true)
        await store.insight.loadBenchmark("opt-checkout", metric: "p99")
        check("another metric re-plots the ratchet", store.insight.series(of: "opt-checkout")?.metric == "p99" && store.insight.series(of: "opt-checkout")?.noiseFloor == nil)
        check("the candidate filter narrows the table", CandidateFilter.blocked.apply(to: store.insight.results(of: "opt-checkout") ?? []).count == 1)

        // Symbols used in the app exist.
        // Errors reach the banner: a failed Stop says what happened, stays through a refresh, and the next action clears it.
        await store.optimizationStore.stopOptimization("opt-bundle")  // already ended
        check("a failed Stop is recorded for the banner", store.lastError?.hasPrefix("Couldn't stop the optimization.") == true,
              store.lastError ?? "no error")
        await store.refresh()
        check("a refresh does not wipe an error nobody has read", store.lastError != nil)
        store.dismissError()
        check("the banner's close button clears the error", store.lastError == nil)
        await store.sandboxStore.endSandbox()
        await store.sandboxStore.endSandbox()  // already off
        check("a failed End sandbox is recorded", store.lastError?.hasPrefix("Couldn't end the sandbox.") == true,
              store.lastError ?? "no error")
        await store.optimizationStore.pauseAgent("agent-claude-login")
        check("the next action clears the previous error", store.lastError == nil)

        // Launch at login: the system's answers map to what Settings shows, and a login launch is recognised.
        check("an enabled login item reads as on", LoginItem.State(.enabled) == .on)
        check("a login item waiting for approval still reads as requested", LoginItem.State(.requiresApproval).isRequested)
        check("a missing login item reads as unavailable", LoginItem.State(.notFound) == .unavailable)
        let normalLaunch = NSAppleEventDescriptor(
            eventClass: AEEventClass(kCoreEventClass), eventID: AEEventID(kAEOpenApplication), targetDescriptor: nil,
            returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
        check("a plain open is not a login launch", !LaunchContext.isLoginLaunch(normalLaunch))
        let loginLaunch = NSAppleEventDescriptor(
            eventClass: AEEventClass(kCoreEventClass), eventID: AEEventID(kAEOpenApplication), targetDescriptor: nil,
            returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
        loginLaunch.setParam(NSAppleEventDescriptor(enumCode: 0x6C67_6974), forKeyword: AEKeyword(keyAEPropData))
        check("an open with the login-item marker is a login launch", LaunchContext.isLoginLaunch(loginLaunch))
        let item = LoginItem(touchesSystem: false)
        check("test mode never registers the app", item.set(true) == nil && item.state == .on && item.set(false) == nil && item.state == .off)

        for name in ["chart.line.uptrend.xyaxis", "gearshape", "link"] {
            check("SF Symbol \(name) exists", NSImage(systemSymbolName: name, accessibilityDescription: nil) != nil)
        }
    }

    /// The real client path end to end: the generated client, the unix socket, the bearer token, the mapping, the event
    /// stream, against `ratchetd serve --mode sim`. Run by tools/smoke-daemon.sh; nothing here touches the sample data.
    private static func runLive(store: AppStore, daemon: DaemonController) async {
        check("the app uses the daemon, not the sample client", store.scenario == nil && !Store.isOverridden)
        check("the daemon answers on the socket", daemon.phase.isRunning, "\(daemon.phase)")
        check("the store loaded from the daemon", store.isLoaded, store.lastError ?? "no error recorded")
        check("the overview comes from the daemon", store.overview != nil)
        check("the simulation seeded optimizations", !store.optimizationStore.optimizations.isEmpty, "\(store.optimizationStore.optimizations.count)")
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

        // The setup routes.
        do {
            _ = try await store.client.awsProfiles()
            _ = try await store.client.detectedHarnesses(redetect: false)
            let advanced = try await store.client.advancedSettings()
            check("the development overrides are readable", !advanced.defaultModuleDir.isEmpty || advanced.moduleDir.isEmpty)
            _ = try await store.client.connectorSetup()
            _ = try await store.client.preflight(PreflightRequest(source: .demo, agentIDs: [], placement: .local))
            check("what's missing answers for a demo", true)
        } catch {
            check("the setup routes answer", false, error.localizedDescription)
        }

        // An optimization's detail, and an action with its live event.
        guard let live = store.optimizationStore.optimizations.first(where: { $0.status == .running }) ?? store.optimizationStore.optimizations.first else { return }
        await store.optimizationStore.loadDetail(id: live.id)
        check("an optimization's detail loads", store.optimizationStore.details[live.id] != nil, store.lastError ?? "")
        if let agent = store.optimizationStore.details[live.id]?.agents.first(where: { $0.state == .working }) {
            await store.optimizationStore.pauseAgent(agent.id)
            var paused = false
            for _ in 0..<50 where !paused {
                paused = store.optimizationStore.details[live.id]?.agents.first { $0.id == agent.id }?.state == .paused
                if !paused { try? await Task.sleep(for: .milliseconds(100)) }
            }
            check("pausing an agent shows up through the daemon", paused)
            await store.optimizationStore.resumeAgent(agent.id)
        } else {
            print("SKIP pausing an agent: no working agent in the simulation")
        }

        // The log, the result details and the benchmark series, through the daemon.
        if let finished = store.optimizationStore.optimizations.first(where: { $0.status.isFinished && $0.winsCount > 0 }) {
            await store.insight.loadInsights(finished.id)
            let insights = store.insight.insights(of: finished.id)
            check("the result details load", insights != nil, store.lastError ?? "")
            check("the spend breakdown comes from the daemon", insights?.spend?.categories.isEmpty == false)
            check("the simulation answers with a real diff", insights?.change?.unavailable == nil && insights?.change?.files.isEmpty == false,
                  insights?.change?.unavailable ?? "no change")
            await store.insight.loadBenchmark(finished.id)
            check("the ratchet series loads", store.insight.series(of: finished.id)?.steps.isEmpty == false)
            check("the candidates load with their verdicts", store.insight.results(of: finished.id)?.isEmpty == false)
        } else {
            print("SKIP result details: no finished optimization with a win in the simulation")
        }
        if let agent = store.optimizationStore.details[live.id]?.agents.first {
            await store.insight.loadAgentLog(agent.id)
            check("an agent's log loads from the daemon", store.insight.log(agent.id)?.error == nil, store.insight.log(agent.id)?.error ?? "")
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
