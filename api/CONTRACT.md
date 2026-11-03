# The API contract and how type sharing is enforced

`api/openapi.yaml` is generated from the Rust types (`ratchetd openapi`, utoipa) and is the one description of the wire format.
Two things are checked against it, so a type that drifts fails a build instead of a user.

| Consumer | Check | Where |
|---|---|---|
| **Rust** | The committed file is byte-identical to the generated one; every route round-trips its types; property names are camelCase; the event union is snapshotted; the committed table of the Electron app's API methods resolves to real operations; no response can carry a secret. | `crates/ratchetd/tests/api/` (`cargo test -p ratchetd`) |
| **Swift** | The Mac app's client (`Sources/RatchetAPI`) is generated from this same `openapi.yaml` (`tools/generate-api.sh`, committed). The "API client is current" step of the mac-app job in `.github/workflows/native.yml` regenerates it and shows a warning when it differs. That step is **advisory** (`continue-on-error`): it builds swift-openapi-generator, so it must not turn the job red by itself. | `apps/mac`, `.github/workflows/native.yml` |

There used to be a third check, in TypeScript: `packages/contract` generated types from this file with `openapi-typescript` and
asserted (`contract.check.ts`, 159 rows) that the Electron app's view models equalled them. It went with the Electron app. The
table of intentional differences it documented (kinds `dev` / `real` became `sim` / `real`, `SettingsVM.dev` became
`advanced`, `ValidateConfigResult` one object instead of a union, environment-draft calls answering at once with status
`drafting`, the REST layout of the connector calls, ...) is at `fc503b1:api/CONTRACT.md`; the wire is what the Rust types and
`openapi.yaml` say today.

## After changing a Rust type

```
cargo run -p ratchetd -- openapi          # rewrites api/openapi.yaml (--check: exit 1 when it differs)
apps/mac/Ratchet/tools/generate-api.sh    # on a Mac: regenerates Sources/RatchetAPI
cargo test -p ratchetd                    # the contract tests and the snapshots
```

Review the diff of `api/openapi.yaml` and of the insta snapshots (`cargo insta review`).

## Wire conventions

- Property names are camelCase; ids are strings; times are epoch milliseconds (`createdAt`) unless a field says otherwise.
- **Nullable response fields are required.** utoipa treats every `Option<T>` as not required, but the daemon always writes
  `null` (no `skip_serializing_if`), so response types carry `#[schema(required = true)]` on every `Option` field and Swift sees
  `T?` rather than a missing key. Inputs and query structs stay optional. The few fields that are omitted when absent
  (`Kpi.unit`, `RatchetPoint.label`) say so with `skip_serializing_if` and `nullable = false`.
- Errors are `{ "code", "message", "fields" }` with the usual statuses (400 invalid, 401, 404, 409 conflict / not ready, 501
  unsupported group).
- Live updates are server-sent events on `GET /v1/events`. `hello` and `resync` are the stream's own control events (a client
  reloads everything on `resync`); the others announce a change (`optimization`, `agent`, `candidate`, `spend`, `run-active`,
  `github`, `connector`, `environment-draft`, ...) and a client refetches what they name.
- Connectors are one resource family: `GET /v1/connectors`, `POST /v1/connectors/{refresh,github,aws,agents}`,
  `POST` / `DELETE /v1/connectors/github/sign-in` (the device flow is one resource), `PATCH` / `DELETE /v1/connectors/{id}`,
  `POST /v1/connectors/{id}/{default,test}`, `GET` / `PUT /v1/connector-selections`. `UpdateConnectorInput.monthlyCapUsd`
  is `Option<Option<f64>>`: `null` removes the cap, absent leaves it.
- Environment drafts: `POST /v1/environment-drafts` and `…/revisions` answer at once with the draft in status `drafting` (a setup
  agent runs for minutes); `environment-draft` events and `GET …/{id}` carry the rest. Creating an optimization with an
  `environmentDraftId` requires an approved draft (409 otherwise).
- `AgentSelection.where` is `where_` in Rust and `where` on the wire (Swift escapes the keyword).

## Not on the wire

The save and open dialogs (`exportReport`, `pickLocalFolder`) are the app's: `GET …/report` returns `{ fileName, markdown }` and
`inspectRepo` takes a path. The daemon's own `status`, `calibration`, `protected-paths`, `spend/reconciliations` and `getTemplate`
have no screen yet.
