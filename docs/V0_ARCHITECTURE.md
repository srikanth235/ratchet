# Ratchet v0 — Architecture

> A ratchet only turns one way. Ratchet lets coding agents try to make your
> software faster or cheaper, and keeps a change only when a trusted benchmark
> confirms it is better.

Status: draft for discussion. UI design: "Ratchet App Design" canvas
(Midnight Brass), six screens; see §9.

## 1. v0 scope

v0 is **developer-centric performance optimization**: latency, throughput,
cold start, build time, cost per run. Anything with a reproducible benchmark
that prints a number.

| In scope | Out of scope (later) |
|---|---|
| macOS desktop app (Swift / SwiftUI) as mission control | Hosted controller / web dashboard |
| Agents: **Claude Code** cloud sessions, **Codex** cloud tasks | Grok, OpenCode, self-hosted runners |
| Cloud: **AWS** only | Azure, GCP, cross-cloud placement |
| GitHub repos with a reproducible benchmark (k6, criterion, pytest-benchmark, hyperfine, custom command) | Repos without a benchmark; ML / forecasting objectives |
| One optimization = one benchmark metric, one budget, one time limit | Multi-metric objectives, cross-agent collaboration |
| Result delivered as a PR plus a benchmark report | Autonomous production deployment |

## 2. Topology

```
┌──────────────────── Ratchet.app (macOS) ────────────────────┐
│ Optimization engine · Ratchet · Budget guard · Ledger · UI  │
│ AgentAdapters (Claude Code, Codex) · GitHub watcher         │
│ AWS controller (cost meter, reaper, bench runner)           │
└───────┬───────────────────────┬─────────────────────┬───────┘
        │ launch / steer        │ watch branches      │ AWS APIs (admin profile)
        ▼                       ▼                     ▼
 Agent sessions (vendor)     GitHub              AWS sandbox account
 ├─ Claude Code session ──▶ ratchet/<opt>/claude  ├─ agent boundary: tagged EC2, artifacts
 └─ Codex cloud task    ──▶ ratchet/<opt>/codex   ├─ trusted bench runner (eval-only role)
        │                        │ re-bench        └─ S3 sealed traces (deny agents)
        └── aws cli, scoped creds ──▶ agent boundary
```

Principles:

1. **The Mac is mission control, not the runtime.** Agents run in vendor cloud
   sessions; experiments and benchmarks run on AWS. Work continues if the lid
   closes.
2. **GitHub is the bus.** Agent = branch, candidate = commit, winner = PR.
3. **Agents never touch the trusted path.** Only benchmarks the app launches,
   with credentials no agent holds, move the ratchet.
4. **Budget is enforced by AWS permissions and the app, never by prompt
   compliance.**

## 3. Core concepts

| Concept | Definition |
|---|---|
| **Optimization** | Repo, benchmark, metric, agents, USD budget, time limit, stop rules |
| **Benchmark** | Command from `ratchet.yaml` that emits metrics (e.g. k6 `http_req_duration.p95`) |
| **Metric** | Name, direction (lower / higher is better), minimum gain, noise floor |
| **Agent** | One Claude Code or Codex session working on its own branch `ratchet/<opt>/<agent>` |
| **Candidate** | A commit an agent submits, with cost attributed |
| **Guardrails** | Hard pass/fail checks: tests, API contract, error rate, peak memory |
| **Bench runner** | Trusted re-benchmark of every candidate on pinned hardware |
| **Hidden trace** | Sealed workload agents never see; used to confirm a win |
| **Ratchet** | Pointer to the best *verified* candidate; only moves forward |
| **Ledger** | Append-only record of every spend event, candidate and result |

### Ratchet rule

A candidate becomes a **verified win** iff:

1. all guardrails pass, and
2. its bench result (N runs on pinned hardware) beats the current best by more
   than `max(noise_floor, min_gain)`, and
3. a confirm run on a **fresh instance** against the **hidden trace** also
   beats the current best.

