# ratchetd: parity with `packages/desktop/src/main`

`ratchetd` replaces the Electron main process as the thing the UI talks to. The contract is `RatchetApi`
(`packages/desktop/src/shared/types.ts`, `channels.ts`) plus the view models and mappers (`mappers.ts`, `kpis.ts`,
`value.ts`, `metrics.ts`); the simulation is `dev-engine.ts` + `seed.ts` + `dev-stats.ts` + `dev-sandbox.ts`. The
TypeScript stays as the executable spec until the cut-over.

How identity is checked, beyond the ported cases:

| Check | Where | What it proves |
|---|---|---|
| **The contract file** | `tests/api/openapi.rs::the_committed_openapi_file_is_current`, `api/openapi.yaml` | The committed OpenAPI document is byte-identical to the one generated from the routes and types. Regenerate with `cargo run -p ratchetd -- openapi`. The Swift client is generated from this file. |
| **Round trip of every route** | `tests/api/contract.rs::every_route_round_trips_its_types` | Every operation in the document is called on an in-process daemon over the real socket; the body parses into its Rust type and serializes back to the same JSON (no lost, renamed or extra field). A meta-check fails when a documented route has no case, or a case has no route. |
| **camelCase on the wire** | `tests/api/openapi.rs::schema_property_names_are_camel_case` | utoipa ignores serde's `rename_all_fields`; a snake_case property in a schema means the Swift client would be generated wrong. |
| **Every `RatchetApi` method has a route** | `tests/api/electron_api.rs::every_method_of_the_electron_api_has_a_route` | The table maps each of the 51 IPC channels (and the event channel) to an operation id (and is checked against `channels.ts` while it exists). |
| **Snapshots** | `tests/api/snapshots/` (insta) | The route table, the schema names and the SSE event union. |

## Differences on purpose

- **No IPC, no zod, no preload.** zod validation of arguments became serde (400 `invalid_request` with a `fields`
  entry per problem) plus the format checks of `live-settings.ts` (`src/api/validate.rs`). `isTrustedUrl` / trusted
  senders became the bearer token on a 0600 socket (`tests/api/auth.rs`).
- **`pickLocalFolder`** is the app's `NSOpenPanel`; `POST /v1/repos/inspections` takes the chosen path.
  **`exportReport`** returns the markdown (`GET /v1/optimizations/{id}/report`); the save dialog is the app's.
- **`dev: LiveSettings & {defaults}`** is `advanced: {values, defaults}`; **`SettingsVM.engine.kind`** is `sim | real`
  (was `dev | real`).
- **`Optimization.status`** keeps the UI value `pr-open`; `UiStatus` is its own enum.
- **`ValidateConfigResult`** is `{ok, summary, errors}` instead of a union keyed on `ok`; **`EngineEventVM`**
  gained `hello`, `resync`, `github`, and (provisional) `environment-draft` and `connector`; `agent-event` carries the
  log line (`event`) so a log view can append without refetching.
- **Money and time** are as before (USD floats, epoch ms); `metricOptions` order follows the `BTreeMap` of the stats
  (alphabetical, percentiles sorted by number) where the TypeScript followed JSON insertion order.
- **Overview and connectors** are the TypeScript spec now (`shared/overview.ts`, `shared/connectors.ts`), one to one; see
  `api/CONTRACT.md`. The wire types of connectors are `ratchet-connectors::model`, re-exported by `ratchetd::api`. **Environment
  drafts** are the one provisional group left: no TypeScript to port yet, shapes from `design/v02.html` /
  `design/v02/*.png` and `docs/SIMPLIFIED_MODEL.md`; the tags say `(provisional)`.
- **Secrets** are not stored by the simulation (only whether one is set). `RealService` (`ratchetd serve --mode real`, the default)
  keeps them in a `ratchet_engine::desktop::SecretStore`: the macOS Keychain, or with `--secrets-file` a 0600 file; without either
  `PUT /v1/settings/secrets/{name}` answers 409. The per-boot API token keeps its file implementation.
