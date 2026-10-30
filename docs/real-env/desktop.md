# Real-environment testing: desktop app

`packages/desktop` (Electron main + React renderer) implements the simplified model in
[SIMPLIFIED_MODEL.md](../SIMPLIFIED_MODEL.md). This file lists what is real, what is faked in the automated tests, and what to verify on a
real Mac with a real GitHub account, AWS profile and agent CLIs.

## What is real vs. faked

| Piece | State |
|---|---|
| Ledger (`openLedger(<userData>/ratchet.db)`), all reads, view-model mappers | **Real.** Every screen reads the SQLite ledger through `Ledger` methods. |
| Typed IPC, zod input validation, sender check, event push | **Real.** The renderer API (`RatchetApi`) has no method that returns a secret, and no plan / apply / destroy of infrastructure. |
| Secrets (`safeStorage`), settings file | **Real** code; the OS keychain path is only exercised on a real desktop session. Without a keychain (Linux CI) the app refuses to store a token and says so. |
| Engine (`EngineApi`) | **Real.** Repositories: `createHybridRuntime` per repo (local agents by default, cloud evaluator, automatic sandbox). Demos: the same Engine on `createSimulationRuntime`. `DevEngine` (seeded data) is only used by renderer tests and `RATCHET_ENGINE=dev`. |
| Sandbox | **Automatic.** Created when a run starts, removed after the last run plus the idle window (30 min), receipt written. Demos get a mock one (`ratchet-sim-<project>`), repositories an AWS one. |
| GitHub sign-in, repo list, `ratchet.yaml` read, template pull request | **Real code.** Tests fake `fetch` (unit) or a local HTTP server (`packages/e2e/harness/fakes.mjs`); nothing was run against github.com. |
| AWS profiles | **Real parser** of `~/.aws/config` and `~/.aws/credentials` (names and kinds only). "Test connection" is `createConnectionTester` (STS); not run against a real account here. |
| Harness detection | **Real** (`detectHarnesses` in the main process); tests use fake `claude` / `codex` executables on PATH. |
| Local agents, cloud evaluator on real AWS | **Real code, not exercised here.** |
| PR url | Kept in `settings.json` (the ledger has no column). |

### How the main process is wired (`src/main`)

* `index.ts`: opens the ledger and the settings store, creates the `HarnessCache` (probed on launch), the GitHub API (client id from
  `RATCHET_GITHUB_CLIENT_ID`), the service and the IPC; drives `powerSaveBlocker.start('prevent-app-suspension')` / `stop` from
  the `run-active` event (`power.ts`).
