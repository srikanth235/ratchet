# Ratchet v0 — Architecture

> A ratchet only turns one way. Ratchet runs autonomous improvement campaigns
> where the "best known" solution only moves forward, and only when a trusted
> evaluator confirms the improvement.

Status: draft for discussion.

## 1. v0 scope

| In scope | Out of scope (later) |
|---|---|
| macOS desktop app (Swift / SwiftUI) as mission control | Hosted controller / web dashboard |
| Harnesses: **Claude Code** cloud sessions, **Codex** cloud tasks | Grok, OpenCode, self-hosted runners |
| Cloud: **AWS** only | Azure, GCP, cross-cloud placement |
| GitHub-hosted repos with an existing reproducible eval | Repos without an eval |
| One campaign = one objective, one budget, one deadline | Cross-harness collaboration |
| Result delivered as a PR plus an eval report | Autonomous production deployment |

## 2. Topology

```
┌──────────────────── Ratchet.app (macOS) ────────────────────┐
│ Campaign engine · Ratchet · Budget guard · Ledger · UI      │
│ HarnessAdapters (Claude Code, Codex) · GitHub watcher       │
│ AWS controller (provision, cost meter, reaper, eval runner) │
└───────┬───────────────────────┬─────────────────────┬───────┘
        │ start/steer/observe   │ watch branches      │ AWS APIs (admin profile)
        ▼                       ▼                     ▼
 Vendor cloud sessions       GitHub              AWS sandbox account
 ├─ Claude Code session ──▶ lane/<c>/claude ◀─┐  ├─ experiment jobs (agent creds)
 └─ Codex cloud task    ──▶ lane/<c>/codex  ◀─┤  ├─ eval jobs     (eval creds, app-only)
        │                                     │  └─ S3: artifacts / sealed data
        └──── aws CLI with scoped creds ──────┴──▶
```

Principles:

1. **The Mac is mission control, not the runtime.** Agents run in vendor cloud
   sessions; experiments and evals run on AWS. Lanes keep working if the lid
   closes.
2. **GitHub is the bus.** Lane = branch, candidate = commit, winner = PR.
3. **Agents never touch the trusted evaluator.** Only app-launched evals move the ratchet.
4. **Budget is enforced by AWS permissions and the app, never by prompt compliance.**

## 3. Core concepts

| Concept | Definition |
|---|---|
| **Campaign** | Repo, objective, harness lanes, USD budget, deadline, stop rules |
| **Objective** | Metric name, direction (↑/↓), noise floor, minimum meaningful delta |
| **Lane** | One harness session working on its own branch `lane/<campaign>/<harness>` |
| **Candidate** | A commit SHA submitted by a lane, with artifacts and cost attributed |
| **Evaluator** | Pinned eval definition, run by the app on AWS with eval-only credentials |
| **Gates** | Hard pass/fail checks (tests, interface, leakage, latency ceiling) |
| **Ratchet** | Pointer to the best *confirmed* candidate; only moves forward |
| **Ledger** | Append-only record of every spend event, candidate and eval |

### Ratchet rule

A candidate moves the ratchet iff:

1. all gates pass, and
2. dev-split metric beats current best by more than `max(noise_floor, min_delta)`, and
3. confirmation on the **sealed** split also beats current best.

The noise floor comes from running the baseline `N` times (seeds / splits) at
campaign start.

## 4. Repository contract: `ratchet.yaml`

```yaml
version: 0
objective:
  metric: wape            # key in eval output JSON
  direction: minimize
  min_delta: 0.002
baseline:
  repeats: 3              # used to estimate the noise floor
eval:
  ref: ratchet/eval       # protected branch/tag holding the trusted eval
  image: python:3.12      # or a Dockerfile path
  setup: pip install -r requirements.txt
  command: python -m eval.run --split $RATCHET_SPLIT --out $RATCHET_OUT
  instance_type: m7i.xlarge
  timeout_minutes: 30
gates:
  - name: tests
    command: pytest -q
  - name: interface
    command: python -m eval.check_interface
protected_paths:          # changes here disqualify a candidate
  - eval/**
  - data/holdout/**
  - ratchet.yaml
agent:
  instructions: RATCHET.md   # task brief injected into every session
  allowed_instance_types: [m7i.xlarge, c7i.2xlarge, g5.xlarge]
```

The eval code is read from `eval.ref`, never from the candidate's tree, so a lane
cannot change how it is scored.

## 5. Harness adapters

