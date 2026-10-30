# Ratchet v0 — Plan of action

Goal: an Electron app that runs Claude Code and Codex against a benchmark on
AWS for 8 hours unattended and hands back a verified, reproducible speed-up as
a PR. Architecture: [`V0_ARCHITECTURE.md`](V0_ARCHITECTURE.md).

Assumes 2 engineers, ~12 weeks. Phases overlap where noted.

## Phase 0 · De-risk (week 1)

Answer the questions that could change the architecture before writing product code.

| Spike | Question | Done when |
|---|---|---|
| **A. Agent sessions** | Can a third-party app create, message and stream Claude Code cloud sessions and Codex cloud tasks, inject env secrets and allow AWS egress? Terms OK? | Go/no-go memo per vendor, with working script |
| **B. Bench noise** | How noisy is k6 p95 on `c7i.2xlarge` dedicated? | 10 baseline runs → coefficient of variation; pick `repeats` and `min_gain` |
| **C. IAM boundary** | Can an agent key be fenced to tagged EC2 + its S3 prefix only? | Policy passes `simulate-principal-policy` tests incl. "can't read sealed bucket" |

**Fallback if A fails:** run `claude -p` / `codex exec` headless on a
Ratchet-launched EC2 runner inside the sandbox (same adapter interface).

## Phase 1 · Skeleton — M0 (weeks 2–3)

- pnpm monorepo: `core`, `cli`, `desktop`, `ui`; strict TS, ESLint, Vitest, CI.
- `ratchet.yaml` schema (zod) + `ratchet validate`.
- Ledger schema in SQLite (optimizations, agents, candidates, results, spend).
- Electron shell, secure preload/IPC, Midnight Brass tokens in `ui`.
- Optimizations screen on mock data.
- Demo repo `ratchet-demo-search`: small HTTP search service with deliberate
  slow paths, k6 bench, tests, OpenAPI contract.

**Exit:** `ratchet validate` passes on the demo repo; app opens to screen 1.

## Phase 2 · AWS sandbox — M1 (weeks 3–5)

- `infra/contract.md` + `infra/aws` OpenTofu module (7 resources).
- `TofuRunner` (bundled binary; plan → diff UI → apply / destroy).
- `CredentialMinter` (per-agent IAM user + key, deny-all kill switch).
- Cost meter (tagged resources × price), budget guard (80% warn, 100% stop).
- Reaper (on launch, timer, teardown).
- AWS sandbox screen wired to real plan output.

**Exit:** apply → mint key → launch tagged instance → kill switch → reaper
leaves zero resources; destroy is clean. Run in a test account in CI nightly.

## Phase 3 · Trusted bench — M2 (weeks 5–7)

- Bench runner: pinned instance, checkout candidate + `bench.ref`, guardrails,
  N runs, result JSON to S3.
- Baseline + noise floor; hidden-trace confirm on a fresh instance.
- Ratchet rule (verified / rejected / failed confirm / blocked).
- `ratchet bench <sha>` from the CLI.
- Benchmark history screen on real results.

**Exit:** hand-written optimization of the demo repo becomes a verified win; a
change that edits `bench/**` is blocked; a noise-level change is rejected.

## Phase 4 · First agent — M3 (weeks 7–9)

- `AgentAdapter` + Claude Code implementation (per Spike A).
- `RATCHET.md` brief + generated `.ratchet/context.md`.
- Branch watcher, `Ratchet-Candidate` trailer, queue to bench runner.
- App-steered loop: post verdict + current best back to the agent.
- Live agents screen; stop rules (budget, time, plateau).

**Exit:** Claude Code alone reaches ≥1 verified win on the demo repo in a
2-hour unattended run.

## Phase 5 · Second agent — M4 (weeks 9–10)

- Codex adapter; two agents in parallel; shared ratchet (rebase onto best).
- Per-agent spend and results; bench queue fairness.

**Exit:** 8-hour run, both agents, within budget, no manual intervention.

## Phase 6 · Ship v0 — M5 (weeks 11–12)

- Result screen, PR builder (squash verified commits + report), report export.
- New optimization flow end to end; preflight checks.
- Teardown checklist, Cost Explorer reconcile.
- Signed + notarized macOS build, auto-update.
- Security review (keys never in renderer, sealed bucket deny, IPC surface).
- Second demo: CLI cold start with hyperfine.
- Pilot with 2–3 design partners on their own repos.

**Exit:** v0 success criteria below met on both demos and at least one partner repo.

## v0 success criteria

1. 8 h unattended run ends with a PR and benchmark report.
2. ≥1 verified win per demo; **cost per verified win** reported.
3. Zero orphaned AWS resources after teardown (checked by the reaper and a
   tag sweep).
4. Tested: no agent identity can read sealed traces or edit protected paths
   and still win.
5. Spend never exceeds budget by more than one metering interval.

## Risks

| Risk | Mitigation |
|---|---|
| Vendor session APIs unavailable or unstable | Spike A first; EC2 headless-runner fallback behind the same adapter |
| Benchmark noise hides real gains | Spike B; dedicated hosts, more repeats, confirm runs |
| Agents game the benchmark | Bench from protected ref, sealed hidden trace, guardrails, protected paths |
| Cost overrun | IAM limits on types/count/lifetime, live meter, kill switch, reaper |
| IAM setup scares users | Plan diff in the app, dedicated sandbox account, one-click destroy |

## Week 1 checklist

- [ ] Spike A script for Claude Code cloud sessions
- [ ] Spike A script for Codex cloud tasks
- [ ] Spike B: 10 baseline k6 runs on `c7i.2xlarge`
- [ ] Spike C: agent boundary policy + simulator tests
- [ ] Decision memo; update architecture doc if anything changes
