# Simplified product model (v0.2)

Decision record for the simplification agreed after the first full build. Where this file and
`V0_ARCHITECTURE.md` disagree, this file wins until the architecture doc is updated.

## Principles

1. **Settings hold who you are; the repo holds what to optimize.** Nothing about a specific
   repository (repo name, toolchain, machine, infra) is a global setting.
2. **The repo declares what it needs; Ratchet owns how it is isolated.** Agents edit the repo, so
   anything security-relevant in it is read from the protected ref, and isolation (network, IAM,
   images, the OpenTofu module) ships with the app.
3. **Agents run where they already are.** The Claude Code / Codex CLIs installed and signed in on
   the user's machine are detected and used. Only the evaluator must run in the trusted cloud sandbox.
4. **No knobs the system can derive.** Statistics, protected paths, egress, dependency handling and
   teardown are automatic.

## Settings: three rows

| Row | How | Replaces |
|---|---|---|
| GitHub | "Sign in with GitHub" (GitHub App / device flow). Token kept in the OS keychain | repo field + pasted token |
| AWS | "Connect AWS": pick an SSO / profile from `~/.aws/config` (listed, not typed) or a one-click CloudFormation role. Region defaults to `us-east-1`; `ratchet.yaml` can override | profile/region/sign-in fields |
| Agents | Detected harnesses with version and sign-in status; an API key field appears only for a harness that is not signed in, or for cloud runners | always-shown model key fields |

Removed from Settings: GitHub repository, runner/bench AMI ids, OpenTofu binary and module paths,
the simulation/live toggle. Images and the module are versioned with the app (`infra/aws`,
published AMIs looked up by tag `ratchet:image` + app version; an explicit override stays in an
"Advanced" disclosure for development only).

## Harness detection

The main process probes on launch and on demand (never the renderer):

| Harness | Installed | Signed in |
|---|---|---|
| Claude Code | `claude --version` | `claude` auth status (OAuth credentials present) or `ANTHROPIC_API_KEY` |
| Codex | `codex --version` | `codex login status` / `~/.codex/auth.json`, or `OPENAI_API_KEY` |

PATH is resolved the way a login shell would (GUI apps on macOS do not inherit the shell PATH).
Each result: `{ kind, installed, version, path, auth: 'signed-in' | 'api-key' | 'signed-out', hint }`.
The agent picker pre-selects detected, signed-in harnesses and shows install / sign-in hints for the rest.

## Where agents run

- **This machine (default):** each agent works in its own git worktree of a local clone, driven by
  the same runner loop (`runRunnerLoop`) through `LocalRunnerControl` with the real
  `ClaudeCodeAdapter` / `CodexAdapter`. Uses the user's installed CLIs and login. The app prevents
  system sleep while a run is live.
- **Cloud (unattended):** the existing agent-runner path (EC2 runner, SSM, `ratchet-runner`),
  for long runs with the laptop closed. Needs API keys.

Either way the **evaluator runs in the cloud sandbox** (bench + confirm instances), so measurements
are on pinned hardware and the holdout never touches the agents' machine.
Simulation remains for tests and demos; in the app a demo is just a template run with mock agents.

## `ratchet.yaml`: the only file a repo needs

```yaml
version: 0
objective: { metric: http_req_duration.p95, direction: minimize }
evaluator:
  template: http-latency              # or a custom `run:`; templates fill run/holdout/guardrails
environment:                          # all optional
  scope: full                         # none | single | full: decided by the setup agent, see "Dynamic environments"
  image: .devcontainer/Dockerfile     # toolchain for setup, guardrails and the evaluator
  machine: c7i.2xlarge                # bench hardware; default from the template
  region: us-east-1
  network: [registry.npmjs.org]       # extra egress for agents / the dependency job
```

- `environment.image`: the repo's container (Dockerfile or devcontainer) replaces per-repo AMIs.
  The base bench image is generic (Docker + SSM); the container is built by the dependency job
  (which has egress) from the protected ref, saved to S3, and loaded offline on bench instances.
- **Auto-calibrated statistics:** `repeats`, `min_gain` and the noise factor become optional.
  Defaults come from the baseline (coefficient of variation): enough repeats to resolve a gain of
  `max(2 × CV, 1%)`, shown to the user in plain words ("this metric is noisy (±9%); wins must
  beat 10%").
- **Automatic protected paths:** `ratchet.yaml`, the evaluator's files (the template's or those
  `run:` references under `bench/` / `eval/`), lockfiles and install config are protected without
  being listed. `protected:` only adds to them.
- **Value model is optional and late:** `objective.value` can be set from the Result screen.
- **Advanced infra override:** `infra: <path>` points at a custom OpenTofu module; read from the
  protected ref only, validated against the sandbox contract, off by default.

## Evaluator templates (Benchmarks tab → gallery)