- **`RealService`** implements every group, environment drafts included (the trait defaults still answer 501 `unsupported` for a service that cannot say). Connectors live in the connector store
  (`ratchet-connectors`) and the single-account calls of the first Settings (`setAwsProfile`, `setSecret`, `githubSignIn`, ...)
  work on top of them, as in `service.ts`; the first start migrates the old settings file's token, profile and keys into connectors.
  Its tests are `tests/api/real_service.rs` (ports of `service.test.ts`), see `crates/ratchet-engine/PARITY.md`.

## Test mapping

Rust paths are relative to `crates/ratchetd`. "unit" = a `#[cfg(test)]` module next to the code; `it::` = the
integration binary `tests/api/main.rs` (module paths).

### `mappers.test.ts` (22)

| TS case | Rust test |
|---|---|
| pure helpers: derives display units | `views::format::tests::units` |
| pure helpers: formats relative time | `views::format::tests::relative_times` |
| pure helpers: median / downsample | `views::kpis::tests::median_of_even_and_odd` (`downsample` is not used by any route and was dropped) |
| pure helpers: maps ledger status to the UI status, adding pr-open | `it::sim::pull_request_and_report_follow_the_wins` |
| maps optimization summaries | `it::sim::the_seed_has_a_live_run_two_finished_ones_and_a_draft` |
| values the improvement from the ledger value columns | `it::views::the_improvement_is_valued_from_the_ledger_columns` |
| builds the ratchet series | `it::sim::candidates_series_and_spend_agree_with_the_ledger` |
| maps candidates with verdicts, vs-baseline % and short shas | `it::sim::candidates_series_and_spend_agree_with_the_ledger` |
| maps spend with category labels, per-agent totals and a cumulative series | `it::sim::candidates_series_and_spend_agree_with_the_ledger` |
| merges activity newest-first without duplicating verified verdicts | `it::views::activity_is_newest_first_and_a_verified_verdict_shows_once` |
| computes range KPIs with change chips vs the previous equal-length range | `it::views::range_kpis_carry_change_chips_against_the_previous_equal_window` |
| omits change chips when the previous range has no data, and counts older wins in a longer range | `it::views::kpis_omit_chips_without_a_previous_window_and_a_longer_range_counts_older_wins` |
| drops the value KPI when no optimization has a value model; empty ranges say so | `it::views::the_value_kpi_exists_only_when_some_optimization_has_a_value_model` |
| lists the objective family as metric options | `views::metrics::tests::options_are_the_objectives_siblings_in_percentile_order`, `top_level_objective_lists_other_top_level_metrics` |
| compares baseline vs best for the family and the guardrail metrics, and the span timings | `views::metrics::tests::compare_rows_cover_the_family_and_the_guardrail_metrics`, `spans_pair_baseline_and_best`, `it::sim::the_seeded_detail_has_everything_the_result_screen_needs` |
| plots another metric of the family | `it::contract::every_route_round_trips_its_types` (`?metric=http_req_duration.p99`: no noise band) |
| builds the winning change from a diff, or says why there is none | `it::views::the_winning_change_comes_from_a_diff_or_says_why_there_is_none` |
| builds agent cards | `it::sim::the_seeded_detail_has_everything_the_result_screen_needs` |
| ignores user messages when picking the hypothesis | `it::views::the_hypothesis_is_the_latest_agent_message_not_the_users` |
| pages agent events with a cursor | `it::contract::the_agent_log_pages_by_cursor` |
| derives the bench pipeline from the newest candidate | `it::views::the_pipeline_follows_the_newest_candidate` |
| renders a markdown report | `it::sim::pull_request_and_report_follow_the_wins`, `it::views::reports_say_when_there_is_nothing_yet` |
| validateConfigText: summary and path-qualified errors | `src/api/summary.rs` tests (unit), `it::contract::every_route_round_trips_its_types` |

