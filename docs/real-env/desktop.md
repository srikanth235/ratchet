# Real-environment testing: desktop app

`packages/desktop` (Electron main + React renderer) is complete against the **dev engine stub**. This file lists
what to verify on a real Mac, and exactly what is stubbed until the real engine lands.

## What is real vs. stubbed

| Piece | State |
|---|---|
| Ledger (`openLedger(<userData>/ratchet.db)`), all reads, view-model mappers | **Real.** Every screen reads the SQLite ledger through `Ledger` methods. |
| Typed IPC, zod input validation, sender check, event push | **Real.** |
| Secrets (`safeStorage`), settings file | **Real** code; the OS keychain path is only exercised on a real desktop session. |
| Engine (`EngineApi`) | **Real** in both modes. Simulation: the core engine on mock AWS, a local git remote and scripted agents. Live: the core engine on `createLiveRuntime`, built from Settings (below). `DevEngine` (seeded demo data) is only used by renderer tests and `RATCHET_ENGINE=dev`. |
| Sandbox plan / apply / destroy | **Real code, mock cloud in simulation.** Both modes go through `engine.planSandbox / applySandbox / destroySandbox`, which record `planned → applying → applied → destroying → destroyed / failed` and the outputs in the ledger (`sandboxes`). Simulation uses `MockSandboxProvider` (sandbox id `ratchet-sim`); live uses `TofuSandboxProvider` (id `ratchet-sandbox`). The two never share a row. |
| PR url, teardown flags | Kept in `settings.json` (the ledger has no column); teardown checklist items 1 to 3 are *derived* from the optimization status. |
| Bench pipeline strip (Live agents) | *Derived* from the newest candidate's bench/confirm rows; the real bench runner will have live state to show. |

`DevEngine` writes everything through the normal `Ledger` API and judges candidates with the real
`evaluateRatchet`, so the data has the right shape. It seeds four optimizations into an empty ledger and keeps the
seeded "search-api" one running (one simulated agent turn every 2.5 s). It resumes any `running` optimization on startup.

### Live mode is built from Settings

`createEngineForApp(ledger, 'live', { live })` (`src/main/engine.ts`) reads the non-secret settings from `SettingsStore`
and the secrets from the keychain (main process only) and calls `createLiveRuntime`. It is built in stages:

1. **No sandbox applied yet** (`sandbox: null`): only plan / apply / destroy work (they need just the profile, the tofu
   binary and the module). Every other port is "unready" and fails with `not_configured` and a sentence saying what to set.
2. **After an apply** the service rebuilds the engine from the outputs stored in the ledger (`sandboxes.outputs`); the
   compute, artifacts and runner ports become real. Changing a setting or a secret also rebuilds it, except while an
   optimization is running (Settings then says the change applies once it has stopped).
3. GitHub (repository + token), the bench AMI and the runner AMI complete the wiring; `LiveRuntime.missing` lists what is left.

Rules enforced by the engine and covered by tests: a live optimization is created only inside an applied sandbox and is
linked to it (`optimizations.sandbox_id`); starting one re-checks; destroy is refused while an optimization in the sandbox is
baselining / running / stopping, while agent identities of its optimizations were not torn down, and while any tagged
instance is alive. The Settings repository must match the repository in the New optimization form (live runs use one clone
and one token).

* `EngineCreateOptimizationInput` still has no ratchet.yaml text. The UI sends `configYaml` as an extra property (only the
  dev engine uses it); the real engine reads `configPath` at `baseRef` from the repository.
* Mode switching (Settings) calls `createApp(mode)` again after `dispose()`.
* API keys: `SettingsStore.getSecret(name)` is main-process only; `secretSource(settings)` hands them to the engine.

## Checklist on a real Mac

### 1. Signed, notarized build

1. `pnpm install && pnpm --filter @ratchet/desktop build`.
2. Set `CSC_LINK` / `CSC_KEY_PASSWORD` (Developer ID Application cert) and `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`,
   `APPLE_TEAM_ID`; run `pnpm --filter @ratchet/desktop package`.