Otherwise it is *rejected* (no gain), *failed confirm* (bench win that did not
hold on the hidden trace) or *blocked* (guardrail failed). The noise floor comes
from running the baseline N times before any agent starts.

## 4. Repository contract: `ratchet.yaml`

```yaml
version: 0
objective:
  metric: http_req_duration.p95   # key in the benchmark's JSON output
  direction: minimize
  min_gain: 0.03                  # 3%
bench:
  ref: ratchet/bench              # protected ref holding the trusted bench
  run: k6 run bench/k6/search.js
  repeats: 5                      # also used for the baseline noise floor
  hardware: c7i.2xlarge           # pinned, dedicated host
  confirm_trace: s3://sealed/search-trace.jsonl
guardrails:
  - run: pytest -q
  - run: openapi-diff --base main
  - error_rate: "< 0.001"
  - peak_rss: "< 1.2x"
protected:                        # changes here block a candidate
  - bench/**
  - ratchet.yaml
  - tests/contract/**
agent:
  instructions: RATCHET.md        # brief injected into every session
  allowed_instance_types: [c7i.2xlarge, c7i.xlarge, m7i.large]
```

The bench code comes from `bench.ref`, never from the candidate's tree, so an
agent cannot change how it is scored.

## 5. Agent adapters

```swift
protocol AgentAdapter {
    var kind: AgentKind { get }                    // .claudeCode, .codex
    func preflight() async throws -> Preflight     // signed in, environment ready
    func start(_ spec: AgentSpec) async throws -> SessionHandle
    func send(_ message: String, to: SessionHandle) async throws
    func events(_ session: SessionHandle) -> AsyncThrowingStream<AgentEvent, Error>
    func stop(_ session: SessionHandle) async throws
}
```

- **Claude Code:** start and steer cloud sessions; the session environment
  holds the agent's AWS credentials and a network policy allowing AWS + GitHub.
- **Codex:** start cloud tasks in a Codex environment with the same
  credentials and an internet allowlist for AWS and GitHub.

The app uses each vendor's CLI with the user's own login.

> **To verify before building:** the supported commands / APIs for creating,
> messaging and streaming Claude Code cloud sessions and Codex cloud tasks from a
> third-party app; how each injects secrets; session duration and concurrency
> limits; terms for programmatic use.

## 6. The improvement loop

- **Self-loop (always):** each session gets `RATCHET.md` plus a generated
  `.ratchet/context.md` (metric, current best, past candidates, remaining
  budget, rules). The agent iterates: profile → hypothesize → change → run
  experiments on AWS → commit → submit.
- **App-steered (when the app is open):** after each result, the app messages
  the agent with the verdict and the current best. Agents rebase onto the
  ratchet best (shared baseline, independent exploration).

**Submitting:** the agent pushes a commit with a `Ratchet-Candidate: true`
trailer. The watcher checks `protected` paths and queues the candidate for the
bench runner.

**Bench runner pipeline** (one candidate at a time, queue visible in the UI):
candidate → guardrails → bench × N → confirm on hidden trace → verdict.

**Stopping:** budget exhausted, time limit reached, N evals with no gain, or a
manual stop ("Stop & revoke keys").

## 7. AWS design

### Infrastructure as code: OpenTofu + sandbox contract

Sandbox infrastructure is defined in **OpenTofu** (Linux Foundation fork of
Terraform; MPL-licensed, so safe to bundle). Cloud independence comes from a
**sandbox contract**: every cloud module takes the same inputs and produces
the same outputs, and the app depends only on the contract.

```
infra/
  contract.md        # what every sandbox must provide
  aws/               # v0
  gcp/  azure/       # later
```

```hcl
# Inputs — identical for every cloud
variable "sandbox_id"             { type = string }
variable "region"                 { type = string }
variable "allowed_instance_types" { type = list(string) }

# Outputs — identical for every cloud
output "artifacts_bucket"   { value = ... }  # agent read/write, per-agent prefix
output "sealed_bucket"      { value = ... }  # bench runner only
output "agent_boundary_ref" { value = ... }  # AWS: permission boundary ARN
output "eval_identity_ref"  { value = ... }  # AWS: bench runner role / profile
output "network_ref"        { value = ... }  # AWS: subnet + security group
```

