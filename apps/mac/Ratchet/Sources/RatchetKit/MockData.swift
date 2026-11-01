import Foundation

/// The sample world behind `MockRatchetClient`: five optimizations in every state, connectors, templates, a
/// full-stack environment draft. Dates are relative to `now`, so a fixed `now` gives identical snapshots.
public struct MockWorld: Sendable {
    public var now: Date
    public var optimizations: [Optimization]
    public var details: [String: OptimizationDetail]
    public var winSeries: [WinStep]
    public var connectors: [Connector]
    public var selections: [String: ConnectionSelection]
    public var repos: [Repo]
    public var templates: [Template]
    public var benchmarks: [BenchmarkRun]
    public var sandbox: SandboxStatus
    public var receipt: TeardownReceipt?
    public var drafts: [String: EnvironmentDraft]
    public var budgetUSD: Double

    /// One verified win on the staircase: the total value locked in right after it.
    public struct WinStep: Sendable, Hashable {
        public var date: Date
        public var valueUSD: Double
        public var title: String
        public var optimizationTitle: String
    }

    public static let hour: TimeInterval = 3600
    public static let day: TimeInterval = 86_400

    public static func standard(now: Date) -> MockWorld {
        let optimizations = makeOptimizations(now: now)
        var details: [String: OptimizationDetail] = [:]
        for optimization in optimizations { details[optimization.id] = makeDetail(optimization, now: now) }
        // Days counted from the start of the 90-day window, as in design/v02.html.
        func step(_ day: Double, _ value: Double, _ title: String, _ optimization: String) -> WinStep {
            WinStep(date: now.addingTimeInterval(-(90 - day) * Self.day), valueUSD: value, title: title, optimizationTitle: optimization)
        }
        let winSteps: [WinStep] = [
            step(6, 1200, "Lazy-load the plugin registry", "CLI cold start"),
            step(14, 2400, "Snapshot the config parse", "CLI cold start"),
            step(23, 4100, "Quantile loss", "Forecast accuracy"),
            step(31, 6000, "Holiday regressors", "Forecast accuracy"),
            step(38, 8600, "Seasonal naive blend", "Forecast accuracy"),
            step(52, 10_600, "Expand queries with synonyms", "Search relevance"),
            step(60, 11_500, "Rerank with a cross-encoder", "Search relevance"),
            step(73, 13_600, "Batch the cart price lookups", "Checkout p95"),
            step(84, 15_400, "Pool size 20 → 48, reuse connections", "Checkout p95"),
        ]
        return MockWorld(
            now: now,
            optimizations: optimizations,
            details: details,
            winSeries: winSteps,
            connectors: makeConnectors(),
            selections: [
                "acme-org/shop-api": ConnectionSelection(
                    repoID: "acme-org/shop-api", awsID: "aws-acme-prod",
                    agents: ["agent-claude-login": .local, "agent-claude-team": .cloud]),
            ],
            repos: makeRepos(),
            templates: makeTemplates(),
            benchmarks: makeBenchmarks(optimizations, now: now),
            sandbox: SandboxStatus(isOn: true, account: "acme-prod", costSoFarUSD: 1.20, offAt: now.addingTimeInterval(4320)),
            receipt: details["opt-forecast"]?.receipt,
            drafts: [:],
            budgetUSD: 500)
    }

    // MARK: Optimizations