### `service.test.ts` (30)

| TS case | Rust test |
|---|---|
| GitHub row: signed out by default; the device flow shows a code, then signs in | `it::sim::timers_drive_a_run_without_ticks`, `it::contract::every_route_round_trips_its_types` (sign-out, sign-in), `it::sse::github_and_connector_events_are_published` |
| GitHub row: a denied sign-in / a rejected pasted token / a token from the environment | ported: `tests/api/real_service.rs::github_row` |
| GitHub row: a pasted token (Advanced) is checked first | `it::sim::settings_connections_and_preflight_tell_the_truth` (format check), `it::contract::failures_are_api_errors` |
| GitHub row: Test connection for GitHub | `it::sim::settings_connections_and_preflight_tell_the_truth` |
| repo picker: lists the account repositories (needs the sign-in) and searches them | `it::contract::every_route_round_trips_its_types`, `failures_are_api_errors` (409 `not_ready`) |
| repo picker: reads ratchet.yaml and summarizes it | `it::contract::every_route_round_trips_its_types` |
| repo picker: offers the templates when there is no ratchet.yaml, and shows why an invalid one is invalid | `it::contract::every_route_round_trips_its_types`, `it::sim::repos_and_inspection` |
| repo picker: a local folder | `it::sim::repos_and_inspection` |
| repo picker: without a folder dialog, picking returns nothing | dropped (the app's `NSOpenPanel`) |
| templates: lists the gallery | `it::contract::every_route_round_trips_its_types` |
| templates: proposeTemplate opens the pull request | `it::sim::template_proposals_use_the_core_templates` |
| templates: surfaces the engine refusing (files already exist) | ported: `tests/api/real_service.rs::templates::surfaces_the_engine_refusing_files_already_exist` |
| creating: a real run uses owner/name; what is missing stops it before anything is created | `it::sim::creating_validates_and_checks_readiness` |
| creating: a demo goes to the simulation and needs no repository | `it::sim::creating_validates_and_checks_readiness`, `it::contract::every_route_round_trips_its_types` |
| preflight: a demo needs nothing | `it::sim::settings_connections_and_preflight_tell_the_truth` |
| preflight: a real run names each missing thing, then turns green | `it::sim::settings_connections_and_preflight_tell_the_truth` |
| preflight: a signed-out harness is missing | `it::contract::every_route_round_trips_its_types` (`agent-codex` fails) |
| preflight: cloud runners need API keys | `it::sim::settings_connections_and_preflight_tell_the_truth` |
| AWS: lists profiles, remembers the choice and the kind of sign-in, defaults the region | `it::contract::every_route_round_trips_its_types` |
| AWS: Test connection passes profile and region, never invents a profile | `it::sim::settings_connections_and_preflight_tell_the_truth` |
| agents: detects the harnesses once and again on demand | `it::contract::every_route_round_trips_its_types` |
| the developer overrides are validated and kept apart | `src/api/validate.rs` tests (unit), `it::contract::failures_are_api_errors` (nothing saved on a bad field) |
| environments: phase, region, cost, when it removes itself | `it::sim::the_environment_lives_with_its_runs` |
| environments: idle shows the destroy time; ending it returns the picture with the receipt | `it::sim::the_environment_lives_with_its_runs`, `it::contract::every_route_round_trips_its_types` |
| environments: follows the sandbox events for the phase | `it::sse::hello_comes_first_and_events_follow_in_order` (`sandbox` events) |
| Result gets the receipt and the calibration in plain words | `it::sim::the_environment_lives_with_its_runs`, `the_seeded_detail_has_everything_the_result_screen_needs` |
| an engine that cannot say yields an empty, honest view | `service.rs` default bodies answer `501 unsupported` (provisional groups) |
| secrets never reach the renderer: no response, event or error carries a key | `it::electron_api::no_response_can_carry_a_secret`, `it::sim::settings_connections_and_preflight_tell_the_truth`, `connectors_keep_their_invariants` |
| exposes no method that returns a secret, and none that applies or destroys infrastructure | `it::electron_api::no_response_can_carry_a_secret` |

### `ipc.test.ts` (13)

| TS case | Rust test |
|---|---|
| registers a handler for every method in the schema table | `it::electron_api::every_method_of_the_electron_api_has_a_route`, `it::contract::every_route_round_trips_its_types` |
| the preload bridge exposes exactly the IPC methods, none that reads a secret | same two |
| serves reads from the ledger | `it::sim`, `it::views` (every read against seeded or hand-written rows) |
| accepts a well-formed createOptimization and returns the new id | `it::contract::every_route_round_trips_its_types` (201 `CreatedOptimization`) |
| a demo needs no repository | `it::sim::creating_validates_and_checks_readiness` |
| says so when the repository is not a GitHub owner/name | `it::sim::creating_validates_and_checks_readiness` (`InvalidFields` on `repoUrl`) |
| rejects untrusted senders without touching the service | `it::auth::every_route_needs_the_token` (401 on every route, with no token, a wrong one, and one byte short) |
| isTrustedUrl allows file:// and the dev server origin only | replaced by the socket mode and the token: `it::auth::the_socket_and_the_token_file_are_private` |
| forwards engine events and wires commands to the engine | `it::sse::*`, `it::sim::a_started_run_baselines_then_ticks_through_the_real_rule` |
| opens a PR (stored for the UI) and exports a report | `it::sim::pull_request_and_report_follow_the_wins` |
| exportReport reports cancellation | dropped (the save dialog is the app's) |

### `settings.test.ts` (5)

| TS case | Rust test |
|---|---|
| stores secrets encrypted, round-trips them, persists across instances | the Keychain belongs to the real engine; `auth::tests::the_file_store_is_private_and_atomic` covers the token store |
| refuses to store secrets when the keychain is unavailable | `SettingsVm.encryptionAvailable` is on the wire; the simulation always has one |
| treats environment-provided credentials as configured | ported: `src/real/settings.rs` tests |
| survives a corrupt settings file | ported: `src/real/settings.rs` tests |
| getSettings exposes only configured yes/no | `it::sim::settings_connections_and_preflight_tell_the_truth` |

### `overview.test.ts` (12)

`LedgerViews::overview` (`src/views/overview.rs`) is the port of `buildOverview`; the tests are `tests/api/overview.rs` (`it::overview::`),
run on the seeded simulation ledger at the TS test's fixed `NOW` (2026-10-30T12:00Z).

| TS case | Rust test |
|---|---|
| the numbers: sums spend and value over the range, and relates them (return per $1, payback in days) | `it::overview::sums_spend_and_value_over_the_range_and_relates_them` |
| the numbers: filters by range: 90 days adds the older optimization, all time starts at the first activity | `it::overview::filters_by_range_90_days_adds_the_older_optimization_all_time_starts_at_the_first_activity` |
| the numbers: the table adds up to the hero | `it::overview::the_table_adds_up_to_the_hero` |
| the numbers: the staircase is the cumulative value of the wins in the range, oldest first, naming each optimization | `it::overview::the_staircase_is_the_cumulative_value_of_the_wins_in_the_range` |
| the numbers: a row says what happened | `it::overview::a_row_says_what_happened` |
| no value model: shows no value and no return when nothing has a value model, but still adds up the spend | `it::overview::shows_no_value_and_no_return_when_nothing_has_a_value_model` |
| no value model: an empty ledger is all zeros and empty lists, over any range | `it::overview::an_empty_ledger_is_all_zeros_and_empty_lists_over_any_range` |
| no value model: measured only when every contributing model is measured | `it::overview::measured_only_when_every_contributing_model_is_measured` |
| needs you: lists finished runs with verified wins and no pull request, whatever the range | `it::overview::lists_finished_runs_with_verified_wins_and_no_pull_request` |
| needs you: a pull request takes it off the list and the status reads "PR open" | `it::overview::a_pull_request_takes_it_off_the_list_and_the_status_reads_pr_open` |
| needs you: a recent failed run asks for a look; an old one stops asking | `it::overview::a_recent_failed_run_asks_for_a_look_an_old_one_stops_asking` |
| needs you: a finished run without a win reads "No win" and is not waiting on anyone | `it::overview::a_finished_run_without_a_win_reads_no_win_and_is_not_waiting_on_anyone` |

The Rust seed (`src/sim/seed.rs`) now carries the TS seed's per-optimization value models (ingest: $400 per k/s, edge worker: $3,000 per s;
the draft has none); before, only the search demo was valued, which made every Overview number differ from the TS test.
Spend events are summed in `ORDER BY at, id` (the TS sums in table order); the difference is below the tests' tolerances.

### `dev-engine.test.ts` (12) and `seed.ts`

| TS case | Rust test |
|---|---|
| creates a draft with one agent per kind and the parsed objective | `it::contract::every_route_round_trips_its_types` |
| rejects an invalid config | not reachable through the API (the repo owns ratchet.yaml); `it::sim::repos_and_inspection` shows the errors |
| start records a baseline and moves to running | `it::sim::a_started_run_baselines_then_ticks_through_the_real_rule` |
| simulates turns through the real ratchet rule: monotonic ratchet, verdicts, spend, events | same |
| pause skips an agent, resume brings it back, messages show up as the next hypothesis | `it::sim::paused_agents_skip_their_turn_and_messages_are_read_once` |
| stops on budget exhaustion and on plateau | `it::sim::a_run_ends_when_the_budget_is_spent`, `a_run_ends_on_a_plateau` |
| stop is the kill switch | `it::sim::stop_ends_the_run_and_a_stopped_run_cannot_restart` |
| opens a PR only when there is a verified win | `it::sim::pull_request_and_report_follow_the_wins` |
| unknown ids fail loudly | `it::contract::failures_are_api_errors` |
| repoSlug normalises git urls | `sim::tests::repo_slug_normalises_remote_urls` |
| createEngineForApp: seeds an empty ledger, resumes the running demo | `it::sim::the_seed_has_a_live_run_two_finished_ones_and_a_draft`, `the_ledger_file_survives_a_restart_and_runs_resume` |
| createEngineForApp: does not reseed a ledger that already has data | `it::sim::the_ledger_file_survives_a_restart_and_runs_resume` |

## New (no TypeScript counterpart)

| Area | Rust tests |
|---|---|
| Auth | `it::auth::*` (401 on every route and on the event stream and the document, bearer parsing, 0600 socket and token file, private directory, per-boot token, a live socket is not taken over and a stale one is replaced, shutdown with a stream open removes both files, custom token store); `auth::tests`, `paths::tests` (unit) |
| SSE | `it::sse::*` (hello first, ids count up without gaps, `Last-Event-ID` replays exactly the missed events, resync when the id is too old or from before a restart, resume from `GET /v1/status`); `hub::tests` (unit) |
| OpenAPI | `it::openapi::*` (committed file current, yaml = json, snapshots, unique camelCase operation ids, every tag used, no dangling `$ref`, camelCase properties, enum wire values) |
| Provisional groups | `it::sim::connectors_keep_their_invariants`, `drafts_follow_their_life_cycle`, `tests/api/overview.rs` |
| Cost reconcile | `it::sim::cost_reconcile_is_pending_while_running_and_idempotent_afterwards` |
| LaunchAgent | `launchagent::tests` (plist content, install/uninstall with a recording `Launchctl`; `launchctl` is never called) |
| Timers | `it::sim::timers_drive_a_run_without_ticks` (baseline, turns, stop, device flow) |