* `engine.ts` (`createEngineForApp`): the product engine. `slugOf(opt.repo)` decides where a command goes: a GitHub repo to the
  runtime built for it, a filesystem path (a demo's local remote) to its simulation runtime. Runtimes are built lazily, from
  Settings and the cached detection: region from the repo's `environment.region` (read in step 1), the bench AMI found by tag for
  the app version (`resolveImageRefs`; the cloud runner AMI too, only needed for "Cloud"), the profile, the GitHub token, the
  harnesses and the login PATH. Changing a setting or signing in/out marks them stale; an idle runtime is rebuilt on next use,
  one with an active run is kept until it finishes. An AWS sandbox that outlived the app is watched from launch so its idle
  destroy still happens. `run-active` from every runtime is merged into one signal.
* `service.ts`: the renderer-facing API. `preflight` is the "what's missing" list; `inspectRepo` reads `ratchet.yaml` from the
  default branch (GitHub contents API) or a local folder (`local-repo.ts`) and summarizes it with core's `parseConfig` and
  `effectiveProtected`; `environments` / `endSandbox` use `sandboxActivity` / `endSandbox`; `getOptimization` adds the teardown
  receipt and the calibration sentence.
* `github.ts` + `github-signin.ts`: REST + the OAuth device flow (poll loop with `slow_down`, expiry, denial; injectable
  `fetch`/`sleep`). `aws-profiles.ts`, `harnesses.ts`, `local-repo.ts`, `power.ts`: small, injectable, unit-tested.

Configuration:

| Variable | Use |
|---|---|
| `RATCHET_GITHUB_CLIENT_ID` | OAuth app (device flow enabled) for "Sign in with GitHub". Without it a placeholder is used and Settings says so. |
| `RATCHET_GITHUB_API_URL`, `RATCHET_GITHUB_WEB_URL` | GitHub Enterprise; the end-to-end tests point them at a fake. |
| `RATCHET_SIM_IDLE_MS` | Idle window of a demo's mock environment (default 30 min). |
| `RATCHET_E2E_PICK_FOLDER` | Test only: answers the "Local folder…" dialog. |
| `RATCHET_USER_DATA_DIR`, `RATCHET_E2E_SAVE_DIR`, `RATCHET_REPO_ROOT`, `RATCHET_ENGINE=dev` | As before (isolated userData, report export, monorepo root for demos, the seeded stub). |

## Checklist on a real Mac

### 1. Signed, notarized build

1. `pnpm install && pnpm --filter @ratchet/desktop build`.
2. Set `CSC_LINK` / `CSC_KEY_PASSWORD` (Developer ID Application cert) and `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`,
   `APPLE_TEAM_ID`; run `pnpm --filter @ratchet/desktop package`.
3. Verify: `codesign --verify --deep --strict --verbose=2 Ratchet.app`, `spctl -a -vvv -t install Ratchet.app`,
   `xcrun stapler validate Ratchet.dmg`.
4. Open the DMG on a *different* Mac (quarantine flag set): no Gatekeeper warning, app starts.
5. Confirm native module loading: the app opens (proves `better-sqlite3` loads from `app.asar.unpacked` under the hardened
   runtime; the entitlement `disable-library-validation` in `build/entitlements.mac.plist` exists for this).
6. Confirm migrations ship: `Ratchet.app/Contents/Resources/migrations/*.sql` exists.
7. Unverified risk: electron-builder with pnpm workspaces sometimes misses production deps (`better-sqlite3`, `yaml`, `zod` and now
   `@ratchet/agents` / `@ratchet/aws` are externalized from the main bundle). If the packaged app crashes with "Cannot find
   module", set `node-linker=hoisted` for the desktop package install, or bundle those deps.

None of the packaging config (`electron-builder.yml`, entitlements) has been run: only `electron-vite build` and the headless
Linux launch below were exercised.

### 2. Settings (three rows)

1. **GitHub.** Register an OAuth app with the device flow enabled, launch with `RATCHET_GITHUB_CLIENT_ID=<id>`. *Sign in with
   GitHub* shows a code and opens `github.com/login/device`; approve; the row shows `Signed in as <you>` within a few seconds.
   `~/Library/Application Support/Ratchet/settings.json` holds base64 cipher text only (`grep -c gho_ settings.json` is 0);
   Keychain Access shows a "Ratchet Safe Storage" item. *Test connection* says `signed in as <you>`. *Sign out* clears it. Deny the
   keychain once: the flow ends with the "keychain is unavailable" message instead of storing anything. Unverified: the token has
   scope `repo` (a GitHub App with fine-grained permissions would be narrower; the flow is the same).
2. **AWS.** *Connect AWS* lists the profiles and SSO sessions from `~/.aws` (names only). Pick an SSO profile after
   `aws sso login --profile <p>`, *Test connection*: `signed in to account <id> in <region>`; an expired session appends the
   `aws sso login` command. The region shows `us-east-1`; change it, save.
3. **Agents.** With Claude Code and Codex installed and signed in, both show their version and `Signed in`, and `2 ready`. Sign out
   of Codex in a terminal, *Re-detect*: it shows `Signed out`, the `codex login` hint and an API key field. Install nothing for a
   harness: `Not installed` with the install command. Set `ANTHROPIC_API_KEY` in the environment you launch from: Claude Code
   shows `API key`. A GUI launch does not inherit the shell PATH: confirm the CLIs are still found (login-shell PATH).
4. **Advanced (development)** is collapsed; AMI ids typed there bypass the tag lookup.
5. DevTools: `window.ratchet.getSettings()` returns booleans for the secrets and no values; `window.ratchet.githubStatus()` has the
   login but no token.

### 3. A real run on a repository

Needs a sandbox AWS account with the published images (tag `ratchet:image` + `ratchet:image_version` = the app version), a GitHub repo
you can push to, and a signed-in Claude Code or Codex.

1. New optimization -> pick the repo (the list is your account's repos, most recently pushed first). With no `ratchet.yaml` on the
   default branch the templates appear; *Add via pull request* opens a PR and shows its link (check the branch
   `ratchet/template-<id>`); merge it, *I merged it: check again*.
2. Review: measured metric, toolchain (`host` or the container), machine, region, protected paths (each with a reason). *What's
   missing* must name exactly what is missing: GitHub, AWS, an agent that is not signed in; fix it in Settings and the list empties.
   *Cloud (unattended)* adds the API key requirement and the cloud runner image.
3. Budget & time (defaults $50 / 2 h) -> *Start*. Agents opens at *Baselining*. The Mac must not sleep while it runs (check with
   `pmset -g assertions`: a `PreventUserIdleSystemSleep`-class assertion from Ratchet while a run is active, released after).
4. Environments shows the sandbox appearing (`Creating`, then `Ready`) in the repo's region, instances and cost so far; *End sandbox
   now* is disabled while a run is active. After the last run it goes `Idle` and says when it removes itself.
5. *End sandbox now* (confirm) or wait for the idle window: the receipt appears on Environments and on Result; run the zero-orphan
   list at the end of `docs/real-env/aws.md` §8 and compare.
6. Local agents: each agent works in its own worktree under `<userData>/sandboxes/local-agents`; they die with the app.
7. Local folder: pick a clone whose `origin` is on GitHub: the same ratchet.yaml is read from the working tree.

### 4. Not covered by anything yet

* Windows / Linux packaging (config exists for AppImage, untested).
* Auto-update.
* "Open session" on an agent card: needs the runner's session URL from the engine.
* A repo-less local folder (no GitHub `origin`): the evaluator fetches candidates from GitHub, so it is refused with a message.
* More than one machine: the idle-destroy of a sandbox created by the app on another computer.

## Running the app headless (what dev/CI did)

```
pnpm --filter @ratchet/desktop build
pnpm --filter @ratchet/e2e test:e2e          # Playwright under Xvfb; SKIP_BUILD=1, E2E_FILTER=03
```

The end-to-end specs launch the built app with a throwaway user-data dir and fakes for GitHub, the agent CLIs and `~/.aws`
(`packages/e2e/harness/fakes.mjs`). Under Xvfb the OS keychain is unavailable, so the device-flow and pasted-token tests assert
the refusal instead of a sign-in (the stored-token path is covered by the unit tests with a fake cipher).
