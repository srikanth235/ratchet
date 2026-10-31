# ratchet-agents parity with packages/agents

Every TypeScript test in `packages/agents/test` is mapped to the Rust test that replaces it (or marked **dropped** with the reason).
The TS suite has 158 test definitions (163 with `it.each` expanded); the Rust crate has 188 tests (20 unit tests in `src/`, the rest in
`tests/`). Rust test names are the TS names in snake case. File names: `tests/<ts-file-with-underscores>.rs`.

Legend: **ported** = same assertions; **ported (changed)** = same intent, assertion adapted (reason given); **dropped** = not ported.

## artifacts-race.test.ts -> tests/artifacts_race.rs

| TS test | Rust test |
|---|---|
| readEvents stops before an empty batch and returns it on the next poll | `read_events_stops_before_an_empty_batch_and_returns_it_on_the_next_poll` (ported) |
| the inbox retries a message that read as empty instead of dropping it | `the_inbox_retries_a_message_that_read_as_empty_instead_of_dropping_it` (ported; also asserts exactly one retry sleep) |

Added (no TS counterpart), `tests/artifacts_resilience.rs`: the inbox retries after a store error instead of ending (TS: a rejected
`store.get` killed the mailbox pump for good), skips a malformed message, and the event sink never writes over batches it could not list.

## claude-code.test.ts -> tests/claude_code.rs

| TS test | Rust test |
|---|---|
| ClaudeStreamParser: maps a full transcript | `parser_maps_a_full_transcript_to_status_message_tool_usage_events` |
| ...identical however the stream is chunked, including 1-byte chunks | `parser_is_identical_however_the_stream_is_chunked_including_one_char_chunks` |
| ...holds a partial last line / parses an unterminated final line on end() | `parser_holds_a_partial_last_line_and_parses_an_unterminated_final_line_on_end` |
| ...skips unknown event/block types and garbage lines | `parser_skips_unknown_event_and_block_types_and_garbage_lines` |
| ...reports error results as error events and keeps the usage | `parser_reports_error_results_as_error_events_and_keeps_the_usage` |
| ...treats error_max_turns as an error-subtype result | `parser_treats_error_max_turns_as_an_error_subtype_result` |
| ClaudeCodeAdapter: start builds the command line, pipes the prompt on stdin | `start_builds_the_command_line_pipes_the_prompt_on_stdin_and_returns_the_session` |
| ...honours binary path, permission mode, tool lists and prompt-as-argument | `honours_binary_path_permission_mode_tool_lists_and_prompt_as_argument` |
| ...resume: passes --resume, sends the message, keeps the session id | `resume_passes_resume_id_sends_the_message_and_keeps_the_session_id` |
| ...resume: follows a new session id if the CLI forks one | `resume_follows_a_new_session_id_if_the_cli_forks_one` |
| ...resume of an unknown session: non-zero exit with stderr | `resume_of_an_unknown_session_turns_a_non_zero_exit_with_stderr_into_an_error_and_an_error_event` |
| ...an is_error result fails the turn (error event emitted once) | `an_is_error_result_fails_the_turn_with_the_error_event_emitted_once` |
| ...error_max_turns ends the turn normally | `error_max_turns_ends_the_turn_normally_so_the_runner_can_carry_on` |
| ...a stream that ends without a result line is an error | `a_stream_that_ends_without_a_result_line_is_an_error_even_with_exit_zero` |
| ...a missing binary is reported, not thrown raw | `a_missing_binary_is_reported_not_raised_raw` |
| ...abort: signals the process group (SIGTERM) and rejects with AgentAbortedError | `abort_signals_the_process_group_with_sigterm_and_fails_with_aborted` (`AgentError::Aborted`) |
| ...an already-aborted signal never lets the turn proceed | `an_already_aborted_signal_never_lets_the_turn_proceed` |
| ...preflight: ok with the binary present and a key | `preflight_is_ok_with_the_binary_present_and_a_key` |
| ...preflight: reports a missing key and a missing binary | `preflight_reports_a_missing_key_and_a_missing_binary` |
| ...preflight: a subscription login can be allowed explicitly | `preflight_allows_a_subscription_login_explicitly` |

## codex.test.ts -> tests/codex.rs