3. Verify: `codesign --verify --deep --strict --verbose=2 Ratchet.app`, `spctl -a -vvv -t install Ratchet.app`,
   `xcrun stapler validate Ratchet.dmg`.
4. Open the DMG on a *different* Mac (quarantine flag set): no Gatekeeper warning, app starts.
5. Confirm native module loading: the app opens and shows the seeded optimizations (proves `better-sqlite3` loads
   from `app.asar.unpacked` under the hardened runtime; the entitlement `disable-library-validation` in
   `build/entitlements.mac.plist` exists for this).
6. Confirm migrations ship: `Ratchet.app/Contents/Resources/migrations/*.sql` exists.
7. Unverified risk: electron-builder with pnpm workspaces sometimes misses production deps (`better-sqlite3`,
   `yaml`, `zod` are externalized from the main bundle). If the packaged app crashes with "Cannot find module",
   set `node-linker=hoisted` for the desktop package install, or bundle those deps.

None of the packaging config (`electron-builder.yml`, entitlements) has been run: only `electron-vite build` and
the headless Linux launch below were exercised.

### 2. Keychain / secrets

1. Settings → save an Anthropic key, OpenAI key and GitHub token. Pills turn to *Configured*; the input clears and
   never shows the value again.
2. Quit and relaunch: still *Configured*. `~/Library/Application Support/Ratchet/settings.json` contains base64
   cipher text only (`grep -c sk-ant settings.json` is 0).
3. Keychain Access shows a "Ratchet Safe Storage" item. Deny access once: Settings shows the
   "keychain is unavailable" banner instead of crashing.
4. Clear removes the key; relaunch confirms.
5. Open DevTools (View → Toggle) and run `window.ratchet.getSettings()`: only booleans. Search the renderer's
   network/IPC for key material: none.

### 2b. Live mode: Settings, then the sandbox (needs a sandbox AWS account, `tofu`, a GitHub token)

1. Settings → Mode stays **Simulation**. Fill in the AWS section: profile (an SSO profile after `aws sso login --profile <p>`
   works; pick *SSO* so errors say what to run), region, then **Test connection**: expect `signed in to account <id> in <region>`.
   A wrong or expired profile must say so (for SSO it appends the `aws sso login` command). The account id must be the
   dedicated sandbox account.
2. GitHub: repository `owner/name`, store the token above, **Test connection**: expect `<repo> (private|public, default branch …); the token can push`.
   A read-only token must be reported as "cannot push". Anthropic / OpenAI rows: store a key, **Test connection** per key
   (a wrong key reports `rejected the key (401)`).
3. OpenTofu: leave both empty to use the defaults shown as placeholders (packaged app: `Resources/infra/aws` and, if present,
   `Resources/bin/tofu`; checkout: `infra/aws` and `tofu` on PATH). **Test connection**: expect `OpenTofu 1.x, module …`.
4. Runner / bench AMIs: ids from your image builds. Format errors appear as you type; Save stays disabled until valid.
5. The *Live readiness* list now shows everything ok except *Sandbox applied*.
6. Switch to **Live**. Environments: the badge says *Live account* and *Not planned yet*; nothing ran. **Plan** runs `tofu plan`
   (watch `<userData>/sandboxes/ratchet-sandbox/` fill with `vars.tfvars.json`, `plan.tfplan`) and shows the diff: every
   resource is `+ create` (see `docs/real-env/aws.md` §1 for the expected list). Re-opening the screen must **not** run tofu again.
7. **Apply sandbox** → the confirmation names the account, region and profile and shows the plan summary → **Confirm apply**.
   The badge goes *Applying…*, then *Applied*; the Outputs card shows the real bucket names, boundary ARN and subnet. Check the
   ledger: `sqlite3 ratchet.db "select id,provider,status,account_id,applied_at from sandboxes"` → `ratchet-sandbox|aws|applied|<acct>|…`
   and `select json_extract(outputs,'$.artifactsBucket') from sandboxes`.
8. Quit and relaunch in live mode: the sandbox is still *Applied* (status and outputs come from the ledger) and the live engine is
   rebuilt from them (New optimization preflight: *Sandbox applied* ok).
