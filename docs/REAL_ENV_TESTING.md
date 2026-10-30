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
| `AgentAdapter` | `@ratchet/agents` claude-code, codex | `ScriptedAgent` | Anthropic / OpenAI API keys, CLIs on runner image |
| `RunnerControl` | SSM + S3 event stream | local child process | Runner AMI, SSM agent |
| `BenchRunner` | EC2 bench instance via SSM | `LocalBenchRunner` | Dedicated `c7i.2xlarge`, hidden traces in sealed bucket |

## Order of real tests

1. `docs/real-env/aws.md` — tofu apply/destroy, identities, kill switch, reaper.
2. `docs/real-env/bench.md` — baseline noise on pinned hardware, confirm trace.
3. `docs/real-env/agents.md` — headless Claude Code / Codex on a runner.
4. `docs/real-env/engine.md` — full optimization on demo-search, then a partner repo.
5. `docs/real-env/desktop.md` — signed build on macOS, every screen against live data.

## Security caveats (read before the first apply)

The sandbox isolates the bench: it runs in a private subnet with no internet route, reachable only through S3 and SSM endpoints. Bench and confirm runs use separate identities, and only confirm can read `holdout/`. The bench process runs as the unprivileged `ratchet-bench` user with IMDS blocked. Inputs are staged as tarballs under `bench-inputs/`. `createLiveRuntime` derives all of this from the tofu outputs (`awsComputeConfigFromOutputs`), and a missing piece fails closed. Known residual risks, none tested yet:

| Risk | Status |
|---|---|
| Candidate code and the holdout trace share an OS user on the confirm instance | Open: candidate code could read the trace. Planned fix: a separate user, or fetch the trace after the build |
| DNS tunnelling from the bench subnet via the VPC resolver | Open: needs Route 53 DNS Firewall |
| Root escape on the bench instance (kernel / sudo misconfig) | Mitigated only by a short instance lifetime and a single-purpose AMI |
| Egress proxy is a single instance (when `enable_egress_proxy`) | SPOF: if it fails, agents lose model access; off by default |
| `bench-inputs/` tarballs accumulate | Needs an S3 lifecycle rule; check after the first run |

## Forecasting pack (`examples/demo-forecast`)

Simulated only, on synthetic weekly sales. Scripted agents take WAPE from 43.5 to about 24 with 5 verified wins; a leakage attempt and an evaluator-file edit are `blocked`, and per-window calibration that overfits the backtest is `failed_confirm` on the holdout. Before a real run:

- Use a real dataset: the holdout must be the most recent weeks, staged in the sealed bucket, never in the repo.
- The leakage guardrail checks feature timestamps. It does not detect target leakage hidden in derived columns of a real dataset; review the dataset's columns first.
- The `value` model ($ per WAPE point) is a placeholder; set `basis: measured` only once finance signs off on the figure.
- Backtests are deterministic, so `repeats: 2` is enough; the noise floor comes from fold variance, not machine noise. Check that it stays above zero on real data.