In Swift, `SandboxProvider` wraps the contract; `AWSSandboxProvider` is the
only v0 implementation.

| Layer | Lifetime | Mechanism |
|---|---|---|
| Sandbox (buckets, boundary, runner role, network) | Long-lived | **OpenTofu** |
| Per-agent credentials, kill switch | Minutes–hours | Cloud SDK from the app (`CredentialMinter`) |
| Experiment instances | Seconds–hours | Agents via cloud CLI, inside the boundary |
| Cost meter, reaper | Continuous | Cloud SDK from the app, filtered by tag |

**Operation:** the app bundles a pinned `tofu` binary; setup shows the
`tofu plan` diff in the app before `apply`; teardown is `tofu destroy`; it uses
the user's AWS profile or SSO login; state is local under
`~/Library/Application Support/Ratchet/sandboxes/<id>/` (remote S3 backend
later).

### AWS sandbox module (v0)

Into a **dedicated sandbox account**, `infra/aws` creates 7 resources:

| Resource | Purpose |
|---|---|
| `aws_s3_bucket.artifacts` | Agent build outputs, one prefix per agent |
| `aws_s3_bucket.sealed` | Hidden traffic traces, bench runner only |
| `aws_s3_bucket_policy.sealed_deny_agents` | Explicit deny for every agent identity |
| `aws_iam_policy.agent_boundary` | Tagged EC2 only, allow-listed instance types |
| `aws_iam_role.bench_runner` | Trusted re-bench with separate credentials |
| `aws_iam_instance_profile.bench_runner` | Attached to bench instances |
| `aws_security_group.ratchet` | Egress only; load generator ↔ target inside VPC |

### Agent credentials (per agent)

- An optimization-scoped IAM user per agent, constrained by the boundary:
  - EC2 `RunInstances` only with tag `ratchet:opt=<id>`, allow-listed types,
    max instances (default 4) and max lifetime (default 2 h),
    `InstanceInitiatedShutdownBehavior=terminate`
  - S3 only under `artifacts/<opt>/<agent>/`
  - Explicit deny on the sealed bucket, IAM, billing, everything else
- Injected into the vendor session environment at start.
- **Kill switch:** attach deny-all (immediate), then delete the key.
- Deleted when the optimization ends.

*Why not STS?* Assumed-role credentials expire in 1–12 h (1 h when chaining
from SSO) and can't be refreshed inside a running vendor session without
putting secrets in the transcript. Revisit with a credential-vending endpoint
once a hosted controller exists.

### Trusted bench runner

1. Watcher sees a candidate on an agent branch.
2. App launches pinned hardware (e.g. `c7i.2xlarge`, dedicated host, load
   generator in the same AZ) with the `bench_runner` role.
3. Checks out the candidate for code and `bench.ref` for the benchmark.
4. Runs guardrails, then the benchmark N times; if it beats the best, runs the
   confirm on a fresh instance against the hidden trace.
5. Writes result JSON to S3; the app updates the ledger and ratchet and
   notifies the agent.

### Cost metering and reaper

- **Live estimate:** tagged resources × on-demand/spot price, every minute,
  attributed to agent experiments vs. bench runner vs. storage/egress.
- **Reconcile:** Cost Explorer by tag after the fact (~24 h lag).
- **Guard:** warn at 80% of budget; at 100%, kill switch plus reaper.
- **Reaper:** terminates tagged resources past max lifetime, belonging to a
  stopped agent, or left after the optimization ends. Backstop later:
  EventBridge + Lambda in the sandbox account for when the Mac is offline.

## 8. Mac app

**Stack:** SwiftUI, Swift Concurrency (one actor per agent), GRDB/SQLite
ledger, Keychain for secrets, AWS SDK for Swift, bundled `tofu`, `git` / `gh`
and vendor CLIs as subprocesses. Developer ID + notarization, outside the Mac
App Store (its sandbox would block spawning CLIs).

