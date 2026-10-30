# Real-environment testing

Everything in v0 is written against **ports** (`packages/core/src/ports`). Each
port has a real implementation and a mock/local one. In `simulation` mode the
whole product runs on one machine: mock AWS, a scripted agent, a local git
remote and a local bench runner against `examples/demo-search`.

This file is the checklist for turning simulation into a real run. Each area
keeps its own detailed notes in `docs/real-env/<area>.md`.

| Port | Real implementation | Mock / local | Needs for real test |
|---|---|---|---|
| `SandboxProvider` | `@ratchet/aws` TofuRunner (`tofu` CLI) | `MockSandboxProvider` | Sandbox AWS account, `tofu` binary |
| `CloudCompute` | `@ratchet/aws` (EC2, IAM, SSM) | `MockCloudCompute` | Sandbox account, runner AMI |
| `PriceBook` | `@ratchet/aws` (Pricing API) | static price table | Pricing API access |
| `ArtifactStore` | `@ratchet/aws` (S3) | filesystem | Artifacts bucket |
| `GitHost` | GitHub (git + Octokit) | local bare repo | Repo-scoped GitHub token |
| `AgentAdapter` | `@ratchet/agents` claude-code, codex | `ScriptedAgent` | **Local (default):** the CLIs installed and signed in on the test machine. **Cloud:** Anthropic / OpenAI API keys, CLIs on runner image |
| `RunnerControl` | **Local (default):** `LocalRunnerControl` in worktree mode, one working tree per agent. **Cloud:** SSM + S3 event stream | `LocalRunnerControl` (clone mode), local child process | Local: only the CLIs and a repo you can push to. Cloud: runner AMI, SSM agent |
| Harness detection | `detectHarnesses` (login-shell PATH, `claude auth status`, `codex login status`) | fake spawner / fs | A Dock-launched app on macOS, a signed-in and a signed-out machine |
| Sandbox lifecycle | Engine `sandbox.auto`: created at the first run, destroyed after the idle window, with a teardown receipt | `MockSandboxProvider` + fake clock | AWS account: apply at first start, destroy after the idle window, nothing left (`aws.md`) |
| `BenchRunner` | EC2 bench instance via SSM | `LocalBenchRunner` | Dedicated `c7i.2xlarge`, hidden traces in sealed bucket |

## Order of real tests

0. `docs/real-env/agents.md`, "Local agents with your own login" — detection and a local agent turn, no AWS needed.
1. `docs/real-env/aws.md` — tofu apply/destroy, identities, kill switch, reaper.
2. `docs/real-env/bench.md` — baseline noise on pinned hardware, confirm trace.
3. `docs/real-env/agents.md` — headless Claude Code / Codex on a runner (the `cloud` mode).
4. `docs/real-env/engine.md` — full optimization on demo-search, then a partner repo.
5. `docs/real-env/desktop.md` — signed build on macOS, every screen against live data.

## Local agents and the invisible sandbox (what is new, what is unproven)

`createHybridRuntime` (`@ratchet/runtime`) is the product wiring: agents on this machine through `LocalRunnerControl`, the
evaluator on AWS, and the sandbox created and destroyed by the engine. Its tests run without AWS or the real CLIs: the engine
with the fake clock (idle window, receipt, orphans, restart), detection against fake spawners and shell stand-ins, and one
integration test (`packages/runtime/src/local-agents.int.test.ts`) that runs a stand-in `claude` through the real Engine, a
worktree and a mock sandbox. Unproven until run for real: the real CLIs' flags and auth status output on a developer machine
(`agents.md`), `tofu apply` at the first run start and `tofu destroy` after the idle window (`aws.md`), and the late-bound AWS
ports (built from the sandbox outputs in the ledger on first use). Sleep prevention is an engine signal (`run-active` event,
`engine.runActive()`); the desktop main process has to turn it into `powerSaveBlocker`, which nothing here exercises.

## Security caveats (read before the first apply)

