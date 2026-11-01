# Cut-over: from the TypeScript packages to Rust and SwiftUI

Step 4 of the big-bang plan in [NATIVE_ARCHITECTURE.md](NATIVE_ARCHITECTURE.md). The TypeScript packages were kept as the
executable spec until parity; this page is the evidence for deleting them: what replaces each package, how its tests were
carried over, every item that was dropped or deferred (with the reason and a judgement), the gaps found while checking
and what was done about them, and the verdict per package.

**The TypeScript is not lost.** Everything deleted here is in git history at `fc503b1` (the last commit before the cut-over,
branch `claude/peaceful-lamport-leak6p`): `git show fc503b1:packages/<name>/...`, or `git worktree add ../ts-spec fc503b1`.

## How this was checked

- **Counts.** Every `it(` / `test(` of the TypeScript tests was counted (an `it.each` counts its rows) and compared with the
  `PARITY.md` of each crate. The numbers below are the recount, not the claims; where the two differ it says so.
- **Names.** Every Rust test named in a `PARITY.md` was looked up (`fn <name>`; macro-generated rows by their macro). All exist,
  except three stale rows listed under [Stale documentation](#stale-documentation).
- **Content.** About 250 mapped pairs were read side by side (TS body and Rust body), all of the "ported (changed)" ones among
  them, and every literal a TS assertion expects was searched for in the Rust test (aws 146, bench 182). Where the Rust test
  asserts less than the TS one it is listed below.
- **Run.** `cargo test --workspace` on Linux: 1,692 tests passed and 1 failed in the first run, on a machine that was busy
  with other builds (`core_loop::completes_a_kill_switch_that_was_interrupted`, a timing-sensitive wait of five scheduler
  cycles); it passes alone and in repeated runs. See [Follow-ups](#follow-ups). The final numbers are in the last section.
- **Demos end to end.** `ratchetd simulate --project search|forecast|cli` (the Rust replacement of
  `node packages/runtime/dist/bin/simulate.js`) was run once for every demo: real git, the real bench pipeline, scripted agents
  and a mock cloud. See [Demos](#demos).

## Per package

| TS package | Replacement | Tests: TS → Rust | Verdict |
|---|---|---|---|
| `core` | `ratchet-core`, `ratchet-ledger`, `ratchet-env` | 382 → 382 mapped, 0 dropped | **delete** (assets moved, see below) |
| `engine` | `ratchet-engine` (+ `ratchet-env` for `environment/`) | 168 → 168 mapped, 0 dropped | **delete** |
| `runtime` | `ratchet-engine::runtime` + `ratchetd simulate` | 27 → 27 mapped (+4 new) | **delete** (the `simulate` CLI was ported now) |
| `aws` | `ratchet-sandbox` | 237 → 237 mapped, 0 dropped | **delete** |
| `bench` | `ratchet-bench` | 312 → 309 mapped, 3 dropped | **delete** |
| `agents` | `ratchet-agents` | 158 (163 with `each`) → 163 mapped (+25 new) | **delete** |
| `github` | `ratchet-github` | 36 → 35 mapped, 1 dropped | **delete** |
| `cli` | `ratchet-cli` (`crates/ratchet-cli`, binary `ratchet`) | 41 → 41 mapped | **delete** |
| `contract` | `crates/ratchetd/tests/api/*` (Rust) and the Swift client generation | 159 type-equality rows → none | **delete** |
| `desktop` (Electron main + preload) | `ratchetd`, `ratchet-connectors`, `ratchet-github`, `ratchet-engine::desktop` | 170 → 164 mapped, 6 dropped | **delete** |
| `desktop` (React renderer) | `apps/mac/Ratchet` (SwiftUI) | 65 UI tests → Swift model tests only | **delete**, with the follow-ups below |
| `ui` | `apps/mac/Ratchet/Sources/Ratchet/DesignKit` | 20 → partly (Format tests) | **delete**, with the follow-ups below |
| `e2e` | Rust API tests, `smoke-daemon.sh`, Swift tests | 4 specs, never run in CI | **delete** |
| `examples/*` | (they are the demo repositories) | their own vitest suites | **keep** |
| `infra/` | (OpenTofu modules, machine images, `infra/tests`) | 248 vitest cases + 3 tofu test files | **keep** |

### `core` → `ratchet-core`, `ratchet-ledger`, `ratchet-env`

382 TS cases in 18 files (config 50, config-simplified 28, templates 57, rule 25, protected 15, protected-set 16, stats 10,
output 11, calibrate 18, value 5, environment: hcl 6 + cost 10 + environment 11 + brief 7 + validate 63, ledger 34, sandbox
lifecycle 10, cost reconcile 6). Each has a Rust test; the PARITY counts of the three crates add up exactly. All 35 `Ledger`
methods and all 56 port types exist. Nothing was dropped.

Changed on purpose (all hold): zod → a hand-written parser with the same messages (golden-checked on a 700-document mutation
corpus); `RangeError` → `Result`; ledger on async sqlx; `subscribe` → broadcast channel; YAML syntax errors read differently
after the `invalid YAML (line L, column C): ` prefix.

Two real differences found while checking, and what was done:

- **A malformed glob in `protected:` protected nothing, silently.** The TS compiled each glob to a `RegExp` that threw on
  `[z-a]`; the Rust glob "simply never matches". For a list whose job is to block changes to the evaluator that fails open.
  **Ported now:** `parse_config` rejects such a glob (`protected glob is not a valid pattern`, with its path), test
  `rejects_a_protected_glob_that_could_never_match`. (An unterminated `{a` is a literal in both implementations.)
- **`never_stores_secret_values_in_the_ledger` checked less** (the TS test searched the whole serialized SQLite file, the Rust
  test only `engine_kv`). **Ported now:** the test reads every column of every table.

Minor, left as is: `extract_last_json_object` with `1e999` or a lone surrogate skips that object (serde_json limits) and may
read an earlier one; the calibrate "too optimistic pair" test checks the surrounding text, not the `\d+(\.\d)?%` format (the
`calibrate.json` golden covers the exact text).

Assets that are not TypeScript and had consumers:

| Was | Consumer | Now |
|---|---|---|
| `packages/core/templates/**` (7 evaluator templates, 21 files) | `ratchet-core` embeds them with `include_dir!`; the template tests run their `.mjs` evaluators; they are written into users' repos | moved to **`templates/`** at the repository root (`git mv`, history kept); `files.rs` and `tests/templates.rs` point there |
| `src/ledger/migrations/0000_init.sql` | `ratchet-ledger/tests/migration.rs` compares the Rust schema with the drizzle one | frozen copy at `crates/ratchet-ledger/tests/fixtures/drizzle_0000_init.sql` |
| drizzle `meta/*.json`, `scripts/gen-templates.mjs`, `scripts/copy-assets.mjs` | TS build only | deleted |

### `engine` and `runtime` → `ratchet-engine`

`engine`: 168 definitions (175 vitest cases with `each`): 127 in the engine proper, 41 under `environment/` (apply 10, drafter
16, mock 15), which `crates/ratchet-env/PARITY.md` maps (the engine PARITY header says "127" without pointing there). All 168
have Rust tests; most Rust tests assert more than the TS ones (e.g. runtime summary 4 → 24 assertions). `runtime`: 27 → 27
(+4 demo-verdict tests).

Changed or weakened: the unsubscribed-listener assertion is gone (a dropped broadcast receiver cannot hear anything); the
`agents: ['gemini']` rejection is unrepresentable in the typed `AgentKind`; the mock "tampered check" half is covered by
`drafter::approve_refuses_a_draft_whose_checks_did_not_pass`.

Gaps found and ported:

- **`packages/runtime/src/bin/simulate.ts` had no Rust command.** `ratchet_engine::runtime::simulate()` existed only as a library
  function. **Ported now:** `ratchetd simulate [--project search|forecast|cli] [--attempts N] [--duration S] [--repeats N]
  [--minutes M] [--db PATH] [--summary PATH.json] [--repo-root DIR]` prints the same summary, writes `--summary`, and exits 0
  only with a verified win. Docs that pointed at the JS were updated. The library function and the summary builder already had
  tests (`runtime_summary`, `runtime_simulation`); `simulate` itself is exercised by the CI `demos` job and was run once per
  demo before this cut.

### `aws` → `ratchet-sandbox`

237 cases (186 `it` + 51 `each` rows). Every case has a Rust test (226 tests: some loops collapse rows into one test, others
add cases). `sandbox/environment.test.ts` (29 cases) is mapped in `ratchet-env/PARITY.md` (27 tests, the `each` rows looped
inside one) and is missing from `ratchet-sandbox/PARITY.md`, whose "208 → 199" is exactly the total without it: a bookkeeping
gap, not a coverage gap. Not tested in either language: SSM's missing `ResponseCode` read as 0, and kill-on-drop of the
process runner.

### `bench` → `ratchet-bench`

312 cases (303 + 9 for the C launcher test that lived under `infra/`) → 309 Rust tests, 3 dropped:

| Dropped | Reason | Judgement |
|---|---|---|
| `integration.test.ts` (LocalBenchRunner on `examples/demo-search`, real `tsc`, cheating candidate blocked) | needed the TS toolchain | The pipeline is covered by `runners.rs` with a shell bench, and end to end by the `demos` CI job (real evaluator on the real demo). Acceptable |
| runners: "keep candidate-only protected files" for the remote runner | the TS body returned early for the remote runner (vacuous); the local variant is ported | Nothing lost |
| launcher: "compiles statically with the flags of the image build" | the Rust binary has no C warnings; the static link is a build flag | Replaced by the `launcher` job of `images.yml`, which links it static-pie, checks it and installs it with the real image script |

`ratchet-bench/PARITY.md` says "310 + 2 dropped" (it counts the launcher case differently), and its dependency
manifest-validation table is shifted by one row (all 15 Rust tests exist and assert the right errors).

### `agents` → `ratchet-agents`

158 definitions, 163 with `each`, → 188 Rust tests (17 unit, 171 integration; the binary test now always runs where the TS one
skipped without `pnpm build`). `scripts/**` (3 demos, 33 files) and the 10 transcript fixtures were already identical copies
under `crates/ratchet-agents/`.

Dropped or changed on purpose:

| Item | Reason | Judgement |
|---|---|---|
| demo-search cc/cx: real `pnpm test` after each script → a structural guardrail check | needed node/pnpm/vitest | **A real loss of coverage**: nothing runs the demos' own tests on the patched trees any more. Mitigated: the `demos` CI job runs all three demos end to end through the real evaluator and guardrails, and the unpatched demos' vitest suites run in the `node` job. Not the same check; listed as a follow-up |
| demo-search parallel: `demoTestsPass(finalDir)` | same | same |
| demo-cli / demo-forecast cumulative patches: `tsc` + `vitest` after each order | same | same |
| claude abort error no longer carries the session id | its only reader was a dead store (`runner-loop.ts:347`) | holds |
| behaviour differences 1–10 (reader retries, worktree order, env-number exit 2, `bench` alias absent from the runner config) | deliberate | hold. #10 mattered only while the cloud AMI ran the TS runner; see the next section |

**Gap found and ported: the cloud runner image still shipped the TypeScript runner.** `infra/images/pack-runner.sh` ran
`pnpm pack` on `@ratchet/core` and `@ratchet/agents`, and `60-ratchet-runner.sh` installed them with npm: the Rust engine's
`SsmRunnerControl` was starting the TS runner on the AMI, and deleting `packages/` would have made the image unbuildable.
**Ported now:** the image installs the static Rust `ratchet-runner` (musl, per architecture, built by a new `runner` job of
`images.yml`, which also runs the real `pack-runner.sh` and `60-ratchet-runner.sh` into a scratch prefix and checks that a
tampered binary is refused); `pack-runner.sh` can also build it locally. `90-verify-runner.sh` no longer requires a `ratchet`
command, which nothing ever installed. Not verified: a real Packer build and a launch on AWS (no AWS access here, same as before).

### `github` → `ratchet-github`

36 cases → 35 mapped + 1 dropped (`buildResultReport` throws for an unknown optimization: the ledger lookup is the caller's job
and `ratchet-engine/src/report.rs` returns `not_found`; neither implementation calls `buildResultReport` outside the package).
The two report fixtures are byte-identical to the vitest snapshots. The REST client is `reqwest` (not octocrab as the
architecture page said), tested against an in-process fake GitHub server, a port of `packages/e2e/harness/fakes.mjs`.

### `cli` → `ratchet-cli`

41 cases → all mapped (offline 30, daemon 12, binary 3 + units). Same
commands and flags. On purpose: clap usage errors exit 2 (TS 1); `cost reconcile` asks the daemon (no `--db`, `--lag-hours`,
`--accept-estimated`; the daemon owns the ledger). About 20 daemon commands are new. The Mac app's Stop, Pull request, Report,
Agent log and Spend are all `ratchet` commands (`stop`, `pr`, `report`, `agent log`, `spend`).

### `contract`

`api.d.ts` (4,420 lines) and `contract.check.ts` (159 `Same`/`Within`/`Sends` rows) proved that the Electron app's TypeScript view
models equalled the wire types. With the Electron app gone nothing consumes either file (no Rust, Swift, CI or tool reads them).
What still guards the contract: `crates/ratchetd/tests/api/` (the committed `api/openapi.yaml` is byte-identical to the generated
one; every route round-trips its types; camelCase; the event union is snapshotted; the committed method → operation table
`electron_api.rs` resolves to real operations) and, for Swift, `tools/generate-api.sh`. The `channels.ts` cross-check in
`electron_api.rs` read a TS file and became a silent no-op once it was gone; it was removed. **The Swift "API client is current"
step of `native.yml` is advisory (`continue-on-error`), so after this cut nothing hard-fails if the Swift client drifts from
`openapi.yaml`.** Left advisory because it could not be run here; hardening it is a follow-up.

### `desktop` main process → `ratchetd` and friends

170 test cases in 11 files (`each` runs counted once per `it`): connectors 25/25, overview 12/12, settings 5/5, machine 9/9, mappers 23/23 (`downsample` was
unused), product-engine 19+/all, service 38/39, github 9/9, dev-engine 11/12, ipc 14 (12 mapped, 2 dropped; 3 of them have no PARITY row but are
covered), dev-git 0/2. Dropped (6): `service` "picking a folder without a dialog" and `ipc` "exportReport reports cancellation" (the
app's save/open dialogs), `isTrustedUrl` (a 0600 unix socket plus a bearer token replace the preload bridge), dev-engine "rejects
an invalid config" (unreachable through the API), the two dev-git cases (see below).

The `ratchetd/PARITY.md` rows marked "deferred to the real engine" (GitHub row denied/rejected/environment token, template
refused when files exist, settings keychain/corrupt file/environment credentials) **are implemented and tested** now
(`tests/api/real_service.rs`, `src/real/settings.rs`); the PARITY rows are stale, as is "environment drafts answer 501".

Two behaviours of the Electron main process that the daemon did not have, **ported now**:

- **Keep the machine awake while a run is active** (`power.ts`, Electron `powerSaveBlocker`): `ratchetd` was wired to a blocker
  that did nothing. Now `ratchetd::host::CommandPowerBlocker` runs `caffeinate -i -w <daemon pid>` on a Mac while any run is
  active (the helper exits with the daemon). Tested with a stand-in helper.
- **The bundled OpenTofu module** (`extraResources: infra/aws`): the `.app` did not contain `infra/aws`, so real AWS runs from the
  bundle would have looked for it in the working directory. Now `build.sh` copies it to `Contents/Resources/infra/aws` (tests and
  state excluded, as electron-builder did), `ratchetd` finds `Contents/Resources` from its own path
  (`host::bundled_resources`, tested), and `smoke-daemon.sh` checks it is in the bundle. A pinned `tofu` binary in
  `Resources/bin` is still picked up when present; none is shipped, `tofu` comes from `PATH`.

### `desktop` renderer, `ui`, `e2e` → the SwiftUI app

The native app does not yet do everything the Electron renderer did. The audit compared every screen and every API operation
the app calls (22 of the 63 operations are not called by Swift). What was ported now, with Swift Testing tests
(`MockClientTests`, `AppStoreTests`, `DaemonClientTests`):

- **Stop** (the kill switch) on a running optimization, with a confirmation ("Stop and revoke keys").
- **Create pull request** for the verified wins (the old button only opened an existing pull request, on a hard-coded
  `github.com`; the address now comes from the daemon, so GitHub Enterprise works).
- **Export report** (save the markdown report).

**Not ported; follow-ups.** Each is reachable through the API, and most through the `ratchet` CLI, but not through the app:

| Electron renderer feature | Reachable today through |
|---|---|
| Live agent log (paged, appended on events) | `ratchet agent log`, `ratchet events` |
| Spend breakdown by category and agent; per-win cost | `ratchet spend` |
| "Where the time went" spans; before/after compare rows | `GET /v1/optimizations/{id}` |
| Benchmarks: metric switch, latency histogram, candidate filters (`@ratchet/ui` `Histogram`, `EvalBars`) | `GET …/ratchet?metric=` |
| Template → pull request ("Add via pull request") | the API only (`proposeTemplate`); the Swift gallery prefills the wizard |
| New optimization: repo search, "Local folder…", `ratchet.yaml` review and invalid-YAML reason, "What's missing" preflight | `ratchet inspect <path>`; preflight: the API only |
| Connectors: paste a GitHub token, AWS region and monthly cap, rename, edit in place, "Check connection"/"Refresh all", re-detect CLIs, `~/.aws` profile list, keychain-unavailable explanation | the API only |
| Advanced (development) overrides: AMI ids, `tofu` binary and module directory | the API only (`saveAdvancedSettings`) |
| Discard an environment draft | the API only |
| Demo mode showed a real diff of the winning change (`dev-git`) | `ratchetd` simulation answers "no git access"; real-mode demos do diff |
| A full Environments screen ("How it fits together") | the sidebar sandbox line and receipt |

`packages/e2e` had four specs; none ran in CI (CI only syntax-checked them). 01-boot (Electron security) does not apply to
SwiftUI; its API-surface half is `electron_api.rs` and `tests/api/auth.rs`. 02-navigation is partly the Swift self-test and the
snapshots. 03-main-flow (a full demo run through the UI) has no native counterpart: `ratchetd simulate` and the `demos` job cover
the engine, `smoke-daemon.sh` + `--selftest-live` cover the client path (they do not start a run). 04-sandbox (connectors against
fakes) is covered on the server by the fake GitHub and the `real_service` tests, not through the UI. `NATIVE_ARCHITECTURE.md`
promised XCUITest flows, Spotlight and Quick Look: none exist.

### Demos

`examples/demo-search`, `demo-forecast`, `demo-cli` stay (the Rust demo tests, `ratchet-core`'s config test and the simulation
runtime read them). They are TypeScript projects, so the Node toolchain stays: the root `package.json` and `pnpm-workspace.yaml`
now list only `examples/*` and `infra/tests`. `ratchetd simulate` needs `pnpm install` and a built demo
(`tsc -p tsconfig.build.json` in the demo directory), exactly like the TS runtime did.

| Run | Result |
|---|---|
| `ratchetd simulate --project search` | exit 0: 2 verified wins of 18 candidates, +56% (run while the machine was busy compiling) |
| `ratchetd simulate --project forecast` | exit 0: 5 verified wins of 22 candidates, +45% |
| `ratchetd simulate --project cli` | exit 0: 5 verified wins of 19 candidates, +59% (a first run on a busy machine ended with no win: the cold-start bench is timing based, which is why the TS e2e retried up to three times and the Rust one does too) |
| `RATCHET_E2E=1 cargo test -p ratchet-engine --test engine runtime_simulation` | 5 passed in 19 min (search with both agents, forecast, cli: the verdict invariants of the TS `simulation.e2e` on all three demos) |

### Things that stay, and why

- **`infra/`** and its tests: OpenTofu modules, the machine-image definitions and `infra/tests` (vitest). `images.test.ts` read
  TypeScript sources as text to cross-check the image scripts against the instance user data; it now reads the Rust sources
  (`crates/ratchet-sandbox/src/compute/{aws,user_data}.rs`, `crates/ratchet-bench/src/{image_scripts,remote_scripts,deps_scripts,shell}.rs`,
  `crates/ratchet-agents`, `crates/ratchet-engine/src/runtime/live.rs`), and the runner-image assertions describe the new
  pipeline. `infra/images/build.sh` takes the AMI version tag from `crates/ratchetd/Cargo.toml` (what `ratchetd::VERSION` sends)
  instead of `packages/desktop/package.json`. Node and npm stay inside the runner image: the `claude` and `codex` CLIs are npm packages.
- **`examples/`**: see above.
- **`templates/`**: see the `core` section.
- **Golden fixtures** (`crates/ratchet-{core,env,bench}/tests/golden/*.json`, 28 files): kept. They are committed data that the
  Rust tests read at test time; no test runs a generator. The three `generate.mjs` generators imported the TypeScript sources and
  were **deleted**, so the fixtures can no longer be regenerated: they are frozen expectations of what the TypeScript computed
  (number formatting, statistics, the ratchet rule, glob matching, config validation on a mutation corpus, environment validation,
  bench script text...). The original generators are at `fc503b1:crates/*/tests/golden/generate.mjs`. A change in the Rust
  behaviour that a fixture pins now needs a deliberate fixture edit.
- **`crates/ratchet-agents/scripts/`** (the recorded scripts of the scripted agents and `build-scripts.mjs`, which rebuilds them
  from a built demo): kept; they need Node and the demos, not the TS packages.
- **`api/openapi.yaml`** and the Swift generation: kept. `tsconfig.base.json`: kept (extended by `infra/tests`).

## Stale documentation

Fixed in this change: `NATIVE_ARCHITECTURE.md` (status, octocrab, "bench-host agent binary"), `api/CONTRACT.md`, the README,
`docs/real-env/*`, `docs/templates.md`, `docs/packs/*`, `design/README.md`, `infra/images/README.md`, `infra/contract.md`, and the
`PARITY.md` rows below. Not touched because they describe the history of the TS design: `V0_ARCHITECTURE.md`, `PLAN.md`,
`SIMPLIFIED_MODEL.md` carry a banner saying so.

PARITY rows corrected: `ratchetd` "37 IPC channels" (51 + the event channel), the missing `overview_numbers_add_up`, the
deferred/501 rows; `ratchet-engine` the missing `provisional_groups::…` row and "127 TS cases"; `ratchet-github` "`ratchet-ledger`
does not exist yet" and the XCUITest claim; `ratchet-sandbox` now lists `environment.test.ts`; `ratchet-bench` the shifted table.

## Follow-ups

1. **Swift app parity** with the Electron renderer: the table above. The renderer is no longer in the tree
   (`git show fc503b1:packages/desktop/src/renderer/screens/Result.tsx`, `…/Connectors.tsx`, `…/Benchmarks.tsx`,
   `…/components/UseTemplate.tsx`, `packages/ui/src/Histogram.tsx`). **This departs from the rule "if large, list it and keep the
   TS package"**: the task named `desktop`, `ui` and `e2e` for deletion and the Swift port could not be built here (no macOS), so
   it was done for the three highest-value items only.
2. **Harden the Swift client drift check** (`native.yml` "API client is current" is advisory) once it has been seen to pass.
3. **Demo patch suites run their tests again**: after applying each cumulative patch run the demo's `tsc` and `vitest` (the
   TS tests did).
4. **A real Packer build and launch of the runner and bench images** on AWS; the CI checks stop at the install scripts.
5. **Timing-sensitive tests** failed once each under load and were hardened: `core_loop::completes_a_kill_switch_that_was_interrupted`
   (five scheduler cycles, now 500), `runtime_local_agents` (the receipt is written a moment after the sandbox row is marked
   destroyed: now awaited), the `memory` template test on macOS (see above). Others of the kind may exist.
6. **Packaged app, real-mode demos** need `examples/` and `node_modules` next to the daemon; the packaged app's demo mode is the
   daemon's own simulation and needs neither. `demo_scripts.rs` locates the scripts through `CARGO_MANIFEST_DIR`.
7. **XCUITest flows, Spotlight, Quick Look** (promised by the architecture page, not built).
8. **Ship a pinned `tofu`** in `Contents/Resources/bin` (optional, picked up automatically).
9. **Dead code from the Electron host**: `ratchet-bench`'s `NodeRuntimeInfo::electron` (the shim that ran Electron as node) has no
   caller any more.
10. **Real-env procedures** (`docs/real-env/*.md`) still show TypeScript snippets; re-express them with `ratchetd` / `ratchet`.

## Final numbers

- `cargo test --workspace --no-fail-fast` on Linux: **1,698 passed, 0 failed, 0 ignored**. The baseline run before this change had
  1,693 tests (1,692 passed and the load-related failure noted above); the five new ones are the four `ratchetd::host` tests and
  the protected-glob config test. `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean.
- `pnpm install --frozen-lockfile && pnpm build && pnpm typecheck && pnpm test`: demo-search 67, demo-forecast 28, demo-cli 72,
  infra/tests 248 (+1 skipped, needs root) passed.
- The `ratchet-runner` musl build, `pack-runner.sh`, `60-ratchet-runner.sh` (scratch prefix) and the tampered-binary refusal were run
  by hand on x86_64 Linux; the arm64 build is exercised only by `images.yml`.
- CI for the head commit: see the links in the final report of the change.