```swift
protocol HarnessAdapter {
    var kind: HarnessKind { get }                 // .claudeCode, .codex
    func preflight() async throws -> Preflight    // installed, logged in, env configured
    func start(_ lane: LaneSpec) async throws -> SessionHandle
    func send(_ message: String, to: SessionHandle) async throws
    func events(_ session: SessionHandle) -> AsyncThrowingStream<LaneEvent, Error>
    func stop(_ session: SessionHandle) async throws
}
```

- **Claude Code**: start and steer cloud sessions; the session's environment
  holds the lane's AWS credentials and a network policy allowing AWS + GitHub.
- **Codex**: start cloud tasks against a Codex environment with the same
  credentials and an internet allowlist covering AWS and GitHub endpoints.

The app shells out to each vendor's CLI wherever possible, using the user's own
login, so v0 uses the subscriptions the way they are meant to be used.

> **To verify before building:** the exact supported commands / APIs for creating,
> messaging and streaming Claude Code cloud sessions and Codex cloud tasks from a
> third-party app; how each injects environment secrets; session duration and
> concurrency limits; and terms for programmatic use.

## 6. The improvement loop

Hybrid driver:

- **Self-loop (always):** each session gets `RATCHET.md` plus a generated
  `.ratchet/context.md` (objective, current best, experiment history, remaining
  budget, rules). The agent iterates on its own: hypothesize → change → run
  experiments on AWS → commit → submit.
- **App-steered (when the app is open):** after each eval, the app sends the
  lane a message with the official result and the current global best. Lanes
  may rebase onto the global best (a shared ratchet, with independent
  exploration).

**Submitting a candidate:** the agent pushes a commit to its lane branch with a
`Ratchet-Candidate: true` trailer. The GitHub watcher picks it up, checks
`protected_paths`, queues gates and eval.

**Stopping:** budget exhausted, deadline reached, `K` evals with no ratchet
movement, or a manual stop.

## 7. AWS design

### Infrastructure as code: OpenTofu + sandbox contract

Sandbox infrastructure is defined in **OpenTofu** (open-source, Linux
Foundation fork of Terraform; HCL; MPL-licensed, so safe to bundle). OpenTofu
does not make resources cloud-agnostic — an IAM role is not a GCP service
account — so cloud independence comes from a **Ratchet sandbox contract**: every
cloud module takes the same inputs and produces the same outputs, and the app
only depends on the contract.

```
infra/
  contract.md        # what every sandbox must provide
  aws/               # v0
  gcp/               # later
  azure/             # later
```

```hcl
# Inputs — identical for every cloud
variable "sandbox_id"             { type = string }
variable "region"                 { type = string }
variable "allowed_instance_types" { type = list(string) }

# Outputs — identical for every cloud; the app consumes only these
output "artifacts_bucket"   { value = ... }  # agent read/write, per campaign/lane prefix
output "sealed_bucket"      { value = ... }  # eval identity only
output "agent_boundary_ref" { value = ... }  # AWS: permission boundary ARN
output "eval_identity_ref"  { value = ... }  # AWS: eval role / instance profile ARN
output "network_ref"        { value = ... }  # AWS: subnet + security group
```

In Swift, a `SandboxProvider` protocol wraps the contract; `AWSSandboxProvider`
is the only v0 implementation.

**Split by lifetime:**

| Layer | Lifetime | Mechanism |
|---|---|---|
| Sandbox (buckets, boundary, eval identity, network) | Long-lived, one-time | **OpenTofu** |
| Per-lane credentials, kill switch | Minutes–hours, must be fast | Cloud SDK from the app (`CredentialMinter` protocol) |
| Experiment instances | Seconds–hours | Agents via cloud CLI, inside the boundary |
| Cost meter, reaper | Continuous | Cloud SDK from the app, filtered by tag |

Per-lane credentials stay out of OpenTofu: applies are too slow for them, and
the secrets would land in state.

**Operation:**

