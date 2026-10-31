# The API contract and how type sharing is enforced

`api/openapi.yaml` is generated from the Rust types (`ratchetd openapi`, utoipa) and is the one description of the
wire format. Three things are checked against it, so a type that drifts fails a build instead of a user.

| Consumer | Check | Where |
|---|---|---|
| **Rust** | The committed file is byte-identical to the generated one; every route round-trips its types; property names are camelCase; the event union is snapshotted. | `crates/ratchetd/tests/api/` (`cargo test -p ratchetd`) |
| **TypeScript** (desktop view models) | Types are generated from the file with `openapi-typescript` (pinned `7.13.0`) into `packages/contract/src/api.d.ts`. `contract.check.ts` asserts that every view model, request and event of `packages/desktop/src/shared/types.ts` equals its generated counterpart. It runs in `pnpm typecheck` (`pnpm -r typecheck`, so also in `ci.yml`). | `packages/contract` |
| **Swift** | Enforced separately: the Mac app's client is generated from this same `openapi.yaml`, in the mac-app job of `.github/workflows/native.yml`. It does not use anything from `packages/contract`. (Status today: `apps/mac` still uses hand-written models and the mock client; generating the client in that job is the step that turns this row on.) | `apps/mac`, `.github/workflows/native.yml` |

## TypeScript: how it is wired

```
pnpm contract:gen     # regenerate packages/contract/src/api.d.ts from api/openapi.yaml
pnpm contract:check   # regenerate to a temp file, fail if the committed api.d.ts differs
pnpm typecheck        # = pnpm -r typecheck: runs contract:check, then tsc over contract.check.ts
```

After changing a Rust type: `cargo run -p ratchetd -- openapi`, then `pnpm contract:gen`, then `pnpm typecheck`.

- `packages/contract/src/contract.check.ts` is compiled with `tsc --noEmit` and never runs. It is a set of tables of
  `Same<A, B>` rows (exact equality after flattening intersections, so optional vs. `| null`, a renamed field, a
  widened union or a missing key all fail), `Within<A, B>` (one direction only) and `Sends<Ts, Wire, Extra>` (request
  bodies: what the client sends is accepted, and the only contract fields the client type lacks are the listed `Extra`).
- A failure reads `Type '{ failingRows: "ValueVM" | ...; details: ... }' does not satisfy the constraint 'true'`; the
  names are the rows of the table. Rust `FooVm` is TypeScript `FooVM`; the mapping is the table, nothing is derived
  from names.
- The table has five parts: the view models (schema by schema), the status rows, the SSE events, the request bodies and
  query parameters, and the operations (the result of each `RatchetApi` method against the response of the operation
  that replaces it; the method-to-operation table also lives in `crates/ratchetd/tests/api/electron_api.rs`).
- The check reads `packages/desktop/src/shared/types.ts`; the desktop app does not import `@ratchet/contract`.

## Intentional differences

Each row is spelled out in `contract.check.ts` (the `D…` comments), so a new difference cannot hide.