| Template id | Measures | Holdout | Machine |
|---|---|---|---|
| `http-latency` | k6 p50/p95/p99, error rate | hidden query mix | c7i.2xlarge |
| `cli-cold-start` | hyperfine wall time | hidden invocations | c7i.large |
| `test-suite-time` | test command duration | - | c7i.2xlarge |
| `build-size` | build seconds / bundle KB | - | c7i.xlarge |
| `memory` | peak RSS | hidden workload | c7i.xlarge |
| `forecast-backtest` | WAPE / MAPE / bias | most recent periods | c7i.xlarge |
| `llm-eval` | score / cost per call | hidden eval set | c7i.large |

A template = `ratchet.yaml` fragment + evaluator files + a `RATCHET.md` brief. **Use template** → pick
a repo → Ratchet opens a PR on it adding those files, with a dry-run baseline in the PR description.
`ratchet init --template <id>` does the same from the CLI (the `--pack` flag stays as an alias).
The demos (`demo-search`, `demo-cli`, `demo-forecast`) are listed as "Try a demo" templates that run
in simulation with no setup. "Packs" stop being a user-facing concept; they are template categories.

## New optimization: three steps

1. **Repo:** picker over the signed-in GitHub account's repos (search), plus "local folder" for
   this-machine runs. Ratchet reads `ratchet.yaml` from the default branch; if missing, offers
   templates (opens the PR flow).
2. **Review:** what will be measured, the toolchain, the machine, the estimated cost per hour; the
   detected agents (pre-selected) and where they run (This machine / Cloud).
3. **Budget & time:** defaults $50 / 2 h; Start.

## Sandbox lifecycle: invisible

Created on first run in a region (plan shown once, first time only), reused while runs are active,
destroyed automatically after the last run ends plus an idle window (default 30 min), with the
orphan check. Environments becomes read-only: what is running, what it costs, last teardown receipt.
Teardown checklist → a receipt on the Result screen.

## Dynamic environments

Each optimization's environment is sized by a **setup agent** before the run, instead of one fixed
sandbox for everyone. `environment.scope` records the decision:

| scope | what is created | typical goal |
|---|---|---|
| `none` | nothing extra: the evaluator runs pure code/tests in its container | unit tests, lint, build graph |
| `single` | one machine on pinned hardware (`environment.machine`, optional `environment.image`) | a CLI, a build, one service |
| `full` | app + Postgres/Redis/etc. + seeded data + a separate load generator, as OpenTofu | an API's p95 under load |

The flow (UI: `design/v02.html#s4`):

1. **Draft.** The user's own Claude Code / Codex (local by default, `packages/agents` adapters) reads the
   repo and the goal and writes only the environment: `environment.scope` in `ratchet.yaml` plus files under
   `infra/ratchet/`. A `full` draft's `main.tf` instantiates the base module `ratchet/sandbox`
   (`infra/modules/sandbox`, built on `infra/aws`), which owns placement (private bench subnet, no route out),
   the evaluation-only identity, encrypted disks, hardened metadata and budget tags. The draft chooses machines,
   images and seed scripts, never networking or IAM.
2. **Validate and price.** Ratchet checks the draft (`validateEnvironment`): only the environment is touched
   (`ratchet.yaml` may change only `environment`, no new egress hosts); no internet on measuring hosts; agents
   can read neither results nor the holdout; one machine type for baseline and candidates; everything tagged;
   full teardown; the scope fits the files; every resource priced. `tofu validate` + `tofu plan` then run in the
   selected AWS account (`TofuEnvironmentChecker`); a draft is never shown as planned if no plan ran. The cost
   (`estimateMachinesCost`, static price table or live `PriceBook`) is $/h per resource, start-up minutes, and an
   estimate for the run's time limit.
3. **Revise.** The user asks for changes in plain words ("smaller", "use RDS"); the same agent session continues
   and the draft is re-validated. (Managed services that need a second subnet are explained, not added.)
4. **Approve.** Files are committed to the protected ref, directly or as a pull request, and are **locked**:
   once `environment.scope` is set, `infra/ratchet/**` and `ratchet.yaml` are automatic protected paths, so any
   candidate touching them is blocked without spending a bench run. Approval refuses a draft with a failed check
   or a `ratchet.yaml` that changed since the draft.
5. **Run.** At run start the engine reads the environment from the protected ref (pinned to one commit),
   re-validates it, and passes it to the sandbox provider with the sandbox input (`infra:` custom modules take
   the same path). Optimizations share a sandbox only when region and environment hash match. After the last
   run plus the idle window the environment is destroyed first, then the sandbox, and the receipt lists it.

In code: `EnvironmentDrafter` (`draft` / `revise` / `approve`) in `@ratchet/core`; `AgentEnvironmentDrafter`
(real) and `MockEnvironmentDrafter` (deterministic per scope, for simulation, demos and e2e) in
`@ratchet/engine`; `Engine.draftEnvironment / reviseEnvironment / approveEnvironment`.