```
RatchetApp/
  Optimization/ OptimizationEngine, Ratchet, StopRules
  Agents/       AgentAdapter, ClaudeCodeAdapter, CodexAdapter
  GitHub/       BranchWatcher, PRBuilder
  Sandbox/      SandboxProvider, TofuRunner, AWSSandboxProvider
  AWS/          CredentialMinter, CostMeter, Reaper, BenchRunner
  Ledger/       Schema, Store
  UI/           DesignSystem, Optimizations, NewOptimization, Environment,
                LiveAgents, BenchmarkHistory, Result
```

## 9. UI

### Design system: Midnight Brass

Dark, precise, "instrument panel" look. Implement as `UI/DesignSystem` tokens.

| Token | Value | Use |
|---|---|---|
| Window / sidebar / card | `#0B1019` / `#0E1420` / `#111827` | Layered surfaces, 1px hairlines `rgba(148,163,190,.13)` |
| Text / secondary / tertiary | `#ECEFF4` / `#A7B1C2` / `#8592A6` | All pass 4.5:1 on cards |
| Brass (accent) | `#D9B26F`, highlight `#EBCB8E` | Primary actions, ratchet line, live state, trusted path |
| Mint | `#7BD8B0` | Verified wins, passed guardrails |
| Rose | `#F08A93` | Blocked, stop / revoke |
| Claude Code / Codex | lilac `#B8A6FF` / sky `#7CC4F2` | Agent avatars and chart points |

- **Type:** Instrument Serif for page titles and hero numbers; Instrument Sans
  for UI; JetBrains Mono for metrics, SHAs, code (tabular figures).
- **Shape:** 14px card radius, 8–10px controls, uppercase tracked eyebrow
  labels, subtle dot-grid behind charts.

### Screens

1. **Optimizations:** KPI cards (verified wins, median gain, spend, cost per
   win), live run with ratchet chart, activity feed, table of all runs.
2. **New optimization:** four steps (target, benchmark & metric, guardrails,
   agents & limits) plus a review panel with the generated `ratchet.yaml` and
   preflight checks; primary action "Run baseline & start".
3. **AWS sandbox:** architecture diagram (trusted path in brass), OpenTofu plan
   diff, account details, agent limits, contract outputs.
4. **Live agents:** one card per agent (hypothesis, recent eval bars, stats,
   live log, message / open session / pause) and the bench runner pipeline.
5. **Benchmark history:** ratchet chart with noise band, latency histogram
   baseline vs. best, percentile table, every candidate with its verdict.
6. **Result:** headline before → after, time by code path, winning diff,
   per-agent results, spend breakdown, teardown checklist, "Open pull
   request".

## 10. Milestones

1. **M0, skeleton:** app shell, design-system tokens, `ratchet.yaml` parser,
   ledger, GitHub watcher.
2. **M1, AWS sandbox:** contract, OpenTofu AWS module, `tofu` runner,
   credential minter, cost meter, reaper.
3. **M2, trusted bench:** bench runner, guardrails, noise-floor baseline,
   hidden-trace confirm, ratchet rule.
4. **M3, first agent:** Claude Code adapter end to end on a demo service.
5. **M4, second agent:** Codex adapter; parallel agents; shared ratchet.
6. **M5, demo:** a sample HTTP service (k6 p95) plus one CLI cold-start
   benchmark (hyperfine), with cost per verified win reported.

## 11. Open questions

1. Do vendor sessions allow reliable programmatic create/steer/stream for a
   third-party app? (Blocks M3/M4.)
2. Is a per-agent IAM user acceptable, or is a credential-vending endpoint
   needed from day one?
3. Agents on EC2 only, or also ECS / Lambda for serverless targets?
4. Agent token usage: shown as quota only, or counted against the budget?
5. How are hidden traces produced: captured from production, or generated?
