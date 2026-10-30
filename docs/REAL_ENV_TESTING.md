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