| # | TypeScript | Wire (Rust) | Why |
|---|---|---|---|
| D1 | `EngineInfo.kind: 'dev' \| 'real'` | `'sim' \| 'real'` | The daemon's simulation is called `sim`; `dev` named the in-repo Electron stub. The check maps `dev` to `sim`, so a third kind on either side still fails. |
| D2 | `SettingsVM.dev: LiveSettings & { defaults }` | `advanced: { values, defaults }` | An intersection has no clean schema; the nested form generates readable Swift. |
| D3 | `ValidateConfigResult` is a union keyed on `ok` | one object `{ ok, summary \| null, errors }` | A union tagged by a boolean is awkward in OpenAPI and Swift; the check derives the object from the union. |
| D5 | `TemplateVM` has 11 fields | `TemplateInfo` also has `defaultValueUsd`, `example` | The Rust type is the core registry entry; the desktop mapper trims it. Extra fields are harmless to a client that ignores them; the check strips exactly those two (also in `RepoInspectVM.templates` and `listTemplates`). |
| D6 | `Optimization.status` is the UI pill status (also `merged`, `blocked`) | `UiStatus` (no `merged` / `blocked`) | `merged` / `blocked` only come from the provisional overview. Checked one way: everything the daemon sends is renderable. |
| D7 | `EngineEventVM` has 8 members | also `hello`, `resync`, `github`, `environment-draft`, `connector` | `hello` / `resync` are the stream's own control events; the others belong to the app's new features. The set is asserted exactly. `environment-draft` is checked against `EnvironmentDraftEvent` of `@ratchet/core`. |
| D8 | `optimization` event: `status: string` | the closed ledger enum | Checked one way (wire narrower than the type). The TypeScript type is looser than the daemon; tightening it is a type-only change for later. |
| D9 | `agent-event` carries ids only | also `event: AgentEventVm \| null` | Lets a log view append without refetching. |
| D10 | `CreateOptimizationInput` has no `connectors`, `environmentDraftId` | accepted, optional | Features the desktop app does not have (connectors, environment drafts; `environmentDraftId` must name an approved draft, else 409). Listed as `Extra`; remove when the TypeScript side grows them. |
| D11 | `ProposeTemplateInput` has no `direction`, `metric`, `valueUsd` | accepted, optional | The daemon takes the template's starting direction, metric and value; the desktop app does not offer them. Listed as `Extra`. |
| D12 | `createOptimization` resolves to the id | `{ id }` | A bare string is not a JSON document. |
| D13 | `exportReport` saves a file | `GET …/report` returns `{ fileName, markdown }` | The save dialog is the app's; `pickLocalFolder` is likewise the app's (`inspectRepo` takes the path). Not compared. |
| D14 | `CreateOptimizationInput.connections`, `PreflightInput.repo` | no such field (the wire has `connectors`; preflight has no `repo`) | Provisional (connectors, see `PENDING`). The check leaves these two TypeScript-only keys out; remove when the connectors group is aligned. |

Also not compared because the desktop app has no counterpart: daemon `status`, `calibration`, `protected-paths`,
`spend/reconciliations`, `getTemplate`.

| E1 | `EnvironmentApproval.locked: true` | `locked: boolean` | An OpenAPI schema from utoipa has no literal type; the daemon always sends `true`. The check patches the TypeScript type. |
| D16 | `EnvironmentDrafter.draft / revise` resolve when the agent is done | `POST …/environment-drafts` and `…/revisions` answer at once with the draft in status `drafting` | A setup agent runs for minutes. `environment-draft` events (status, and `phase` / `text` progress lines) and `GET …/{id}` carry the rest. The record's shape is the same. |

Environment drafts are checked against the types of `@ratchet/core` (`packages/core/src/environment/types.ts`), not the desktop
view models, because the desktop app has no environment step yet: `EnvironmentDraftRecord` <-> `EnvironmentDraftVm`,
`EnvironmentApproval` <-> `EnvironmentApprovalVm`, `DraftEnvironmentInput` (without `onProgress`) <-> `CreateEnvironmentDraftInput`,
`EnvironmentDraftEvent` (added to `@ratchet/core` for the check) <-> the `environment-draft` event, and the `EnvironmentDrafter`
methods <-> the operations. HTTP-only additions: `GET /v1/environment-drafts` (list) and `DELETE …/{id}` (discard); the
provisional `…/files` and `…/pull-request` routes are gone (the files are in the draft; approving with `mode: pr` opens the PR).

## Mismatches found and fixed

1. **Nullable fields were optional in the OpenAPI.** utoipa treats every `Option<T>` as not required, but the daemon
   always writes `null` (no `skip_serializing_if`), and the TypeScript view models say `x: T | null`. The generated
   TypeScript therefore had `x?: T | null`. Fixed in Rust: `#[schema(required = true)]` on every `Option` field of a
   response type (99 fields: `crates/ratchetd/src/api/*.rs`, `TemplateInfo.holdout`, `Calibration.{cv,noise,resolvable}`
   in `ratchet-core`). Inputs and query structs stay optional. Wire bytes are unchanged; the document and the
   `ApiEvent` snapshot changed. Swift sees the same `T?`.
2. **`Kpi.unit` and `RatchetPoint.label`** are `?: string` in TypeScript (absent), and the UI relies on it
   (`StairPoint.label?: string`), but Rust sent `null`. Fixed in Rust: both are omitted when absent
   (`skip_serializing_if`, `nullable = false`), so the wire is `unit?: string` / `label?: string`.

No TypeScript type needed changing.

## Not checked yet (provisional)

`overview`, `connectors` and `connector selections` are being rewritten on
`claude/ts-overview-shell` and `claude/ts-connectors`. They are listed as `pending` (with a
`TODO`) in `PENDING` at the end of `contract.check.ts`; when a group lands, add one row per type to the matching table
and delete its line.
