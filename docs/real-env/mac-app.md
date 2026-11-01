# Real-environment testing: the Mac app and `ratchetd`

The Electron app (`packages/desktop`) was replaced by the SwiftUI app `apps/mac/Ratchet` and the daemon `ratchetd`
(`docs/NATIVE_ARCHITECTURE.md`, `docs/CUTOVER.md`). The previous version of this page, with the Electron wiring and its end-to-end
steps, is at `fc503b1:docs/real-env/desktop.md`. This one lists what is real, what is faked in the automated tests, and what to
verify on a real Mac with a real GitHub account, AWS profile and agent CLIs.

## What is real vs. faked

| Piece | State |
|---|---|
| Ledger (SQLite in `~/Library/Application Support/Ratchet`), reads, view-model mappers | **Real.** `ratchetd` is the single writer; the app and the `ratchet` CLI are clients of its unix socket. Covered by `crates/ratchetd/tests/api`. |
| API contract, bearer token on a `0600` socket, SSE | **Real.** `api/openapi.yaml` is generated from the Rust types; the Swift client is generated from it. No route returns a secret (`tests/api/electron_api.rs`). |
| Secrets (Keychain, service `ratchet`), settings file | **Real** code in `ratchet-connectors`; the Keychain path is compiled and exercised on macOS only (the `rust (macos-latest)` job). Without a keychain (Linux) the daemon refuses to store a secret unless `--secrets-file` is given. |
| Engine (`ratchet-engine`) | **Real.** Repositories: a hybrid runtime per repository (local agents by default, cloud evaluator, automatic sandbox). Demos: the same engine on the simulation runtime. |
| Sandbox | **Automatic.** Created when a run starts, removed after the last run plus the idle window (30 min), receipt written. Demos get a mock one. |
| GitHub sign-in, repo list, `ratchet.yaml` read, template pull request | **Real code**, tested against an in-process fake GitHub (`ratchet-github`'s `testing::FakeGithubServer`); nothing was run against github.com. |
| AWS profiles | **Real parser** of `~/.aws/config` and `~/.aws/credentials` (names and kinds only). "Test connection" is the STS probe of `ratchet-engine`; not run against a real account here. |
| Harness detection | **Real**; tests use fake `claude` / `codex` executables on `PATH`. |
| Local agents, cloud evaluator on real AWS | **Real code, not exercised here.** |
| Keep the Mac awake during a run | `caffeinate -i -w <daemon pid>` (`ratchetd::host`), tested with a stand-in helper. |
| OpenTofu module in the bundle | `Contents/Resources/infra/aws`, found by the daemon from its own path; checked by `smoke-daemon.sh` on macOS CI. |

Configuration (environment of `ratchetd`):

| Variable | Use |
|---|---|
| `RATCHET_GITHUB_CLIENT_ID` | OAuth app (device flow enabled) for "Sign in with GitHub". Without it a placeholder is used and Settings says so. |
| `RATCHET_GITHUB_API_URL`, `RATCHET_GITHUB_WEB_URL` | GitHub Enterprise; the tests point them at a fake. |
| `RATCHET_HOME` | The support directory (socket, token, ledger, settings, logs); a short path in tests. |
| `RATCHET_MODE`, `RATCHET_SETTINGS`, `RATCHET_SECRETS_FILE` | `serve --mode real|sim`, the settings file, a plaintext secrets file for machines without a keychain. |

## Build and install

```sh
cd apps/mac/Ratchet && ./build.sh --open       # builds ratchetd + ratchet, bundles them, installs ~/Applications/Ratchet.app
```

Unsigned builds are ad-hoc signed. Developer ID signing, notarization and the LaunchAgent's "Login Items" approval have not been
exercised on a release build.

## What to check on a real Mac

1. Launch: "Starting Ratchet…", then (first time) the Login Items approval prompt; quit and reopen: runs survive.
2. Demo mode (Settings > Daemon): a run in the daemon's simulation, no accounts needed.
3. Connectors: GitHub device flow, an AWS profile, the detected `claude` / `codex` CLIs and their sign-in status.
4. A real run on a small repository (see `engine.md`, `github.md`, `aws.md`); **Stop** with the confirmation, **Create pull request**,
   **Export report**; check the Mac does not sleep during the run.
5. The sandbox line and "End now"; the receipt.

Not in the app yet (use the `ratchet` CLI or the API): the live agent log, the spend breakdown, connector editing, the template pull
request, preflight; see the follow-ups in `docs/CUTOVER.md`.
