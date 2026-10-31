# Ratchet

Agents propose changes. A trusted benchmark decides. Only verified wins ratchet forward.

Ratchet points coding agents (Claude Code, Codex) at one measurable goal in your repository — a p95 latency, a cold-start time,
a forecast error — and lets only the changes that beat a trusted, tamper-proof benchmark move forward. A native Mac app drives
it; a local daemon does the work.

- Architecture: [docs/NATIVE_ARCHITECTURE.md](docs/NATIVE_ARCHITECTURE.md) (the current stack), [docs/V0_ARCHITECTURE.md](docs/V0_ARCHITECTURE.md) (the design it grew from)
- Product model (settings, harness detection, templates, invisible sandbox): [docs/SIMPLIFIED_MODEL.md](docs/SIMPLIFIED_MODEL.md)
- How the TypeScript stack was retired, and what is still open: [docs/CUTOVER.md](docs/CUTOVER.md)
- Plan: [docs/PLAN.md](docs/PLAN.md)

## What is in the repository

```
apps/mac/Ratchet    the native macOS app (SwiftUI); bundles the daemon and the CLI
crates/ratchetd     the daemon: HTTP + server-sent events on a unix socket, OpenAPI contract, LaunchAgent entry
crates/ratchet-cli  `ratchet`, the command line (a client of the same socket, plus offline validate/init/baseline)
crates/ratchet-*    the engine and its parts: core (config, statistics, the ratchet rule), ledger (SQLite), engine,
                    sandbox (AWS + OpenTofu), bench (the trusted evaluator), agents (Claude Code / Codex), github,
                    connectors, env (environment drafts)
api/                openapi.yaml, generated from the Rust types; the Swift client is generated from it
templates/          the evaluator templates `ratchet init --template` and the app write into your repository
infra/              OpenTofu modules for the evaluator sandbox, the machine images (Packer) and their tests
examples/           three demo repositories (search, forecast, CLI cold start) with scripted solutions
design/             the Midnight Brass design reference
```

Every external system (AWS, GitHub, the agent CLIs, git) sits behind a port with a real and a mock implementation, so the whole
product also runs on one machine in simulation.

## Install and run

Needs macOS 26 for the app and a Rust toolchain (rustup.rs, Rust 1.97 or newer).

**The Mac app** (builds the daemon and the CLI too, bundles them, installs `~/Applications/Ratchet.app`):

```bash
cd apps/mac/Ratchet && ./build.sh --open          # --universal: arm64 + x86_64
```

On first launch the app registers its daemon as a LaunchAgent (approve it under System Settings > Login Items if asked), so runs
survive quitting the app. **Try a demo first:** New optimization > Repo > "Try a demo" > Review > Start. It runs on the daemon's
simulation (scripted agents, mock AWS) and needs no account, no CLI and no setup. For real runs open Connectors: sign in to GitHub
(device flow; set `RATCHET_GITHUB_CLIENT_ID` to your OAuth app), pick an AWS profile (the evaluator runs in an automatic sandbox in
your account that is removed after the last run), and let the app detect the Claude Code and Codex CLIs on this Mac.

**The daemon and the CLI on their own** (Linux or macOS):

```bash
cargo build --release -p ratchetd -p ratchet-cli
target/release/ratchetd serve --mode sim --ledger :memory:     # the simulation; --mode real is the default
target/release/ratchet status                                   # the CLI talks to the same socket
target/release/ratchet init --template cli-cold-start           # write an evaluator template into a repository
target/release/ratchet validate                                 # check ./ratchet.yaml
target/release/ratchet run --demo demo-search --wait            # a whole run on a demo, through the daemon
```

`ratchet --help` lists the commands; every one takes `--json`. The socket and its token live in
`~/Library/Application Support/Ratchet` (`RATCHET_HOME` moves them).

**A headless demo run** (real git and bench pipeline, scripted agents, mock cloud; exits 0 on a verified win):

```bash
pnpm install && (cd examples/demo-search && ../../node_modules/.bin/tsc -p tsconfig.build.json)
target/release/ratchetd simulate --project search      # or forecast, cli
```

## Develop

```bash
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0   # smaller, faster builds
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

- API changes: edit the Rust types, then `cargo run -p ratchetd -- openapi` (writes `api/openapi.yaml`) and, for the app,
  `apps/mac/Ratchet/tools/generate-api.sh` (see [api/CONTRACT.md](api/CONTRACT.md)). `cargo test -p ratchetd` fails when the
  committed file is stale.
- Mac app: `apps/mac/Ratchet/CLAUDE.md` describes the layout, the tests (`swift test`), snapshot mode (`tools/snap.sh`) and the
  daemon smoke test (`tools/smoke-daemon.sh`).
- The demos and the infra tests are the only TypeScript left: `pnpm install && pnpm build && pnpm typecheck && pnpm test`.
  Infrastructure: `cd infra/aws && tofu fmt -check -recursive && tofu init -backend=false && tofu validate && tofu test`; machine
  images: [infra/images/README.md](infra/images/README.md).
- Every crate has a `PARITY.md` mapping the tests of the TypeScript it replaced to Rust tests.
- CI: `ci.yml` (demos, infra, the demo end-to-end), `native.yml` (Rust on Linux and macOS, the Mac app, the daemon smoke test),
  `images.yml` (the static launcher and runner binaries, Packer validation).