| TS test | Rust test |
|---|---|
| CodexStreamParser: maps items to message/tool events and turn.completed to usage | `parser_maps_items_to_message_and_tool_events_and_turn_completed_to_usage` |
| ...estimates cost when pricing is given | `parser_estimates_cost_when_pricing_is_given_with_cached_input_at_the_cached_rate` |
| ...is chunking-independent | `parser_is_chunking_independent` |
| ...tolerates unknown events, items and stdout noise | `parser_tolerates_unknown_events_items_and_stdout_noise_and_non_fatal_errors_do_not_fail_the_turn` |
| ...turn.failed marks the turn failed | `parser_turn_failed_marks_the_turn_failed_with_the_vendor_message` |
| CodexAdapter: start: `codex exec --json`, prompt on stdin via "-" | `start_runs_codex_exec_json_with_the_prompt_on_stdin_via_dash_and_returns_the_thread_id` |
| ...resume: `codex exec resume <thread> --json ...` | `resume_runs_codex_exec_resume_thread_json` |
| ...sandbox / prompt-as-argument / extra overrides | `sandbox_prompt_as_argument_and_extra_overrides_are_configurable` |
| ...turn.failed becomes an AgentError | `turn_failed_becomes_an_error_carrying_the_vendor_message` |
| ...non-zero exit without turn.completed carries stderr | `non_zero_exit_without_turn_completed_carries_stderr` |
| ...abort kills the process group | `abort_kills_the_process_group_and_fails_with_aborted` |
| ...preflight: key and binary | `preflight_checks_key_and_binary` |

## context.test.ts -> tests/context.rs

| TS test | Rust test |
|---|---|
| formatDuration | `format_duration_formats_hours_and_minutes_never_negative` |
| improvementPct | `improvement_pct_is_positive_when_the_metric_moved_in_the_objective_direction` (+ the `n/a` case) |
| buildContext: objective, baseline, best | `build_context_states_the_objective_baseline_and_current_best` |
| ...remaining time and budget | `build_context_shows_remaining_time_and_budget` |
| ...history table, escaping pipes | `build_context_lists_the_verdict_history_as_a_table_escaping_pipes` |
| ...rules: protected paths and guardrails | `build_context_lists_rules_protected_paths_and_guardrails` |
| ...dependencies are fixed, unless deps: none | `build_context_tells_the_agent_that_dependencies_are_fixed_unless_the_evaluator_declares_none` |
| ...no history yet | `build_context_says_so_when_there_is_no_history_yet` |
| prompts: initial prompt = brief + context | `initial_prompt_is_the_brief_plus_the_context` |
| prompts: resume message for verified / rejected / no-change | `resume_message_for_a_verified_rejected_and_no_change_turn` |

Added: `numbers_are_formatted_like_the_ts_helpers` (`Number.prototype.toFixed` ties round away from zero, not to even).
Protected paths: `effective_protected` in `domain.rs` is a port of `protected-set.ts` without the evaluator-template registry
(`TODO(core)`); a template's own protect globs are missing from the agent's context listing until core lands (the engine still enforces them).

## controls.test.ts -> tests/controls.rs

| TS test | Rust test |
|---|---|
| artifact layout: uses the per-agent prefix IAM allows | `artifact_layout_uses_the_per_agent_prefix_iam_allows` |
| SsmRunnerControl: start writes config.json and launches ratchet-runner | `ssm_start_writes_config_json_and_launches_ratchet_runner_detached_through_ssm` |
| ...start: shell-quotes values and surfaces a failed launch | `ssm_start_shell_quotes_values_and_surfaces_a_failed_launch` |
| ...send: numbered inbox objects | `ssm_send_writes_numbered_inbox_objects_continuing_after_existing_ones` |
| ...poll: events after the cursor, ignoring torn lines | `ssm_poll_returns_events_after_the_cursor_from_jsonl_batches_ignoring_torn_lines` |
| ...stop: durable stop message plus SIGTERM | `ssm_stop_sends_a_durable_stop_message_plus_sigterm_to_the_runner_pid_and_tolerates_a_gone_instance` |
| ...round trip with the runner-side classes | `ssm_round_trip_with_the_runner_side_classes_sink_to_poll_and_send_to_artifact_inbox` |
| ...a restarted event sink continues the numbering | `a_restarted_event_sink_continues_the_numbering_and_a_failed_put_is_retried_on_the_next_flush` |
| AwsCliArtifactStore: put/get/list | `aws_cli_store_put_get_list_through_s3_cp_and_s3api` |
| ...list returns [] when the prefix is empty | `aws_cli_store_list_returns_empty_when_the_prefix_is_empty_aws_prints_null` |
| FsArtifactStore | `fs_store_behaves_like_s3_for_put_get_list_by_prefix` |
| LocalRunnerControl: separate clones, in-memory inbox, cursors | `local_control_runs_agents_in_separate_clones_with_an_in_memory_inbox_and_cursor_based_polling` |
| ...stop ends a running agent promptly | `local_control_stop_ends_a_running_agent_promptly` |

