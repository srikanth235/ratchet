# Real-environment test plan: engine

> **Written for the TypeScript engine.** Names such as `@ratchet/...`, `packages/...`, `createLiveRuntime` and the `.mjs`/`node` snippets refer to
> the code at commit `fc503b1`; the Rust replacement is `crates/ratchet-engine` (see [CUTOVER.md](../CUTOVER.md)). What to verify on a real account, and why,
> still applies; re-express the snippets with `ratchetd` / `ratchet` before using them.

Scope: `@ratchet/engine` (the optimization engine: baseline, agent launch, control loop,
ratchet, cost guard, kill switch, reaper, teardown, PR). Unit tests run it against in-memory
fakes of every port; nothing in them proves the real ports behave the way the engine assumes.
This plan runs the same engine against progressively more real environments.

Prerequisite docs, in order: `aws.md` (sandbox, identities, kill switch, reaper), `bench.md`
(noise on pinned hardware, hidden trace), `agents.md` (headless agents on a runner). Do not start
here until those pass; a failure in this plan should then point at the engine, not a port.

Success criteria come from `docs/PLAN.md` (Phase 3 to 5 exits and the "v0 success criteria").
They are numbered S1..S5 below and referenced by each stage.

| # | Criterion (docs/PLAN.md) |
|---|---|
| S1 | An 8 h unattended run ends with a PR and a benchmark report |
| S2 | At least one verified win per demo; cost per verified win reported |
| S3 | Zero orphaned AWS resources after teardown (reaper and a tag sweep) |
| S4 | No agent identity can read sealed traces or edit protected paths and still win |
| S5 | Spend never exceeds budget by more than one metering interval |

## Observability used by every stage

- Ledger (SQLite): `candidates`, `bench_results`, `ratchet_moves`, `spend_events`, `agent_events`
  tell the whole story. `engine_kv` holds engine bookkeeping (cursors, launch steps, stop mode).
- `engine.errors` (and `settings.onError`): swallowed non-fatal errors. An empty list at the end of
  a clean run is part of the pass condition; read it.
- Per-tick cost: `spend_events` grouped by `category` and `source` (`ec2:<instance>`, `tokens:<agent>`).
- AWS tag sweep: list every resource tagged for the optimization (EC2 instances in any non-terminated
  state, IAM roles/profiles with the `ratchet/<opt>` path, SSM parameters under `/ratchet/<opt>/`).

Useful queries:

```sql
-- ratchet is monotonic and every move has a verified candidate
SELECT m.id, c.sha, a.kind, m.from_value, m.to_value FROM ratchet_moves m
  JOIN candidates c ON c.id = m.candidate_id JOIN agents a ON a.id = c.agent_id ORDER BY m.id;
-- every verified candidate has bench AND confirm results, and none is blocked-but-benched
SELECT c.sha, c.verdict, group_concat(r.phase) FROM candidates c
  LEFT JOIN bench_results r ON r.candidate_id = c.id GROUP BY c.id;
-- spend by category (estimated vs reconciled)
SELECT category, estimated, round(sum(usd), 2) FROM spend_events GROUP BY 1, 2;
```

## Stage 1: simulation run (one machine, no AWS)

Goal: shake out engine logic with the real `Ledger`, `LocalBenchRunner`, local bare git remote and
a scripted agent, on `examples/demo-search`. Costs nothing, runs in minutes. Mode: `simulation`.

Setup: `Engine({ ledger: openLedger(file), mode: 'simulation', clock: realClock, ports: <mock set>,
settings: { tickIntervalMs: 2000, secrets: () => 'x' } })`; two `mock` agents (one scripted to make
real improvements from `examples/demo-search/SOLUTIONS.md`, one scripted to edit `bench/**`).

Checks:

1. Create + start: baseline recorded (value = median of `repeats`, noise floor = 2 x stddev),
   branches `ratchet/<opt>/<agent>` exist on the local remote, runners started, status `running`.
