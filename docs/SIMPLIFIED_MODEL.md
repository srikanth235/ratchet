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