    static func makeOptimizations(now: Date) -> [Optimization] {
        let ms = MetricSpec(name: "p95 latency", unit: "ms")
        return [
            Optimization(
                id: "opt-checkout", title: "Checkout p95", repo: "acme-org/shop-api", goal: "Make checkout p95 faster",
                status: .running, metric: ms, baseline: 412, best: 288, noise: 0.021, spentUSD: 38, budgetUSD: 50,
                valuePerYearUSD: 4800, secondsLeft: 4320, timeLimitHours: 3, startedAt: now.addingTimeInterval(-6480),
                winsCount: 2, evalsCount: 11, agentsCount: 2),
            Optimization(
                id: "opt-search", title: "Search relevance", repo: "acme-org/search-svc", goal: "Improve search relevance",
                status: .waiting, metric: MetricSpec(name: "relevance", label: "nDCG", decimals: 3, lowerIsBetter: false),
                baseline: 0.612, best: 0.648, noise: 0.008, spentUSD: 22, budgetUSD: 80, valuePerYearUSD: 2900,
                timeLimitHours: 4, startedAt: now.addingTimeInterval(-3 * day), winsCount: 2, evalsCount: 9, agentsCount: 1),
            Optimization(
                id: "opt-forecast", title: "Forecast accuracy", repo: "acme-org/demand-model", goal: "Lower the forecast error",
                status: .merged, metric: MetricSpec(name: "WAPE", label: "WAPE", unit: "%", decimals: 1),
                baseline: 18.2, best: 15.9, noise: 0.012, spentUSD: 64, valuePerYearUSD: 6500,
                timeLimitHours: 6, startedAt: now.addingTimeInterval(-40 * day), prNumber: 198, winsCount: 3, evalsCount: 21, agentsCount: 2),
            Optimization(
                id: "opt-cli", title: "CLI cold start", repo: "srikanth235/acme-cli", goal: "Make the CLI start faster",
                status: .done, metric: ms, baseline: 840, best: 510, noise: 0.03, spentUSD: 41, valuePerYearUSD: 1200,
                timeLimitHours: 2, startedAt: now.addingTimeInterval(-70 * day), prNumber: 214, winsCount: 2, evalsCount: 14, agentsCount: 1),
            Optimization(
                id: "opt-bundle", title: "Bundle size", repo: "acme-org/web-app", goal: "Shrink the JavaScript bundle",
                status: .noWin, metric: MetricSpec(name: "bundle size", unit: "KB"), baseline: 1840, noise: 0.02,
                spentUSD: 19, timeLimitHours: 2, startedAt: now.addingTimeInterval(-20 * day), evalsCount: 8, agentsCount: 1),
        ]
    }

    static func makeDetail(_ optimization: Optimization, now: Date) -> OptimizationDetail {
        let start = optimization.startedAt ?? now
        let team = "Claude Code · team key"
        let login = "Claude Code · your login"
        func move(_ index: Int, _ hoursIn: Double, _ value: Double, _ title: String) -> RatchetMove {
            RatchetMove(id: "\(optimization.id)-move-\(index)", at: start.addingTimeInterval(hoursIn * hour), value: value, title: title)
        }
        func candidate(_ sha: String, _ title: String, _ agent: String, _ verdict: Verdict, _ value: Double?, _ hoursIn: Double) -> Candidate {
            Candidate(id: sha, title: title, agent: agent, verdict: verdict, value: value, at: start.addingTimeInterval(hoursIn * hour))
        }
        switch optimization.id {
        case "opt-checkout":
            return OptimizationDetail(
                optimization: optimization,
                agents: [
                    Agent(id: "agent-claude-login", name: login, placement: .local, state: .working,
                          activity: "Batching the cart price lookups", evals: 6, wins: 1, spentUSD: 0),
                    Agent(id: "agent-claude-team", name: team, placement: .cloud, state: .working,
                          activity: "Tuning the connection pool", evals: 5, wins: 1, spentUSD: 38),
                ],
                candidates: [
                    candidate("e41b7a0", "Index orders by customer_id", login, .evaluating, nil, 1.8),
                    candidate("a91f3c2", "Pool size 20 → 48, reuse connections", team, .verified, 288, 1.5),
                    candidate("3d8c0f1", "Cache the tax table per request", login, .noise, 351, 1.2),
                    candidate("b20e6d9", "Skip the audit write on the hot path", team, .blocked, nil, 0.9),
                    candidate("7c02e9d", "Batch the cart price lookups", login, .verified, 351, 0.6),
                    candidate("19f4aa3", "Compress the order payload", team, .noise, 409, 0.3),
                ],
                moves: [move(1, 0.6, 351, "Batch the cart price lookups"), move(2, 1.5, 288, "Pool size 20 → 48, reuse connections")])
        case "opt-search":
            return OptimizationDetail(
                optimization: optimization,
                agents: [Agent(id: "agent-search-1", name: login, placement: .local, state: .paused,
                               activity: "Waiting for the environment to be approved", evals: 9, wins: 2, spentUSD: 22)],
                candidates: [
                    candidate("5ab90c4", "Rerank with a cross-encoder", login, .verified, 0.648, 40),
                    candidate("c7710e2", "Expand queries with synonyms", login, .verified, 0.631, 20),
                    candidate("0f3be85", "Boost exact title matches", login, .noise, 0.614, 6),
                ],
                moves: [move(1, 20, 0.631, "Expand queries with synonyms"), move(2, 40, 0.648, "Rerank with a cross-encoder")])
        case "opt-forecast":
            return OptimizationDetail(
                optimization: optimization,
                agents: [],
                candidates: [
                    candidate("d19a4b7", "Seasonal naive blend", team, .verified, 15.9, 30),
                    candidate("86e2c30", "Holiday regressors", team, .verified, 16.8, 20),
                    candidate("4c5f1d8", "Quantile loss", login, .verified, 17.6, 9),
                    candidate("91b7a65", "Longer lookback window", login, .noise, 18.1, 5),
                ],
                moves: [move(1, 9, 17.6, "Quantile loss"), move(2, 20, 16.8, "Holiday regressors"), move(3, 30, 15.9, "Seasonal naive blend")],
                receipt: TeardownReceipt(
                    id: "receipt-forecast", account: "acme-prod", startedAt: start, endedAt: start.addingTimeInterval(5.8 * hour),
                    costUSD: 2.61, destroyed: ["EC2 c7i.xlarge × 1", "EBS 80 GB", "Security group", "Key pair"]),
                filesChanged: 4, additions: 96, deletions: 31)
        case "opt-cli":
            return OptimizationDetail(
                optimization: optimization,
                agents: [],
                candidates: [
                    candidate("2e7d0b9", "Lazy-load the plugin registry", login, .verified, 510, 1.4),
                    candidate("f08c3a1", "Snapshot the config parse", login, .verified, 640, 0.7),
                    candidate("a6d41e5", "Avoid the update check on start", login, .noise, 835, 0.3),
                ],
                moves: [move(1, 0.7, 640, "Snapshot the config parse"), move(2, 1.4, 510, "Lazy-load the plugin registry")],
                receipt: TeardownReceipt(
                    id: "receipt-cli", account: "personal-dev", startedAt: start, endedAt: start.addingTimeInterval(1.9 * hour),
                    costUSD: 0.38, destroyed: ["EC2 c7i.large × 1", "EBS 40 GB", "Security group"]),
                filesChanged: 5, additions: 118, deletions: 34)
        default:
            return OptimizationDetail(
                optimization: optimization,
                candidates: [
                    candidate("6b1e9c0", "Tree-shake the date library", login, .noise, 1832, 1.1),
                    candidate("c3a07f2", "Split the charts bundle", login, .noise, 1845, 0.6),
                    candidate("12d8e44", "Drop the polyfills", login, .blocked, nil, 0.2),
                ],
                receipt: TeardownReceipt(
                    id: "receipt-bundle", account: "acme-prod", startedAt: start, endedAt: start.addingTimeInterval(1.6 * hour),
                    costUSD: 0.21, destroyed: ["EC2 c7i.xlarge × 1", "EBS 40 GB"]))
        }
    }