The sandbox isolates the bench: it runs in a private subnet with no internet route, reachable only through S3 and SSM endpoints, and the VPC resolver answers only an allow-list of names (Route 53 DNS Firewall). Bench and confirm runs use separate identities, and only confirm can read `holdout/`. On the instance there are two unprivileged users: candidate code (setup, guardrails and every process of the candidate) runs as `ratchet-bench`; the trusted evaluator harness runs as `ratchet-holdout` and is the only user that can open the holdout, which is fetched by root after the build, per run, and wiped afterwards. Both users have IMDS blocked. Inputs are staged as tarballs under `bench-inputs/` and expire after `bench_inputs_retention_days`. Packages for candidate setup arrive as a dependency snapshot (`bench-deps/`) built on a separate short-lived instance in the agent subnet that holds a third identity (`deps_runner`: writes `bench-deps/` only, no sealed access) and never runs candidate code; a candidate that changes the lockfile is blocked (`bench.md`, "Dependencies on an offline bench"). `createLiveRuntime` derives all of this from the tofu outputs (`awsComputeConfigFromOutputs`), and a missing piece fails closed. Nothing below has been exercised on a real AWS account yet; the privilege split (users, sudo rule, launcher, file modes) was checked on a real Linux host, the rest is unit-tested against shims.

| Risk | Status |
|---|---|
| Candidate code and the holdout trace share an OS user on the confirm instance | **Mitigated in code**, verify on the AMI (`bench.md` section 3). The holdout is a `0600` file owned by `ratchet-holdout` in a `0700` directory, fetched by root only after setup and guardrails and wiped after each run; the harness runs as that user from a root-owned read-only snapshot and starts candidate processes as `ratchet-bench` through a sudo launcher with a clean environment. **Remaining:** (1) an evaluator that imports candidate code in-process, or executes candidate-built files, runs that code as `ratchet-holdout`, so the data is exposed to it: packs must keep the harness in `protected` paths and talk to the candidate over a process boundary; (2) the candidate sees the requests/inputs the harness sends it, and can steer numbers it influences, so the result channel (metrics, timing) is still a low-bandwidth leak; (3) the image must provide `sudo`, `setpriv`, `iptables`, and a world-readable toolchain |
| DNS tunnelling from the bench subnet via the VPC resolver | **Mitigated** by `infra/aws/dns_firewall.tf` (on by default): NXDOMAIN for everything except the regional S3/SSM/ssmmessages/ec2messages/logs names, the VPC's internal names and the agent domain allow-list. **Remaining:** association is per VPC, so agent runners are under the same allow-list (add package registries with `dns_firewall_extra_domains`); allowed names still resolve (they are answered by AWS, so they leak nothing to an attacker-controlled name server); IP-literal connections are the security groups' job; verify with the drill in `bench.md` |
| Root escape on the bench instance (kernel / sudo misconfig) | **Open.** Mitigated only by a short instance lifetime and a single-purpose AMI. A root compromise reads the holdout and the instance-profile credentials; the sudo rule widens the surface slightly (one rule, one fixed script) |
| Egress proxy is a single instance (when `enable_egress_proxy`) | **Open**, SPOF: if it fails, agents lose model access; off by default |
| The dependency-snapshot instance has internet egress and handles downloaded packages | **Mitigated by construction, verify on AWS** (`bench.md` section 4b). It runs only the package manager's fetch step (`pnpm fetch` / `npm ci --ignore-scripts`: no lifecycle scripts) on the trusted base ref's lockfile, as an unprivileged user with IMDS blocked and an empty environment; root downloads the base tree and packs what that user wrote; its identity writes `bench-deps/` only and is explicitly denied on the sealed bucket; it is terminated in a `finally` and covered by the reaper and the teardown orphan check. **Remaining:** a malicious package in the *base* lockfile is downloaded (not executed) and lands in the snapshot every bench restores, so the base ref's dependencies are trusted exactly as much as the base ref itself; the registry must be allowed for the whole agent path (proxy/DNS lists are shared with agents); private registries are not supported |
| `bench-inputs/` tarballs accumulate | **Mitigated:** lifecycle rule `expire-bench-inputs` (`bench_inputs_retention_days`, default 7, noncurrent versions included). Check after the first run that the rule is `Enabled` on the bucket |

## Forecasting pack (`examples/demo-forecast`)

Simulated only, on synthetic weekly sales. Scripted agents take WAPE from 43.5 to about 24 with 5 verified wins; a leakage attempt and an evaluator-file edit are `blocked`, and per-window calibration that overfits the backtest is `failed_confirm` on the holdout. Before a real run:

- Use a real dataset: the holdout must be the most recent weeks, staged in the sealed bucket, never in the repo.
- The leakage guardrail checks feature timestamps. It does not detect target leakage hidden in derived columns of a real dataset; review the dataset's columns first.
- The `value` model ($ per WAPE point) is a placeholder; set `basis: measured` only once finance signs off on the figure.
- Backtests are deterministic, so `repeats: 2` is enough; the noise floor comes from fold variance, not machine noise. Check that it stays above zero on real data.
