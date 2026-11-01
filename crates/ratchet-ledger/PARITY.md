# ratchet-ledger: parity with `packages/core/src/ledger`

`packages/core/src/ledger` (drizzle + better-sqlite3) is the executable spec. This crate is the same ledger on
`sqlx` (SQLite, runtime-checked queries: building needs no database). It also runs the two pieces of `ratchet-core`
that are written against the ledger, so their TS tests are ported here: the sandbox lifecycle
(`sandbox/lifecycle.ts`) and the cost reconcile (`cost/reconcile.ts`).

| What | How it is held to the TS |
|---|---|
| Schema | `migrations/0001_init.sql` is the drizzle `0000_init.sql` minus the `--> statement-breakpoint` markers. `tests/migration.rs` applies the original drizzle file to a second database and requires **identical DDL for all 24 objects** (10 tables, 10 indexes, 4 triggers) from `sqlite_master`. |
| Append-only | The same triggers; `tests/ledger.rs` and `tests/properties.rs` try UPDATE/DELETE on `ratchet_moves` and `spend_events`. |
| Row shapes | `ratchet_core::rows` (camelCase JSON, as drizzle's `$inferSelect`), asserted in `listOptimizations` ("exact table shape") and the JSON round trip test. |
| Money, JSON columns | USD `REAL`, JSON `TEXT`; `serde_json` runs with `float_roundtrip` so stored samples read back bit for bit. |

## Test mapping

### `ledger/ledger.test.ts` (34) -> `tests/ledger.rs` (35 tests)

| TS case | Rust test |
|---|---|
| openLedger: applies migrations and enables foreign keys | `applies_migrations_and_enables_foreign_keys` |
| openLedger: uses WAL for file databases and is idempotent on reopen | `uses_wal_for_file_databases_and_is_idempotent_on_reopen` (+ `migration.rs::opening_twice_applies_the_migration_once`) |
| openLedger: enforces foreign keys | `enforces_foreign_keys` |
| optimizations: creates and fetches with defaults | `creates_and_fetches_an_optimization_with_defaults` |
| optimizations: validates input and rejects bad enums at the DB level | `validates_input_and_rejects_bad_enums_at_the_db_level` (the typed enums cannot hold a bad value; the CHECK is exercised with raw SQL) |
| optimizations: stamps started/ended timestamps | `stamps_started_and_ended_timestamps_on_status_changes` |
| baseline: records, reads and re-records | `records_reads_and_re_records_the_baseline_before_the_ratchet_moves` |
| baseline: refuses to change the baseline after a move | `refuses_to_change_the_baseline_after_a_move` |
| baseline: honours an explicit value override | `honours_an_explicit_value_override` |
| agents: lists agents in the order they were added | `lists_agents_created_together_in_the_order_they_were_added` (+ proptest `agents_come_back_in_the_order_they_were_added`) |
| agents: adds, lists and updates | `adds_lists_and_updates_agents` |
| candidates: submits, rejects foreign agents / duplicate shas | `submits_a_candidate_and_rejects_foreign_agents_and_duplicate_shas` |
| candidates: records bench results and sets verdicts | `records_bench_results_and_sets_verdicts` |
| candidates: candidatesWithResults | `candidates_with_results_joins_agent_results_headline_values_and_ratchet_flag` |
| ratchet: moves strictly forward when minimizing | `moves_strictly_forward_when_minimizing` |
| ratchet: ... when maximizing | `moves_strictly_forward_when_maximizing` |
| ratchet: rejects moves without a baseline, foreign/unknown candidates, non-finite values, repeats | `rejects_moves_without_a_baseline_foreign_candidates_non_finite_values_and_repeats` |
| ratchet: ratchetHistory | `ratchet_history_carries_candidate_agent_and_step_gain` |
| ratchet: helpers isImprovement and gainPercent | `ratchet_core::rows::tests::helpers_is_improvement_and_gain_percent` |
| append-only: ratchet_moves | `blocks_update_and_delete_on_ratchet_moves` |
| append-only: spend_events | `blocks_update_and_delete_on_spend_events` |
| append-only: still allows INSERT | `still_allows_insert_and_mutable_tables_remain_mutable` |
| spend: records with defaults and validates | `records_spend_with_defaults_and_validates` |
| spend: aggregates totals and breakdowns | `aggregates_totals_and_breakdowns_by_category_and_agent` |
| listOptimizations: exact table shape | `list_optimizations_returns_the_exact_table_shape` |
| listOptimizations: maximize, no baseline, ordering/filtering | `list_optimizations_handles_maximize_no_baseline_ordering_and_filtering` |
| recentActivity | `recent_activity_merges_verdicts_and_moves_newest_first_with_limit_and_filter` |
| agent events: appends and pages by id | `agent_events_append_and_page_by_id` |
| transaction: rolls back on error | `transaction_rolls_back_on_error` |
| generic columns: stores pack, evaluator kind, value model | `stores_pack_evaluator_kind_and_the_value_model` |
| generic columns: rejects bad evaluator kinds / non-positive values | `rejects_non_positive_values_and_bad_columns_at_the_db_level` |
| sandboxes: records idempotently, stamps apply/destroy, links optimizations | `sandboxes_record_idempotently_stamp_apply_destroy_and_link_optimizations` |
| sandboxes: fills in the account, clears outputs | `sandboxes_fill_in_the_account_on_apply_and_clear_outputs_on_destroy` |
| sandboxes: rejects unknown sandboxes and bad statuses | `sandboxes_reject_unknown_ids_and_bad_providers` |

### `sandbox/lifecycle.test.ts` (10) -> `tests/sandbox_lifecycle.rs` (11 tests)

All ten, against the real SQLite ledger: `reports_an_absent_sandbox_then_walks_planned_applying_applied`,
`records_a_failed_apply_keeps_the_error_and_can_be_retried`,
`destroys_with_the_input_it_was_applied_with_clears_outputs_and_is_idempotent`,
`refuses_to_destroy_something_never_applied_or_unknown`,
`refuses_to_destroy_while_an_optimization_in_the_sandbox_is_running`,
`refuses_while_the_orphan_check_reports_leftovers_and_runs_it_fail_closed`,
`restores_the_previous_status_when_the_providers_own_teardown_guard_refuses`,
`marks_a_destroy_that_failed_inside_the_provider_as_failed_and_allows_a_retry`,
`rejects_a_second_concurrent_operation_on_the_same_sandbox`, `recover_marks_interrupted_operations_as_failed`; and
`the_core_mock_provider_drives_the_same_lifecycle` (extra: the `ratchet_core::ports::mock` provider).

### `cost/reconcile.test.ts` (6) -> `tests/cost_reconcile.rs` (7 tests)

All six: `writes_positive_and_negative_corrections_leaving_estimates_and_tokens_alone`,
`is_idempotent_and_later_data_writes_only_the_remainder`, `is_pending_while_the_optimization_runs_or_inside_the_billing_lag`,
`is_pending_when_billing_is_flagged_incomplete_or_returns_no_data`,
`empty_billing_is_fine_when_the_ledger_holds_no_infrastructure_estimate`, `honours_lag_ms_and_propagates_billing_errors`;
extra: `concurrent_reconciles_correct_a_gap_only_once` (six concurrent reconciles write one correction).

## Tests that only exist in Rust

- `tests/migration.rs`: schema identical to the drizzle migration; migration applied once across reopens.
- `tests/properties.rs` (proptest): the ratchet only turns one way and its history is a chain of strict
  improvements (both directions, random attempt sequences); spend adds up by category, agent and estimate flag;
  the append-only tables reject every UPDATE/DELETE; agents keep insertion order.
- `tests/ledger.rs`: `engine_kv` (`kv_get/set/delete/prefix`, `_` and `%` are not wildcards in prefixes), the JSON
  round trip of a `Candidate`.

## Differences from the TypeScript

| TS | Rust | Why |
|---|---|---|
| synchronous `better-sqlite3`, `ledger.transaction(fn)` | async `sqlx` pool; `Ledger::transaction(|conn| Box::pin(async move {...}))` runs `BEGIN IMMEDIATE`, commits on `Ok`, rolls back on `Err` | The API is async end to end. Memory databases use one connection (a second one would be a different database); file databases use WAL and 4 connections. |
| `LedgerError.code` strings | `LedgerError { code: LedgerErrorCode, message }` in `ratchet-core`; SQLite failures are `LedgerErrorCode::Database` with SQLite's text (`UNIQUE constraint failed`, `CHECK constraint failed`, `FOREIGN KEY constraint failed`, `... is append-only`) | The traits in `ratchet-core` need one error type. |
| `setAgentStatus(id, status, runnerInstanceId?)` (undefined keeps, null clears) | `Patch::{Keep, Set, Clear}`; same for sandbox `outputs` / `account_id` | Explicit three-state patch. |
| `Ledger.sqlite` / `Ledger.db` handles | `Ledger::pool()` | Raw SQL in tests and diagnostics. |
| `engine_kv` used through `packages/engine/src/kv.ts` (it re-creates the table) | `kv_get/kv_set/kv_delete/kv_prefix` on `Ledger`; the table is in the migration | The engine port will use these. |
| ids `opt_xxxxxxxxxxxxxxxx` from `randomUUID` | `OptimizationId::generate()` etc. in `ratchet-core` (same format) | |
| `now` option (`() => number`) | `LedgerOptions { clock: Option<Arc<dyn Clock>> }` | Same `Clock` port as the engine. |

## Gaps

- `Ledger::list_optimizations` issues one query per table and joins in memory, like the TS; there is no pagination.
  Fine for v0 (hundreds of rows); revisit with the daemon.
- No online migration path: pre-release there is one migration (edit it in place), as in the TS.