    // MARK: Connectors, repos, templates

    static func makeConnectors() -> [Connector] {
        [
            Connector(id: "gh-srikanth235", kind: .github, name: "srikanth235", detail: "Personal · 42 repos", state: .connected, isDefault: true, origin: .device),
            Connector(id: "gh-acme-org", kind: .github, name: "acme-org", detail: "GitHub App · 18 repos", state: .connected, origin: .token),
            Connector(id: "aws-acme-prod", kind: .aws, name: "acme-prod", detail: "SSO · us-east-1 · cap $500 / month",
                      state: .connected, isDefault: true, monthlyCapUSD: 500, usedThisMonthUSD: 88,
                      profile: "acme-prod", region: "us-east-1"),
            Connector(id: "aws-personal-dev", kind: .aws, name: "personal-dev", detail: "~/.aws profile · us-west-2 · cap $100 / month",
                      state: .connected, monthlyCapUSD: 100, usedThisMonthUSD: 4,
                      profile: "personal-dev", region: "us-west-2"),
            Connector(id: "aws-acme-staging", kind: .aws, name: "acme-staging", detail: "SSO · eu-west-1", state: .expired,
                      profile: "acme-staging", region: "eu-west-1"),
            Connector(id: "agent-claude-login", kind: .agent, name: "Claude Code · your login", detail: "Found on this Mac · v2.1.314",
                      state: .signedIn, isDefault: true, origin: .login, cli: .claudeCode),
            Connector(id: "agent-claude-team", kind: .agent, name: "Claude Code · team key", detail: "API key ····4f2a · also runs in the cloud",
                      state: .valid, canRunInCloud: true, origin: .apiKey, cli: .claudeCode),
            Connector(id: "agent-codex-login", kind: .agent, name: "Codex · your login", detail: "Found on this Mac · v0.166 · run codex login",
                      state: .signedOut, origin: .login, cli: .codex),
        ]
    }

    static func makeRepos() -> [Repo] {
        [
            Repo(id: "acme-org/shop-api", account: "acme-org"),
            Repo(id: "acme-org/search-svc", account: "acme-org"),
            Repo(id: "acme-org/demand-model", account: "acme-org"),
            Repo(id: "acme-org/web-app", account: "acme-org"),
            Repo(id: "srikanth235/acme-cli", account: "srikanth235"),
            Repo(id: "srikanth235/notes-sync", defaultBranch: "trunk", account: "srikanth235"),
        ]
    }