- The app bundles a pinned `tofu` binary; users don't install anything.
- Setup runs `tofu plan` and shows the resource diff in the app ("Ratchet will
  create these N resources") before `tofu apply`. Teardown is `tofu destroy`.
- `plan`/`apply`/`destroy` use the user's existing AWS profile or SSO login.
- State lives locally in
  `~/Library/Application Support/Ratchet/sandboxes/<id>/` for v0; an optional
  remote backend in the sandbox account (S3) for teams later.

### AWS sandbox module (v0)

Deployed into a **dedicated sandbox account**, `infra/aws` creates:

- `ratchet-artifacts-<id>` S3 bucket (agent read/write, prefixed by campaign/lane)
- `ratchet-sealed-<id>` S3 bucket (eval role only; holdout data)
- `RatchetAgentBoundary` permission boundary
- `RatchetEvalRole` and instance profile for app-launched eval instances
- Security group and subnet (or default VPC)

### Agent credentials (per lane)

- A campaign-scoped IAM user per lane, with an access key, constrained by
  `RatchetAgentBoundary`:
  - EC2 `RunInstances` only with tag `ratchet:campaign=<id>`, allowlisted
    instance types and region, and `InstanceInitiatedShutdownBehavior=terminate`
  - Start/stop/terminate only on instances carrying the same tag
  - S3 only under `ratchet-artifacts-<id>/<campaign>/<lane>/`
  - Explicit deny on the sealed bucket, IAM, billing, and everything else
- Injected into the vendor session environment at start.
- **Kill switch:** attach a deny-all policy (immediate), then delete the key.
- Deleted at campaign end.

*Why not STS?* Assumed-role credentials expire in 1–12h (1h when role-chaining
from SSO) and can't be refreshed inside a running vendor session without putting
secrets into the transcript. Revisit with a credential-vending endpoint once a
hosted controller exists.

### Cost metering

- **Live estimate:** poll tagged resources (EC2 instances, EBS volumes) every
  minute × on-demand/spot price → running spend per lane.
- **Reconcile:** Cost Explorer by `ratchet:campaign` tag after the fact (lags hours).
- **Guard:** at 80% projected spend, warn lanes; at 100%, kill switch plus reaper.

### Reaper

Terminates any tagged resource that is past its max lifetime, belongs to a
stopped lane, or exists after campaign end. Runs on app launch, on a timer, and
at teardown. An app-independent backstop (EventBridge schedule + Lambda in the
sandbox account) is a good addition for when the Mac is offline.

### Trusted eval runner

1. Watcher sees candidate SHA on a lane branch.
2. App launches an EC2 instance with `RatchetEvalRole`, which checks out
   candidate SHA for the code and `eval.ref` for the evaluator.
3. Runs gates, then eval on the dev split; if it qualifies, eval on the sealed split.
4. Writes result JSON to S3; app reads it, updates ledger and ratchet, and
   notifies the lane.

## 8. Mac app

**Stack:** SwiftUI, Swift Concurrency (one actor per lane), GRDB/SQLite for
ledger, Keychain for secrets, AWS SDK for Swift, a bundled `tofu` binary, `git` / `gh` and vendor CLIs as
subprocesses. Distributed via Developer ID + notarization, outside the
Mac App Store (the App Store sandbox would block spawning these CLIs).

**Modules:**

```
RatchetApp/
  Campaign/     CampaignEngine, Ratchet, StopRules
  Harness/      HarnessAdapter, ClaudeCodeAdapter, CodexAdapter
  GitHub/       BranchWatcher, PRBuilder
  Sandbox/      SandboxProvider, TofuRunner (plan/apply/destroy), AWSSandboxProvider
  AWS/          CredentialMinter, CostMeter, Reaper, EvalRunner
  Ledger/       Schema, Store
  UI/           NewCampaign, LiveLanes, RatchetTimeline, Spend, Result
```

**Screens:**

1. **New campaign:** repo, `ratchet.yaml` validation, harness preflight
   (logged in? environment ready?), budget and deadline.
2. **Live lanes:** one card per lane with streamed events, current hypothesis,
   spend, last eval.
3. **Ratchet timeline:** best metric over time with the noise band; every step
   links to a commit.
4. **Spend:** burn vs. budget, per lane, live estimate vs. reconciled.
5. **Result:** winning diff, eval report, "Open PR", teardown confirmation.

## 9. Milestones

1. **M0, skeleton:** app shell, `ratchet.yaml` parser, ledger, GitHub watcher.
2. **M1, AWS sandbox:** sandbox contract, OpenTofu AWS module, bundled `tofu` runner, credential minter, cost meter, reaper.
3. **M2, trusted eval:** eval runner, gates, noise-floor baseline, ratchet rule.
4. **M3, first lane:** Claude Code adapter end to end on a demo repo.
5. **M4, second lane:** Codex adapter; parallel lanes; shared ratchet.
6. **M5, demo:** a forecasting repo (e.g. M5 dataset) plus one perf benchmark,
   with a reported cost per validated improvement.

## 10. Open questions

1. Do vendor sessions allow reliable programmatic create/steer/stream for a
   third-party app? (This blocks M3/M4.)
2. Is a per-lane IAM user acceptable to target users, or do we need a
   credential-vending endpoint from day one?
3. Are agents allowed only EC2, or also Batch / SageMaker / ECS?
4. Agent token usage: shown as quota only, or counted against the budget?
5. Where does the trusted eval live: protected branch, separate repo, or S3?
