# ratchet-cli: parity with `packages/cli`

`packages/cli` (commander, TypeScript) is the executable spec. The binary is still called `ratchet`. The offline
commands (`validate`, `init`, `baseline`) are ports that only need `ratchet-core`; everything else is new: a thin
client of the `ratchetd` socket API, every command with `--json`.

All tests live in `tests/cli/` (one integration binary, `cli::`): `offline.rs` (the ports), `daemon.rs` (against an
in-process `ratchetd` on a temp socket), `binary.rs` (the real executable). Unit tests are next to the code.

## Differences on purpose

| TypeScript | Rust | Why |
|---|---|---|
| commander usage errors exit 1 | clap usage errors exit **2** (help and version 0) | the usual convention; scripts can tell "wrong command line" from "it failed". Exit codes: 0 ok, 1 failure, 2 usage |
| `ratchet cost reconcile <id> --db <file> [--lag-hours] [--accept-estimated]` opens the ledger and calls AWS | `ratchet cost reconcile <id>` asks the daemon | the daemon is the single writer of the ledger (and will own the AWS credentials); the flags belong to the engine's reconciler. Same table, same `pending` / `unchanged` / `reconciled` headlines |
| `Io` with `out` / `err` closures | `Io` trait (`ProcessIo`, `BufferIo`) | same idea, `&dyn Io` |
| test benches are `node bench.mjs` / `node -e ...` | `sh bench.sh` / `sh -c` | the tests need no Node. The cases that run the **templates' own** `node` files (`cold-start.mjs`, `backtest.mjs`) are kept and skip when `node` is missing |
| `baseline --repeats 0` is rejected by the command | clap rejects it first (exit 2); the library entry point still has the check and the original message | `positive_int` is a parser |
| `--pack` / `--list-packs` | unchanged, still aliases | `docs/SIMPLIFIED_MODEL.md` |

New: `--json` on every command (errors go to stderr as `{"error": {code, message, fields}}`), `--socket` /
`--token-file` / `--token` (and `RATCHET_SOCKET` / `RATCHET_TOKEN_FILE` / `RATCHET_TOKEN` / `RATCHET_HOME`),
`templates`, and the daemon commands: `status`, `list`, `show`, `run [--demo|repo] [--wait]`, `stop`, `report [--out]`,
`pr`, `candidates`, `history`, `spend`, `activity`, `agent pause|resume|message|log`, `events`, `env list|end`,
`cost reconcile`, `repos`, `inspect`, `overview`, `connectors`.

## Test mapping

### `program.test.ts` (6) -> `tests/cli/offline.rs`

| TS case | Rust test |
|---|---|
| validate [path] | `program_validate_and_baseline_wiring` |
| validate returns 1 for a missing file | `validate_exits_1_when_the_file_is_missing` |
| baseline [path] --repeats N --json | `program_validate_and_baseline_wiring` |
| baseline propagates failures as exit code 1 | `program_validate_and_baseline_wiring` |
| rejects bad option values with a non-zero exit code | `program_rejects_bad_option_values_and_unknown_commands` |
| unknown command is a usage error; --help exits 0 | `program_rejects_bad_option_values_and_unknown_commands`, `binary::exit_codes_are_0_1_and_2` |

### `commands/validate.test.ts` (4)

| TS case | Rust test |
|---|---|
| prints a summary and exits 0 for a valid config | `validate_prints_a_summary_and_exits_0_for_a_valid_config` |
| prints errors with YAML paths and exits 1 for an invalid config | `validate_prints_errors_with_yaml_paths_and_exits_1_for_an_invalid_config` |
| exits 1 when the file is missing | `validate_exits_1_when_the_file_is_missing` |
| defaults to ./ratchet.yaml in the cwd | `binary::validate_defaults_to_ratchet_yaml_in_the_current_directory` (a real process with `current_dir`; no `chdir` in a test binary) |

New: `validate_json_has_the_summary_or_the_errors`.

### `commands/baseline.test.ts` (13)