## detect.test.ts -> tests/detect.rs (33 tests, one for one)

`parseVersion` x2, `resolveLoginPath` x6, `detectHarnesses: install and version` x5, Claude Code auth x10 (incl. macOS keychain x3),
Codex auth x7, `localAgentEnv` x3 map to the Rust tests of the same name (snake case, prefixes `claude_`, `codex_`, `macos_keychain_`,
`local_agent_env_`). All assertions are ported unchanged, including: key variables are removed from the status probes, only the existence of
credential files is checked, `security find-generic-password` runs without `-w` / `-g`, key values and the masked key never reach a result.

## local-agents.test.ts -> tests/local_agents.rs

| TS test | Rust test |
|---|---|
| worktree mode: each agent a git worktree of one local clone | `worktree_mode_gives_each_agent_a_worktree_of_one_local_clone_with_its_own_identity_branch_and_refs` |
| ...stop removes that agent's worktree; release removes the clone | `worktree_mode_stop_removes_that_agents_worktree_and_nothing_else_release_removes_the_clone` |
| ...a start that cannot clone fails cleanly | `worktree_mode_a_start_that_cannot_clone_fails_cleanly_is_not_registered_and_can_be_retried` |
| ...a second run reuses the clone and replaces a stale worktree | `worktree_mode_a_second_run_reuses_the_existing_clone_and_replaces_a_stale_worktree` |
| ...poll reports a lost agent as failed once | `poll_reports_an_unknown_lost_agent_as_failed_once_so_the_engine_ends_it` |
| real adapters: ClaudeCodeAdapter, detected binary, login PATH, no API key | `local_agents_run_claude_code_adapter_with_the_detected_binary_the_user_login_path_and_no_api_key` |
| ...passes the key only when api-key auth | `local_agents_pass_the_key_only_when_the_harness_reports_api_key_auth` |
| ...refuses a missing or signed-out harness with the hint | `local_agents_refuse_to_launch_a_harness_that_is_missing_or_signed_out_with_the_detection_hint` |
| detection against real processes: shims | `detection_finds_shims_through_the_login_shell_path_reads_version_and_auth` |
| ...a missing CLI is not installed, a hanging one times out | `detection_a_missing_cli_is_not_installed_and_a_hanging_one_times_out` |

## process.test.ts -> tests/process.rs (+ unit test in src/process.rs)

| TS test | Rust test |
|---|---|
| LineSplitter handles CRLF, blank lines and partial tails | `process::tests::line_splitter_handles_crlf_blank_lines_and_partial_tails` (unit) |
| runProcess streams stdout lines, feeds stdin, stderr tail | `streams_stdout_lines_feeds_stdin_and_captures_the_stderr_tail` |
| ...missing binary as a spawn error | `reports_a_missing_binary_as_a_spawn_error` |
| ...abort kills the whole process group incl. SIGTERM-ignoring grandchildren | `abort_kills_the_whole_process_group_including_grandchildren_that_ignore_sigterm` |

Added: `multibyte_output_split_across_reads_is_not_corrupted` (the Rust spawner decodes UTF-8 across read boundaries itself).

## real-fixtures.test.ts -> tests/real_fixtures.rs

`transcripts_captured_from_the_real_clis_parse_completely` (ported; like the TS suite it checks nothing until `real-claude-*.jsonl` /
`real-codex-*.jsonl` files are dropped into `tests/fixtures/`).

## runner-loop.test.ts -> tests/runner_loop.rs (+ unit tests in src/git.rs)

