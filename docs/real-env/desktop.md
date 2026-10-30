# Real-environment testing: desktop app

`packages/desktop` (Electron main + React renderer) is complete against the **dev engine stub**. This file lists
what to verify on a real Mac, and exactly what is stubbed until the real engine lands.

## What is real vs. stubbed

| Piece | State |
|---|---|
| Ledger (`openLedger(<userData>/ratchet.db)`), all reads, view-model mappers | **Real.** Every screen reads the SQLite ledger through `Ledger` methods. |
| Typed IPC, zod input validation, sender check, event push | **Real.** |
| Secrets (`safeStorage`), settings file | **Real** code; the OS keychain path is only exercised on a real desktop session. |
| Engine (`EngineApi`) | **Stub.** `createEngineForApp(ledger, mode)` in `src/main/engine.ts` returns `DevEngine`. |
| `SandboxProvider` | **Stub.** `DevSandboxProvider` returns a fixed OpenTofu plan and fake outputs (account `000000000000`). |
| PR url, teardown flags | Kept in `settings.json` (the ledger has no column); teardown checklist items 1 to 3 are *derived* from the optimization status. |
| Bench pipeline strip (Live agents) | *Derived* from the newest candidate's bench/confirm rows; the real bench runner will have live state to show. |

`DevEngine` writes everything through the normal `Ledger` API and judges candidates with the real
`evaluateRatchet`, so the data has the right shape. It seeds four optimizations into an empty ledger and keeps the
seeded "search-api" one running (one simulated agent turn every 2.5 s). It resumes any `running` optimization on startup.

### Swapping in the real engine

Replace the body of `createEngineForApp` (same signature, returns `{ kind, engine, sandbox, dispose }`). Nothing else
imports the dev engine. Things the real engine must provide that the stub fakes:

* `EngineCreateOptimizationInput` has no ratchet.yaml text. The UI sends `configYaml` as an extra property (the dev
  engine uses it; the real engine should read `configPath` at `baseRef`, or accept the field).
* Mode switching (Settings) calls `createApp(mode)` again after `dispose()`.
* API keys: `SettingsStore.getSecret(name)` is main-process only; pass it into the engine from `createEngineForApp`.

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

### 3. Screens against the live engine

For each screen, run a real optimization on `examples/demo-search` first (see `docs/real-env/engine.md`).

* **Optimizations**: KPI numbers match the ledger (`sqlite3 ratchet.db` queries: wins = rows in `ratchet_moves`,
  spend = `sum(spend_events.usd)`). Live card and chart update while agents run (events push; 10 s fallback poll).
  Unit labels are guessed from the metric key (`unitForMetric`); confirm they are right for your metric.
* **New optimization**: edit each field; the generated yaml validates through IPC (`parseConfig`). Paste a broken
  yaml in *Edit* and check the error paths. Preflight must go red for missing AWS profile / keys in live mode.
  "Run baseline & start" creates the optimization and lands on Live agents; the baseline appears in the ledger.
* **AWS sandbox**: plan list is the real `tofu plan` (counts match `tofu show`); outputs match `infra/contract.md`;
  account id, region and profile are the real ones. The diagram is static.
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