9. Kill the app (`kill -9`) in the middle of an apply: after relaunch the sandbox shows *Failed* with "interrupted"; **Apply sandbox**
   again converges (tofu is idempotent).
10. New optimization before any apply (or after a destroy): preflight *Sandbox applied* is red and creating the optimization is
    refused with "apply it first". After the apply it is created and `select sandbox_id from optimizations` shows `ratchet-sandbox`.
11. Destroy guards, in this order (each must leave the status *Applied* and change nothing in AWS):
    a. a running optimization in the sandbox → the button is disabled with "Stop optimization … first";
    b. stopped but not torn down → "agent identities … run its teardown first";
    c. launch a tagged instance by hand (`aws ec2 run-instances …` with `ratchet:opt=<id>` in the sandbox VPC) → "tagged instance(s) still alive";
    d. after teardown (Result → Run teardown) and no instances: **Destroy sandbox** → confirmation → *Destroying…* → *Destroyed*, outputs
       back to "after apply"; run the zero-orphan list at the end of `docs/real-env/aws.md` §8.
12. Secrets: DevTools → `window.ratchet.getSettings()` returns booleans for the three secrets and no values; `grep -c <key prefix> settings.json`
    is 0; `ps eww` on the `tofu` process shows `AWS_PROFILE` but none of the keys or the token.

Unverified: bundling a pinned `tofu` into `Resources/bin/` (electron-builder only ships `infra/aws` today); behaviour when an SSO session
expires during a long apply; whether one `tofu show -json` is fine for a large plan.

### 3. Screens against the live engine

For each screen, run a real optimization on `examples/demo-search` first (see `docs/real-env/engine.md`).

* **Optimizations**: KPI numbers match the ledger (`sqlite3 ratchet.db` queries: wins = rows in `ratchet_moves`,
  spend = `sum(spend_events.usd)`). Live card and chart update while agents run (events push; 10 s fallback poll).
  Unit labels are guessed from the metric key (`unitForMetric`); confirm they are right for your metric.
* **New optimization**: edit each field; the generated yaml validates through IPC (`parseConfig`). Paste a broken
  yaml in *Edit* and check the error paths. Preflight must go red for missing AWS profile / keys in live mode.
  "Run baseline & start" creates the optimization and lands on Live agents; the baseline appears in the ledger.
* **AWS sandbox**: plan list is the real `tofu plan` (counts match `tofu show`); outputs match `infra/contract.md`;
  account id, region and profile are the real ones; Apply / Destroy follow 2b. The diagram is static.
* **Live agents**: the hypothesis tracks the agent's latest message; log lines stream in order without gaps or
  duplicates across a reconnect (cursor is the ledger event id); Message reaches the agent's next turn; Pause /
  Resume change the runner state; **Stop** (confirm, then kill switch) revokes identities and terminates instances
  (check IAM and EC2 in the console). The bench strip should reflect the real bench runner.
* **Benchmark history**: chart steps equal `ratchet_moves`; noise band equals the baseline noise floor; verdict
  pills and reasons match the ratchet rule; histogram uses the baseline samples vs the best candidate's bench samples.
* **Result**: *Open pull request* creates the PR through the engine and shows its url; *Export report* opens the
  native save dialog and writes Markdown; *Run teardown* destroys the sandbox. The teardown checklist is derived
  from status plus a local flag today: wire it to real teardown state.

### 4. Not covered by anything yet

* Windows / Linux packaging (config exists for AppImage, untested).
* Auto-update.
* The "time by code path" and "winning diff" panels from the design (§9 Result): no data source in the ledger yet.
* "Open session" on an agent card: needs the runner's session URL from the engine.

## Running the app headless (what dev/CI did)

```
pnpm --filter @ratchet/desktop build
PLAYWRIGHT_MODULE=/path/to/node_modules/playwright \
  xvfb-run -a node packages/desktop/scripts/screenshots.mjs ./shots
```

Launches the built app under Electron + Playwright with a throwaway user-data dir and screenshots every screen,
including a create-and-start run through the real IPC. Under Xvfb the OS keychain is unavailable, so the Settings
screenshot shows the keychain banner (expected there).