| TS case | Rust test |
|---|---|
| runs bench.repeats times and prints stats and the noise floor | `baseline_runs_the_repeats_and_prints_stats_and_the_noise_floor` |
| --repeats overrides the config | `baseline_repeats_override_the_config` |
| --json prints a single JSON document and nothing else on stdout | `baseline_json_is_a_single_document_and_nothing_else_on_stdout` |
| honours --k and --min-absolute | `baseline_honours_k_and_min_absolute` |
| min_gain dominates required gain when runs are stable | `baseline_min_gain_dominates_required_gain_when_runs_are_stable` |
| runs in the config file directory and uses the last JSON object | `baseline_runs_in_the_config_directory_and_uses_the_last_json_object` |
| fails when the metric is missing / not a number / stdout has no JSON | `baseline_fails_when_the_metric_is_missing_not_a_number_or_there_is_no_json` |
| fails with stderr tail when the bench exits non-zero | `baseline_fails_with_the_stderr_tail_when_the_bench_exits_non_zero` |
| times out a hung run | `baseline_times_out_a_hung_run_and_kills_what_it_started` (also checks the process group is killed, not waited for) |
| rejects an invalid config before running anything | `baseline_rejects_an_invalid_config_before_running_anything` |
| rejects invalid repeats | `baseline_rejects_invalid_repeats` |

### `commands/init.test.ts` (15, 7 of them `it.each`)

| TS case | Rust test |
|---|---|
| lists the templates and, separately, the demos | `init_lists_the_templates_and_separately_the_demos` |
| --list-packs is the same listing | `init_list_packs_is_the_same_listing_and_templates_is_too` |
| %s: writes files that pass `ratchet validate` (x7) | `init_writes_files_that_pass_validate_for_every_template` (a loop over the registry) |
| honors --metric, --direction and --value-usd | `init_honors_metric_direction_and_value_usd` |
| quotes a metric that needs it, and the result still validates | `init_quotes_a_metric_that_needs_it_and_the_result_still_validates` |
| refuses to overwrite without --force, and writes nothing then | `init_refuses_to_overwrite_without_force_and_writes_nothing_then` |
| --dry-run prints the files and writes nothing | `init_dry_run_prints_the_files_and_writes_nothing` |
| rejects bad input | `init_rejects_bad_input` |
| perf is the cli-cold-start template, forecasting is forecast-backtest | `init_pack_is_a_legacy_alias_and_says_so` |
| --template wins over --pack, and a template name works as a pack | `init_template_wins_over_pack_and_a_template_name_works_as_a_pack` |
| cli-cold-start: node bench/cold-start.mjs reports the objective metric | `the_files_a_template_writes_actually_run` (needs `node`) |
| forecast-backtest: deterministic, plausible WAPE | `the_files_a_template_writes_actually_run` (needs `node`) |
| `ratchet baseline` runs a generated config end to end and explains the calibration | `baseline_runs_a_generated_config_end_to_end_and_explains_the_calibration` (needs `node`) |
| init --template / --pack / --list-templates / --list-packs | `init_program_wiring` |

### `commands/cost.test.ts` (4) -> `tests/cli/daemon.rs::report_pr_and_cost`

| TS case | Rust |
|---|---|
| writes corrections, prints a table, and is idempotent | first call `reconciled: ... correction row(s) written` + a `written` line; second `--json` call `status: unchanged`, `writtenUsd: 0` |
| reports pending and writes nothing inside the billing lag | a running optimization: `pending: nothing was written` (the `--lag-hours` knob is the engine's) |
| fails clearly: no ledger, missing file, unknown optimization, billing errors | unknown optimization: exit 1 `not found`; "no ledger" / "missing file" no longer exist as the CLI never opens the file |
| shows up in --help | `ratchet cost reconcile --help` |

## New (no TypeScript counterpart)

| Area | Rust tests |
|---|---|
| Daemon commands | `daemon::status_shows_the_daemon_and_the_optimizations`, `list_show_and_the_per_run_reads`, `run_creates_starts_and_stop_ends`, `report_pr_and_cost`, `agents_pause_resume_message_and_log`, `environments_list_and_end`, `repos_inspect_overview_and_connectors` |
| `run --wait` (follows the SSE stream, exit 0 only when the run completed; `--json`; `--wait-timeout`) | `daemon::run_wait_follows_the_events_until_the_run_ends`, `run_wait_gives_up_after_its_timeout_with_exit_1` |
| `events` (JSON lines, `--last-event-id`, `--count`, `--timeout`, resync) | `daemon::events_streams_json_lines_and_resumes` |
| Errors: daemon down, no token, wrong token, field errors, `--json` error objects | `daemon::connection_problems_are_explained`, `run_needs_a_target_and_reports_the_daemons_complaints` |
| The executable: exit codes 0/1/2, `RATCHET_SOCKET`, `RATCHET_HOME`, `RATCHET_TOKEN` | `binary::*` |
| Client: SSE parsing, token resolution | `client::tests` (unit); `cli::tests` (durations, numbers, clap consistency) |