All 19 `runRunnerLoop` tests are ported one for one under their snake-case names (prompt/turn/commit/push/verdict/resume/idle,
first prompt, agent-made commit gets the trailer, dirty tree on top of an agent commit, rejected/blocked + `refs/ratchet/accepted`,
rebase onto the best preferring the best, unresolvable rebase resets to the best, pause/resume, user messages, stop mid-turn, stop while
waiting for a verdict, external abort, deadline, budget, budget from tokens, crashed-turn retries, agent-unavailable, push retried and audited,
push never succeeds). The three `candidate trailer` tests are `git::tests::trailer_is_added_once_after_a_blank_line`,
`trailer_joins_an_existing_trailer_block`, `trailer_detection` (unit tests in `src/git.rs`).
Added: `unknown_runner_messages_are_reported_not_fatal`.

## runner-main.test.ts -> tests/runner_main.rs

| TS test | Rust test |
|---|---|
| reads config from the artifact store, runs the loop, ..., writes done.json | `runner_main_reads_config_from_the_store_runs_the_loop_streams_events_answers_inbox_messages_writes_done_json` |
| fetches SSM secrets for the agent (never the git token) and authenticates git | `runner_main_fetches_ssm_secrets_for_the_agent_never_the_git_token_and_authenticates_git_with_the_token` (asserts `AGENT_ENV_PASSTHROUGH` incl. `HTTPS_PROXY`, `NO_PROXY`, `NODE_USE_ENV_PROXY`) |
| validates its inputs with actionable errors | `runner_main_validates_its_inputs_with_actionable_errors` |
| gitAuthEnv scopes an http extraheader to the repo host | `git_auth_env_scopes_an_http_extraheader_to_the_repo_host_via_env_not_argv_or_disk` |
| binary: exits 0 and writes done.json after a full run | `binary_exits_zero_and_writes_done_json_after_a_full_run` (ported (changed): runs `ratchet-runner` built by cargo via `CARGO_BIN_EXE_ratchet-runner`; the TS test was skipped without `pnpm build`) |
| binary: SIGTERM aborts the agent and exits cleanly | `binary_sigterm_aborts_the_agent_and_exits_cleanly_recording_why` |
| binary: exits 2 with a message when misconfigured | `binary_exits_two_with_a_message_when_misconfigured` |

## scripted.test.ts -> tests/scripted.rs (9 tests, one for one)

`start plays attempt 0...`, `applies patches with git apply...`, `skip conditions...`, `an attempt whose patches are all in the tree...`,
`write / edit / delete...`, `usage events...`, `sleeps through the injected Clock...` (ManualClock + AcceleratedClock abort), `a fail step...`,
`git steps run in the workdir` map to the Rust tests of the same name.

## demo-scripts.test.ts, demo-cli-scripts.test.ts, demo-forecast-scripts.test.ts