    static func makeTemplates() -> [Template] {
        [
            Template(id: "http-latency", kind: .template, category: "perf", title: "HTTP latency",
                     summary: "Open-loop load test of an endpoint.", measures: "p50, p95 and p99, error rate", machine: "c7i.2xlarge"),
            Template(id: "cli-cold-start", kind: .template, category: "perf", title: "CLI cold start",
                     summary: "Wall time of repeated invocations.", measures: "median start time", machine: "c7i.large"),
            Template(id: "test-suite-time", kind: .template, category: "perf", title: "Test suite time",
                     summary: "How long the test command takes.", measures: "suite duration", machine: "c7i.2xlarge"),
            Template(id: "build-size", kind: .template, category: "perf", title: "Build size",
                     summary: "Build seconds and bundle size.", measures: "bundle KB, build time", machine: "c7i.xlarge"),
            Template(id: "memory", kind: .template, category: "perf", title: "Memory",
                     summary: "Peak memory of a workload.", measures: "peak RSS", machine: "c7i.xlarge"),
            Template(id: "forecast-backtest", kind: .template, category: "forecasting", title: "Forecast backtest",
                     summary: "Rolling-origin backtest on the newest periods.", measures: "WAPE, MAPE, bias", machine: "c7i.xlarge"),
            Template(id: "llm-eval", kind: .template, category: "llm", title: "LLM eval",
                     summary: "Score, cost and tokens on a hidden eval set.", measures: "score, cost per call", machine: "c7i.large"),
            Template(id: "demo-search", kind: .demo, category: "perf", title: "Search latency demo",
                     summary: "A small search service. Runs in simulation, no setup.", measures: "p95 latency", machine: "simulated"),
            Template(id: "demo-cli", kind: .demo, category: "perf", title: "CLI demo",
                     summary: "A tiny CLI with a slow start. Runs in simulation.", measures: "cold start", machine: "simulated"),
            Template(id: "demo-forecast", kind: .demo, category: "forecasting", title: "Forecast demo",
                     summary: "A toy demand model. Runs in simulation.", measures: "WAPE", machine: "simulated"),
        ]
    }

    static func makeBenchmarks(_ optimizations: [Optimization], now: Date) -> [BenchmarkRun] {
        let templates: [String: (String, String)] = [
            "opt-checkout": ("Open-loop load test", "http-latency"),
            "opt-search": ("Relevance eval", "llm-eval"),
            "opt-forecast": ("Rolling backtest", "forecast-backtest"),
            "opt-cli": ("Cold start", "cli-cold-start"),
            "opt-bundle": ("Bundle size", "build-size"),
        ]
        return optimizations.map { optimization in
            let info = templates[optimization.id] ?? ("Benchmark", "custom")
            return BenchmarkRun(
                id: "bench-" + optimization.id, title: info.0, optimizationTitle: optimization.title, templateID: info.1,
                metric: optimization.metric, baseline: optimization.baseline, best: optimization.best,
                noise: optimization.noise ?? 0.02, evals: optimization.evalsCount,
                date: optimization.startedAt ?? now)
        }
    }

    // MARK: Environment drafts

    static func fullStackDraft(id: String, repoID: String) -> EnvironmentDraft {
        EnvironmentDraft(
            id: id, repoID: repoID, scope: .full,
            headline: "This task needs the full stack: the API, Postgres with seeded orders, Redis, and a separate load generator.",
            emphasis: "full stack",
            reason: "p95 depends on database queries, so a single machine wouldn't measure it.",
            draftedBy: "Claude Code", draftSeconds: 120, draftCostUSD: 0.31, readyMinutes: 6,
            resources: [
                DraftResource(id: "api", name: "API", detail: "shop-api in the repo's container", machine: "c7i.2xlarge", usdPerHour: 0.36),
                DraftResource(id: "pg", name: "Postgres 16", detail: "2.1 GB of anonymised orders", machine: "r7g.large", usdPerHour: 0.11),
                DraftResource(id: "redis", name: "Redis 7", detail: "emptied before each measurement", machine: "t4g.medium", usdPerHour: 0.03),
                DraftResource(id: "load", name: "Load generator", detail: "k6, on its own host", machine: "c7i.large", usdPerHour: 0.09),
            ],
            files: ["main.tf", "Dockerfile", "seed.sh"],
            checks: "isolated, results sealed, same hardware every time, full teardown")
    }
}