2. The scripted winner is `verified` (bench + confirm results both in the ledger), the ratchet moves
   monotonically, the other agent receives a `verdict` message announcing the new shared best.
3. The agent that touches `bench/**` is `blocked` and never benched (no `bench_results` rows).
4. A noise-level change is `rejected`; a change that wins on the bench trace but not on the hidden
   trace is `failed_confirm` (use a candidate that overfits the benchmark query mix).
5. Stop rules, one run each with small limits: budget (fake price high), deadline (`timeLimitSeconds: 120`),
   plateau (`plateauEvals: 3`). Each ends `stopped` with the right `engine.stopReason`.
6. Crash drill: `kill -9` the process mid-run (during a bench, and once between bench and confirm),
   start a new process on the same ledger file. Expect: no duplicate `agent_events`, the interrupted
   candidate is re-benched or confirmed and gets a verdict, ratchet history unchanged.
7. Open the PR against the local remote; the body lists the verified commits in ratchet order.
8. `teardown`: status `completed`, mock compute reports zero tagged instances.

Pass: S2 on the simulated demo, S5 against the mock meter, no entries in `engine.errors` except the
ones the drills provoke on purpose.

## Stage 2: live run on demo-search (real AWS sandbox, real agents)

Mode: `live`. Real ports: `@ratchet/aws` (compute, prices, artifacts, sandbox), `@ratchet/agents`
runner control (SSM + S3 events), `@ratchet/bench` EC2 bench runner with the sealed trace,
`@ratchet/github` against a throwaway fork of `ratchet-demo-search`.

Run these as separate runs, cheapest first. Keep the AWS billing console open; the engine's meter is
an estimate, so compare it with Cost Explorer next day (the ~24 h lag) and record the error.

| Run | Config | Purpose | Exit (docs/PLAN.md) |
|---|---|---|---|
| 2a. Drills | budget $5, 20 min, 1 agent | kill switch, budget trip, reaper, teardown with real AWS | S3, S5 |
| 2b. Claude alone | budget $60, 2 h, `claude-code` | first verified win; Mac asleep for ~30 min | Phase 4: at least one verified win, unattended |
| 2c. Both agents | budget $300, 8 h, `claude-code` + `codex` | full v0 run | Phase 5, S1, S2, S5 |

Drills in 2a (do all of them; each must leave zero tagged resources):

1. **Budget trip.** Set the budget so it trips within ~10 minutes. Expect: warning event at 80%, then
   `stopped`; in CloudTrail the `revoke` (deny-all attach) precedes `runners.stop`, which precedes
   `TerminateInstances`. Total spend minus budget is at most one tick of agent-runner cost (S5).
2. **Manual kill switch** while a bench job is mid-run: the bench is aborted, the candidate stays
   `pending` (not mis-scored), every instance is terminated.
3. **Launch an experiment from the agent runner** (agent role via the instance profile), then stop:
   the reaper terminates it; kill the runner's role first and confirm the agent can no longer call EC2.
4. **Pause / resume / message** each agent from the engine API; the runner log shows the message, a
   paused agent's events and candidates are still collected.
5. **Reaper backstop.** Launch a tagged instance by hand with the optimization tag after `stopped`; the
   next tick terminates it. Launch an experiment older than the max lifetime; it is terminated while running.
6. **Teardown.** SSM parameters gone, role and instance profile deleted, tag sweep empty, status
   `completed`. Make one terminate fail on purpose (deny it in a test policy): `teardown` must throw
   `orphans` and leave status `stopped`.
7. **Engine restart.** `kill -9` the engine process (or close the lid) mid-run; relaunch on the same
   ledger. The runners kept working meanwhile; cursors resume, no gap or duplicate in `agent_events`
   beyond what the runner itself re-sent.

Run 2b/2c specifics:

- S4: before the run, plant a candidate (hand-pushed commit on an agent branch) that edits `bench/**`
  and one that reads like a win but fails the contract tests. Expect `blocked` for both. From the
  agent runner, `aws s3 ls` on the sealed bucket must be denied (that check lives in `aws.md`; re-run
  it from the runner of this very optimization).
- S1: after the run, `openPullRequest` produces a PR whose commits are exactly the ratchet history,
  and the report shows baseline -> best, spend by category and by agent, and cost per verified win.
- S2: compute cost per verified win as total spend / `ratchet_moves` count; record it per agent.
- Expect noise: with the baseline noise floor from `bench.md`, no `rejected` candidate should have
  an improvement above `requiredGain` in its reasons, and no `verified` one below it.
- Deadline behavior: in 2c the engine must stop gracefully at the time limit: runners stop, queued
  candidates are still evaluated (up to `drainTimeoutMs`), then identities revoked, instances terminated.
- Record wall-clock per phase (baseline, each bench, each confirm) to size `tickIntervalMs` and
  `drainTimeoutMs`; the default tick is 15 s, so the overshoot bound for S5 is 15 s of full spend rate.

Abort criteria: any orphan after teardown, any spend above budget plus one interval, any verified
candidate lacking a confirm result, any ratchet move that is not strictly better. Stop and fix before continuing.

## Stage 3: partner repo

Only after 2c passes. Same setup against a design partner's repository, with their `ratchet.yaml`.

1. Dry run `ratchet validate` and a baseline-only run first (`createOptimization` + `start`, then `stop`):
   check noise floor and `repeats` make sense for their benchmark. A baseline coefficient of variation
   above roughly 5% means the benchmark is too noisy for `min_gain: 0.03`; fix that before spending on agents.
2. Confirm their `protected` list covers the benchmark, contract tests and anything that defines
   correctness, and that guardrail commands pass on the baseline (the engine fails the start otherwise).
3. Small first run (budget about $50, 2 h, one agent) with someone from the partner watching the Live
   agents screen; then the full 8 h unattended run.
4. Review the PR with them: every commit has a bench and a confirm result, none touches protected paths,
   the report numbers match their own re-run of the benchmark.

Pass: Phase 6 exit "success criteria met on both demos and at least one partner repo", with S3 and S5
checked again on the partner's account.

## Known unverified assumptions (check on the first live run)

- Runner `usage` events are incremental (not cumulative) per event; the engine adds them up. If a
  runner reports running totals, token spend will be overcounted: check against the provider invoice.
- A `status` event with `data.state` of `done`, `failed`, `paused` or `running` is how a runner reports
  its own state. If runners use another convention, "all agents finished" will never trigger.
- `GitHost.listCandidates(branch, sinceSha)` is stable when the agent rebases or force-pushes (a
  rewritten branch no longer contains `sinceSha`). The engine dedupes by sha, but a rewrite could
  re-list old candidates; they are then ignored, and genuinely new ones after a rewrite are found only
  if the host handles the missing `sinceSha` by returning everything.
- EC2 keeps terminated instances in `describe-instances` for about an hour, which the cost meter
  relies on to charge the tail of a run. If an instance disappears from `listTagged` between ticks,
  the last partial interval of that instance is lost (bounded by the tick interval).
- `listTagged(optimizationId)` returns instances with `state: 'terminated'` for recently terminated ones;
  teardown treats only non-terminated ones as orphans.
- The hidden-trace baseline (an extra `confirm` job on the base ref at start, `confirmBaseline`) works
  with the real bench runner: it needs the trace reachable and costs one extra fresh instance.
- Shared-ratchet announcements use the `ratchet` runner message (`sha`, `branch`, `value`) sent to
  the other runners; the winner's own runner gets its `verdict`. The runner loop must fetch that branch
  and rebase onto the sha.
- `baseRef` is resolved once with `GitHost.resolveRef` and remembered; baseline, confirm-baseline,
  branch creation and protected-path diffs all use that sha. The budget warning is a `notice` event.