| TS test | Rust test |
|---|---|
| demo-search ships the six real fixes | `demo_scripts::demo_search_ships_the_six_real_fixes` |
| demo-search `%s applies ... (git apply --check)` (x4) | `demo_search_patches_01_03_05_06_apply_to_the_current_examples_tree_git_apply_check` (one test looping the four files) |
| demo-search 02 and 04 build on 01 | `demo_search_02_and_04_build_on_01_and_02_so_they_are_checked_cumulatively_in_a_scratch_tree` |
| demo-search cc and cx scripts ... bad attempts | `demo_search_cc_and_cx_scripts_use_the_same_fixes_in_different_orders_and_include_all_three_kinds_of_bad_attempt` |
| demo-search cc: every good attempt keeps `pnpm test` green | `demo_search_cc_every_good_attempt_applies_and_the_three_bad_attempts_are_blocked_or_rejected` (**ported (changed)**: the fake engine uses its structural guardrail, not the demo's real `pnpm test`, which needs node, pnpm and vitest; verdict sequence, reasons, final tree and untouched `bench/` are asserted as before) |
| demo-search cx: different order, same outcome | `demo_search_cx_different_order_same_outcome` (same change) |
| demo-search both agents in parallel through LocalRunnerControl | `demo_search_both_scripted_agents_in_parallel_through_local_runner_control_share_one_ratchet_best` (**ported (changed)**: the final `demoTestsPass(finalDir)` is dropped, same reason; every other assertion is kept) |
| demo-cli ships the seven real improvements | `demo_cli_ships_the_seven_real_improvements` |
| demo-cli applies cumulatively in the %s order and keeps the demo tests green (x2) | `demo_cli_patches_apply_cumulatively_in_the_cc_and_cx_order` (**ported (changed)**: both orders are applied with `git apply`; the `tsc` build and `vitest` run after each order are **dropped** (node toolchain), they stay covered by the TS suite until `packages/agents` is deleted) |
| demo-cli cc and cx play real fixes ... | `demo_cli_scripts_play_real_fixes_in_different_orders_and_include_every_kind_of_bad_attempt` |
| demo-cli the overfit table ... | `demo_cli_the_overfit_table_holds_exactly_the_outputs_of_the_visible_invocations_that_succeed` |
| demo-forecast ships the five real improvements | `demo_forecast_ships_the_five_real_improvements` |
| demo-forecast applies cumulatively ... keeps the demo tests green (x2) | `demo_forecast_patches_apply_cumulatively_in_the_cc_and_cx_order` (**ported (changed)**, same as demo-cli) |
| demo-forecast 04 needs 02 | `demo_forecast_04_needs_02` |
| demo-forecast cc and cx play the same fixes ... | `demo_forecast_scripts_play_the_same_fixes_in_different_orders_and_include_every_kind_of_bad_attempt` |

The demo tests read `examples/demo-*` from the repository root, so they keep working only while `examples/` exists (it is not part of the TS packages).

## Added in Rust (no TS counterpart)

- `tests/snapshots.rs` (insta): `context.md`, the initial prompt, a resume message, the SSM launch script, the `RunnerConfig` wire shape, the Claude and Codex
  transcripts as `AgentEvent` JSON, the harness hints.
- Unit tests in `src/`: `domain` (protected-set globs, wire shapes of `RunnerMessage` / `AgentEvent`), `clock`, `detect` (semver scan, version-directory
  order, secret-name matching), `inbox`, `scripted`, `process`.

## Behaviour differences (all deliberate, none observable in the TS tests)

1. **Reader robustness.** A store error in `ArtifactInbox::next` is logged and retried after the poll interval (TS: the rejected promise ended the
   mailbox pump, so the runner never saw another message). `ArtifactEventSink` reads the last batch number lazily on the first flush and, if
   listing fails, keeps its events and retries instead of numbering from 0.
2. **`FsArtifactStore.put`** writes a temp file and renames it, so a reader never sees a half-written object (`.tmp-*` files are hidden from `list`).
   The empty-object retry in the readers stays, it protects the S3 case.
3. **`LocalRunnerControl::start`** builds the adapter (which can fail with the detection hint) before it clones or adds a worktree; TS added the
   worktree first and left it behind when the adapter factory threw.
4. **Numbers.** Timestamps are `i64` milliseconds, token counts `u64`; JSON floats print like Rust (`20.0`), every consumer parses them the same. The
   `AcceleratedClock` floors `now()`. `Number.prototype.toFixed` tie rounding and `Math.round` are reproduced where text is formatted.
5. **Unknown runner messages** (a `type` this version does not know) decode to `RunnerMessage::Unknown` and the loop reports
   `unknown runner message` (TS: reported with the JSON). Messages without a string `type`, or that fail to decode, are skipped as before.
6. **Env numbers** (`RATCHET_CLOCK_SPEED`, `RATCHET_EVENT_FLUSH_MS`, `RATCHET_INBOX_POLL_MS`, `RATCHET_MAX_IDLE_TURNS`) that are not numbers make `ratchet-runner`
   exit 2 with a message (TS: `NaN` silently disabled the feature or threw a `RangeError` for the clock).
7. **`RATCHET_CLAUDE_PERMISSION_MODE`** is passed through for any value (`ClaudePermissionMode::Other`), like the TS cast.
8. **Events** drop `null`/absent `data` entries (an `exitCode` of `null` on a signal exit is omitted; TS wrote `null`).
9. **Spawn errors** read like node's (`spawn claude ENOENT`), built from the OS error.

## Local stand-ins to unify when ratchet-core / ratchet-sandbox land

`src/domain.rs`, every item marked `TODO(core)` or `TODO(sandbox)`: ids, `AgentKind`, `CandidateVerdict`, `Clock`, `AgentEvent`/`AgentUsage`/`AgentSpec`/
`AgentSession`/`AgentAdapter`, `RunnerMessage`/`RunnerConfig`/`RunnerHandle`/`RunnerControl`/`RunnerInbox`, a slice of `RatchetConfig` (objective, evaluator,
guardrails, protected, environment, infra; unknown fields kept), the protected-set computation, `ArtifactStore`, and the `CloudCompute::run_command` slice of the SSM capability.
