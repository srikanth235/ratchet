# ratchet-engine: parity with the TypeScript engine and runtime

The TypeScript in `packages/engine` and `packages/runtime` is the executable spec. Every TS test case is ported to a
Rust test with the same assertions; this file maps one to the other. All tests run in one integration-test binary
(`tests/engine/main.rs`, `cargo test -p ratchet-engine --all-features --test engine`) against the in-memory fakes of
`ratchet_engine::testing` (feature `testing`) and a real (in-memory or temp-file) SQLite ledger.

## Engine (`packages/engine/src/*.test.ts`): 127 TS cases → 127 Rust tests

### Deliberate differences (the assertion is kept in spirit, the mechanics differ)

- **Typed inputs.** Cases that pass a value TypeScript's types would have rejected at compile time are asserted at the
  parse boundary instead: `agentsWhere: 'moon'` and `direction: 'sideways'` (`serde_json::from_str::<AgentsWhere /
  Direction>` fails), and `environment.region: 42` in `regionOf` (the typed `Environment.region` is `Option<String>`).
- **Fake provider outputs.** The Rust `FakeSandbox` returns `ratchet_core::ports::mock::sample_sandbox_outputs`, so bucket
  and subnet names differ from the TS fake (`ratchet-artifacts-<id>` instead of `ratchet-<id>-artifacts`, `subnet-0a1`
  instead of `subnet-1`); the tests assert the Rust names.
- **Raw SQL.** `h.ledger.sqlite.prepare(...)` becomes `sqlx` on `Ledger::pool()` (a dev-dependency) in `reconcile`, and
  `Ledger::kv_set` / `kv_delete` for the `engine_kv` pokes.
- **Fakes.** `onApply` is `FakeSandbox::on_apply` plus `FakeSandbox::observe` (records the ledger status at the start of
  apply / destroy); `h.compute.listTagged = ...` is `FakeCompute::fail_list_tagged`; `h.git.commitFiles = undefined` is
  `FakeGit::can_commit_files = false`.
- **Harness.** `until` yields real time (5 ms) between cycles: a resumed kill switch runs on real SQLite I/O, where the
  TS version completes in microtasks. `FakeBench::close_gate` uses `watch::send_replace` (a plain `send` is a no-op
  without subscribers).
- **`agents_where`, `calibration`, `teardown_receipt`** are `async` in Rust (they read the ledger / kv instead of a
  synchronous SQLite handle).

### `packages/engine/src/engine.test.ts` → `tests/engine/core_loop.rs` (61 TS, 61 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| createOptimization / reads ratchet.yaml via the git host, snapshots it, and creates agents with branches | `reads_ratchet_yaml_via_the_git_host_snapshots_it_and_creates_agents_with_branches` |
| createOptimization / gives two agents of the same kind distinct branches | `gives_two_agents_of_the_same_kind_distinct_branches` |
| createOptimization / validates ids, input and the config | `validates_ids_input_and_the_config` |
| createOptimization / rejects malformed ids on every public entry point | `rejects_malformed_ids_on_every_public_entry_point` |
| start / runs the baseline, records the noise floor, then launches each agent in order | `start_runs_the_baseline_records_the_noise_floor_then_launches_each_agent_in_order` |
| start / never stores secret values in the ledger | `never_stores_secret_values_in_the_ledger` |
| start / fails the optimization and cleans up when a secret is unavailable | `fails_the_optimization_and_cleans_up_when_a_secret_is_unavailable` |
| start / fails when the baseline breaks its own command guardrail | `fails_when_the_baseline_breaks_its_own_command_guardrail` |
| start / can only be started once | `can_only_be_started_once` |
| start / a kill switch during the baseline wins and start() returns quietly | `a_kill_switch_during_the_baseline_wins_and_start_returns_quietly` |
| happy path / baseline -> two agents -> verified, rejected, blocked and failed-confirm candidates -> monotonic ratchet -> PR | `baseline_two_agents_verified_rejected_blocked_and_failed_confirm_candidates_monotonic_ratchet_pr` |
| happy path / uses a custom report builder for the PR body | `uses_a_custom_report_builder_for_the_pr_body` |
| happy path / refuses to open a PR without a verified candidate | `refuses_to_open_a_pr_without_a_verified_candidate` |
| happy path / keeps the hidden-trace baseline separate from the bench scale | `keeps_the_hidden_trace_baseline_separate_from_the_bench_scale` |
| happy path / retries bench infrastructure failures, then rejects after the attempt limit | `retries_bench_infrastructure_failures_then_rejects_after_the_attempt_limit` |
| happy path / a guardrail that fails before any bench run (no samples at all) blocks the candidate instead of wedging the queue | `a_guardrail_that_fails_before_any_bench_run_blocks_the_candidate_instead_of_wedging_the_queue` |
| happy path / an outcome with neither samples nor a failed guardrail counts as a failed attempt and is rejected after the attempt limit | `an_outcome_with_neither_samples_nor_a_failed_guardrail_counts_as_a_failed_attempt` |
| happy path / retries a candidate whose changed files cannot be read yet, without losing order | `retries_a_candidate_whose_changed_files_cannot_be_read_yet_without_losing_order` |
| happy path / ignores duplicate candidates the git host re-lists | `ignores_duplicate_candidates_the_git_host_relists` |
| cost and stop rules / meters instances x price by category with estimated=true, from a per-instance watermark | `meters_instances_x_price_by_category_with_estimated_true_from_a_per_instance_watermark` |
| cost and stop rules / counts bench instances as bench_runner spend | `counts_bench_instances_as_bench_runner_spend` |
| cost and stop rules / records token usage from runner events as spend category tokens | `records_token_usage_from_runner_events_as_spend_category_tokens` |
| cost and stop rules / warns at 80% and trips the kill switch at 100% in the right order, within one metering interval | `warns_at_80_percent_and_trips_the_kill_switch_at_100_percent_in_the_right_order` |
| cost and stop rules / a token-spend overrun also trips the kill switch | `a_token_spend_overrun_also_trips_the_kill_switch` |
| cost and stop rules / stops gracefully at the deadline: runners stop, queued candidates are still evaluated, then everything is torn down | `stops_gracefully_at_the_deadline_runners_stop_queued_candidates_are_still_evaluated_then_teardown` |
| cost and stop rules / picks up a candidate pushed right before a graceful stop (final poll) | `picks_up_a_candidate_pushed_right_before_a_graceful_stop_final_poll` |
| graceful stop drains every candidate / all agents done in the same tick as their last push: the final git poll submits it and the drain verifies it | `graceful_stop_drains_every_candidate::all_agents_done_in_the_same_tick_as_their_last_push` |
| graceful stop drains every candidate / a runner that pushes its last commit while it is being stopped is still collected and evaluated | `graceful_stop_drains_every_candidate::a_runner_that_pushes_its_last_commit_while_it_is_being_stopped_is_still_collected` |
| graceful stop drains every candidate / reports done only AFTER the push becomes visible: the drain keeps polling git until runners are done and the branch is quiet | `graceful_stop_drains_every_candidate::reports_done_only_after_the_push_becomes_visible_the_drain_keeps_polling_git` |
| graceful stop drains every candidate / survives a transient git failure during the final poll: it retries while draining | `graceful_stop_drains_every_candidate::survives_a_transient_git_failure_during_the_final_poll` |
| graceful stop drains every candidate / an agent that finishes while the others keep running still gets its last push submitted in that tick | `graceful_stop_drains_every_candidate::an_agent_that_finishes_while_the_others_keep_running_still_gets_its_last_push_submitted` |
| graceful stop drains every candidate / evaluates a backlog queued from several agents (bench + confirm) before terminating | `graceful_stop_drains_every_candidate::evaluates_a_backlog_queued_from_several_agents_before_terminating` |
| graceful stop drains every candidate / the drain is bounded by drainTimeoutMs | `graceful_stop_drains_every_candidate::the_drain_is_bounded_by_drain_timeout_ms` |
| identical shas from different agents / keys candidates per (agent, sha): both get rows and verdicts; the second is rejected after the first moves the ratchet | `identical_shas_from_different_agents::keys_candidates_per_agent_and_sha` |
| identical shas from different agents / a later commit on one branch is found even when another agent already submitted the same sha | `identical_shas_from_different_agents::a_later_commit_on_one_branch_is_found_even_when_another_agent_already_submitted_the_same_sha` |
| cost and stop rules / stops after plateauEvals consecutive evaluations without a verified win; a win resets the count | `stops_after_plateau_evals_consecutive_evaluations_without_a_verified_win_a_win_resets_the_count` |
| cost and stop rules / counts blocked candidates toward the plateau | `counts_blocked_candidates_toward_the_plateau` |
| cost and stop rules / stops when every agent reports done | `stops_when_every_agent_reports_done` |
| runner finish events / treats the runner loop's closing status (reason, no state) as done / failed, and stops once every agent is finished | `treats_the_runner_loops_closing_status_as_done_or_failed_and_stops_once_every_agent_is_finished` |
| runner finish events / a runner that finished because the engine stopped it is not treated as done early | `a_runner_that_finished_because_the_engine_stopped_it_is_not_treated_as_done_early` |
| kill switch / stop() revokes first, then stops runners, then terminates; and aborts the in-flight bench | `stop_revokes_first_then_stops_runners_then_terminates_and_aborts_the_in_flight_bench` |
| kill switch / retries a failed revocation on later ticks and keeps terminating late instances (reaper) | `retries_a_failed_revocation_on_later_ticks_and_keeps_terminating_late_instances` |
| kill switch / stop() is idempotent and a stopped optimization stays stopped | `stop_is_idempotent_and_a_stopped_optimization_stays_stopped` |
| reaper / terminates experiments past their max lifetime, and instances of a stopped or finished agent | `terminates_experiments_past_their_max_lifetime_and_instances_of_a_stopped_or_finished_agent` |
| reaper / terminates agent runners that outlive the time limit plus grace | `terminates_agent_runners_that_outlive_the_time_limit_plus_grace` |
| agent control / routes pause / resume / message to the right runner and tracks status | `routes_pause_resume_message_to_the_right_runner_and_tracks_status` |
| agent control / survives a runner that cannot be reached | `survives_a_runner_that_cannot_be_reached` |
| restart and resume / re-polls runners from their stored cursors: no lost and no replayed events | `re_polls_runners_from_their_stored_cursors_no_lost_and_no_replayed_events` |
| restart and resume / finishes a bench that was in flight when the process died (candidate re-queued) | `finishes_a_bench_that_was_in_flight_when_the_process_died` |
| restart and resume / continues with the confirm run when the crash came between bench and confirm | `continues_with_the_confirm_run_when_the_crash_came_between_bench_and_confirm` |
| restart and resume / settles a candidate whose results were all written but whose verdict was not | `settles_a_candidate_whose_results_were_all_written_but_whose_verdict_was_not` |
| restart and resume / resumes from a SQLite file after the process dies mid-run | `resumes_from_a_sqlite_file_after_the_process_dies_mid_run` |
| restart and resume / finishes launching half-launched agents after a hard crash, without repeating completed steps | `finishes_launching_half_launched_agents_after_a_hard_crash_without_repeating_completed_steps` |
| restart and resume / completes a kill switch that was interrupted | `completes_a_kill_switch_that_was_interrupted` |
| teardown / deletes secrets and identities, terminates leftovers, verifies zero instances and completes | `teardown_deletes_secrets_and_identities_terminates_leftovers_verifies_zero_instances_and_completes` |
| teardown / refuses to report success while an instance survives termination | `teardown_refuses_to_report_success_while_an_instance_survives_termination` |
| teardown / reports a failed secret deletion instead of completing | `teardown_reports_a_failed_secret_deletion_instead_of_completing` |
| teardown / can tear down a draft that never started | `can_tear_down_a_draft_that_never_started` |
| run loop and events / run() ticks on the injected clock until aborted | `run_ticks_on_the_injected_clock_until_cancelled` |
| run loop and events / subscribe returns an unsubscribe, and a throwing listener does not break the loop | `a_dropped_subscriber_does_not_disturb_the_engine_and_late_subscribers_see_later_events` |
| run loop and events / exposes its mode | `exposes_its_mode` |

### `packages/engine/src/local-agents.test.ts` → `tests/engine/local_agents.rs` (9 TS, 9 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| agents on this machine / defaults to local when the engine has a local runner control, and starts agents without any cloud runner resources | `defaults_to_local_when_the_engine_has_a_local_runner_control_and_starts_agents_without_any_cloud_runner_resources` |
| agents on this machine / routes events, messages, pause / resume and stop to the local control | `routes_events_messages_pause_resume_and_stop_to_the_local_control` |
| agents on this machine / teardown releases the local workspaces and leaves cloud-side secrets alone | `teardown_releases_the_local_workspaces_and_leaves_cloud_side_secrets_alone` |
| agents on this machine / a lost local agent (the app restarted) is reported failed and the run can end | `a_lost_local_agent_the_app_restarted_is_reported_failed_and_the_run_can_end` |
| agents on this machine / an optimization can ask for the cloud, which keeps today's path (identity, secrets, runner instance) | `an_optimization_can_ask_for_the_cloud_which_keeps_todays_path_identity_secrets_runner_instance` |
| agents on this machine / the engine setting picks the default, and an engine without a local control stays in the cloud | `the_engine_setting_picks_the_default_and_an_engine_without_a_local_control_stays_in_the_cloud` |
| agents on this machine / local agents are refused where there is no local control | `local_agents_are_refused_where_there_is_no_local_control` |
| agents on this machine / survives an engine restart: the choice is remembered per optimization | `survives_an_engine_restart_the_choice_is_remembered_per_optimization` |
| agents on this machine / works with the automatic sandbox: the evaluator is cloud even when the agents are not | `works_with_the_automatic_sandbox_the_evaluator_is_cloud_even_when_the_agents_are_not` |

### `packages/engine/src/reconcile.test.ts` → `tests/engine/reconcile.rs` (4 TS, 4 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| reconcileCosts / is unavailable without a cost reconciler, and teardown then records no reconcile | `is_unavailable_without_a_cost_reconciler_and_teardown_then_records_no_reconcile` |
| reconcileCosts / teardown reconciles best effort: pending inside the billing lag, nothing written, no billing query | `teardown_reconciles_best_effort_pending_inside_the_billing_lag_nothing_written_no_billing_query` |
| reconcileCosts / writes correction rows once billing is final, and a second run changes nothing | `writes_correction_rows_once_billing_is_final_and_a_second_run_changes_nothing` |
| reconcileCosts / a failing reconciler never fails teardown | `a_failing_reconciler_never_fails_teardown` |

### `packages/engine/src/sandbox-auto.test.ts` → `tests/engine/sandbox_auto.rs` (24 TS, 24 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| regionOf / reads environment.region defensively and falls back to the setting default | `region_of_reads_environment_region_defensively_and_falls_back_to_the_setting_default` |
| automatic sandbox: creation / is created at the first run start, not before, from ratchet.yaml and the default region | `is_created_at_the_first_run_start_not_before_from_ratchet_yaml_and_the_default_region` |
| automatic sandbox: creation / uses the configured default region and id scheme; other regions get their own sandbox | `uses_the_configured_default_region_and_id_scheme_other_regions_get_their_own_sandbox` |
| automatic sandbox: creation / simulation ids differ from live ones | `simulation_ids_differ_from_live_ones` |
| automatic sandbox: creation / is reused while runs are active: concurrent first starts apply once | `is_reused_while_runs_are_active_concurrent_first_starts_apply_once` |
| automatic sandbox: creation / a failed creation leaves the optimization a draft with a clear error, and a retry works | `a_failed_creation_leaves_the_optimization_a_draft_with_a_clear_error_and_a_retry_works` |
| automatic sandbox: creation / an auto sandbox is off unless asked for: the explicit rules still apply | `an_auto_sandbox_is_off_unless_asked_for_the_explicit_rules_still_apply` |
| automatic sandbox: idle window and teardown / is not destroyed while a run is active, however long it takes | `is_not_destroyed_while_a_run_is_active_however_long_it_takes` |
| automatic sandbox: idle window and teardown / is destroyed after the last run ends plus the idle window (default 30 min), with the ended runs torn down first | `is_destroyed_after_the_last_run_ends_plus_the_idle_window_default_30_min_with_the_ended_runs_torn_down_first` |
| automatic sandbox: idle window and teardown / records a receipt: what was removed, what it cost, nothing left running | `records_a_receipt_what_was_removed_what_it_cost_nothing_left_running` |
| automatic sandbox: idle window and teardown / honours a configured idle window | `honours_a_configured_idle_window` |
| automatic sandbox: idle window and teardown / a run that starts inside the idle window reuses the sandbox and stops the clock | `a_run_that_starts_inside_the_idle_window_reuses_the_sandbox_and_stops_the_clock` |
| automatic sandbox: idle window and teardown / a run after the teardown creates a fresh sandbox | `a_run_after_the_teardown_creates_a_fresh_sandbox` |
| automatic sandbox: idle window and teardown / keeps the idle clock across an app restart (a sandbox that outlived the app goes on the first tick) | `keeps_the_idle_clock_across_an_app_restart_a_sandbox_that_outlived_the_app_goes_on_the_first_tick` |
| automatic sandbox: idle window and teardown / a blocked destroy (an instance that will not terminate) is reported in the receipt path, not hidden, and retried later | `a_blocked_destroy_an_instance_that_will_not_terminate_is_reported_in_the_receipt_path_not_hidden_and_retried_later` |
| automatic sandbox: idle window and teardown / a provider failure during the destroy leaves the sandbox failed (infrastructure of unknown state) and is retried | `a_provider_failure_during_the_destroy_leaves_the_sandbox_failed_infrastructure_of_unknown_state_and_is_retried` |
| automatic sandbox: idle window and teardown / an explicitly applied sandbox is never destroyed automatically | `an_explicitly_applied_sandbox_is_never_destroyed_automatically` |
| automatic sandbox: idle window and teardown / an explicit apply or destroy takes an auto sandbox out of automatic management | `an_explicit_apply_or_destroy_takes_an_auto_sandbox_out_of_automatic_management` |
| automatic sandbox: idle window and teardown / is off by default: nothing is destroyed behind an explicit user | `is_off_by_default_nothing_is_destroyed_behind_an_explicit_user` |
| endSandbox (manual) / refuses while a run is active, then tears down on demand with a receipt | `refuses_while_a_run_is_active_then_tears_down_on_demand_with_a_receipt` |
| run-active (sleep prevention) / flips true when a run starts (even while its environment is still being created) and false when the last one ends | `flips_true_when_a_run_starts_even_while_its_environment_is_still_being_created_and_false_when_the_last_one_ends` |
| run-active (sleep prevention) / works without the automatic sandbox, and a failed start does not leave it stuck | `works_without_the_automatic_sandbox_and_a_failed_start_does_not_leave_it_stuck` |
| run-active (sleep prevention) / is true again after a restart with a run in flight | `is_true_again_after_a_restart_with_a_run_in_flight` |
| TICK constant sanity / one engine tick is a minute in the harness (the windows above are multiples of it) | `one_engine_tick_is_a_minute_in_the_harness_the_windows_above_are_multiples_of_it` |

### `packages/engine/src/sandbox.test.ts` → `tests/engine/sandbox_lifecycle.rs` (13 TS, 13 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| sandbox lifecycle / plans, applies and records planned -> applying -> applied with the outputs in the ledger | `plans_applies_and_records_planned_applying_applied_with_the_outputs_in_the_ledger` |
| sandbox lifecycle / records a failed apply and recovers on retry | `records_a_failed_apply_and_recovers_on_retry` |
| sandbox lifecycle / marks an apply that a crash interrupted as failed when the engine restarts | `marks_an_apply_that_a_crash_interrupted_as_failed_when_the_engine_restarts` |
| sandbox lifecycle / destroys with the input it was applied with: applied -> destroying -> destroyed, outputs cleared | `destroys_with_the_input_it_was_applied_with_applied_destroying_destroyed_outputs_cleared` |
| sandbox lifecycle / a destroy that fails inside the provider leaves the sandbox failed, and can be retried | `a_destroy_that_fails_inside_the_provider_leaves_the_sandbox_failed_and_can_be_retried` |
| sandbox lifecycle / keeps the ledger state across an engine restart | `keeps_the_ledger_state_across_an_engine_restart` |
| optimizations and the sandbox / live mode refuses to create or start an optimization without an applied sandbox | `live_mode_refuses_to_create_or_start_an_optimization_without_an_applied_sandbox` |
| optimizations and the sandbox / live mode links the sandbox to the optimization it runs in | `live_mode_links_the_sandbox_to_the_optimization_it_runs_in` |
| optimizations and the sandbox / simulation does not need a sandbox, and links one only once it is applied | `simulation_does_not_need_a_sandbox_and_links_one_only_once_it_is_applied` |
| optimizations and the sandbox / refuses to destroy while an optimization in the sandbox is running, then allows it after teardown | `refuses_to_destroy_while_an_optimization_in_the_sandbox_is_running_then_allows_it_after_teardown` |
| optimizations and the sandbox / refuses to destroy while a tagged instance is still alive | `refuses_to_destroy_while_a_tagged_instance_is_still_alive` |
| optimizations and the sandbox / fails closed when the orphan check itself cannot run | `fails_closed_when_the_orphan_check_itself_cannot_run` |
| optimizations and the sandbox / treats a compute that does not exist yet (no sandbox outputs) as having no orphans | `treats_a_compute_that_does_not_exist_yet_no_sandbox_outputs_as_having_no_orphans` |

### `packages/engine/src/simplified.test.ts` → `tests/engine/simplified.rs` (16 TS, 16 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| auto-calibrated statistics / derives repeats and the win threshold from the baseline and says so in plain words | `auto_calibrated_statistics::derives_repeats_and_the_win_threshold_from_the_baseline_and_says_so_in_plain_words` |
| auto-calibrated statistics / the calibrated threshold decides verdicts (noise floor 4 and 4% of 100 = 4: a gain of exactly 4 is not a win) | `auto_calibrated_statistics::the_calibrated_threshold_decides_verdicts_noise_floor_4_and_4_percent_of_100_is_4_a_gain_of_exactly_4_is_not_a_win` |
| auto-calibrated statistics / a noisy baseline raises the threshold instead of crowning noise | `auto_calibrated_statistics::a_noisy_baseline_raises_the_threshold_instead_of_crowning_noise` |
| auto-calibrated statistics / explicit repeats and min_gain in ratchet.yaml are respected as they were | `auto_calibrated_statistics::explicit_repeats_and_min_gain_in_ratchet_yaml_are_respected_as_they_were` |
| auto-calibrated statistics / noise_factor in ratchet.yaml sets the k of the noise floor | `auto_calibrated_statistics::noise_factor_in_ratchet_yaml_sets_the_k_of_the_noise_floor` |
| auto-calibrated statistics / survives a restart, and is re-derived from the ledger when the remembered copy is missing | `auto_calibrated_statistics::survives_a_restart_and_is_re_derived_from_the_ledger_when_the_remembered_copy_is_missing` |
| auto-calibrated statistics / is null before the baseline has run, and unknown ids are refused | `auto_calibrated_statistics::is_null_before_the_baseline_has_run_and_unknown_ids_are_refused` |
| automatic protected paths / blocks lockfile and install-config changes without any protected: list, before anything is benched | `automatic_protected_paths::blocks_lockfile_and_install_config_changes_without_any_protected_list_before_anything_is_benched` |
| automatic protected paths / a template's evaluator files are protected, and the user's list still adds to them | `automatic_protected_paths::a_templates_evaluator_files_are_protected_and_the_users_list_still_adds_to_them` |
| automatic protected paths / protectedPaths(id) lists the effective set with the reason for each entry | `automatic_protected_paths::protected_paths_id_lists_the_effective_set_with_the_reason_for_each_entry` |
| listTemplates and proposeTemplate / lists the gallery: the seven templates, then the demos | `templates::lists_the_gallery_the_seven_templates_then_the_demos` |
| listTemplates and proposeTemplate / commits the template on a new branch and opens a PR that says what was added and how to run a baseline | `templates::commits_the_template_on_a_new_branch_and_opens_a_pr_that_says_what_was_added_and_how_to_run_a_baseline` |
| listTemplates and proposeTemplate / passes the objective options through, and accepts a legacy pack name | `templates::passes_the_objective_options_through_and_accepts_a_legacy_pack_name` |
| listTemplates and proposeTemplate / refuses when files of the template already exist, unless overwrite is given | `templates::refuses_when_files_of_the_template_already_exist_unless_overwrite_is_given` |
| listTemplates and proposeTemplate / takes another branch name when the first is taken, and gives up with the original error when that fails too | `templates::takes_another_branch_name_when_the_first_is_taken_and_gives_up_with_the_original_error_when_that_fails_too` |
| listTemplates and proposeTemplate / rejects demos, unknown templates, bad options and a host that cannot commit files | `templates::rejects_demos_unknown_templates_bad_options_and_a_host_that_cannot_commit_files` |

## Runtime (`packages/runtime`)

Source: `crates/ratchet-engine/src/runtime/` (`simulation.rs`, `live.rs`, `live_checks.rs`, `summary.rs`, `simulate.rs`, `clock.rs`).
27 TS tests → 27 Rust tests (+ 4 demo sequence tests that have no TS counterpart). The e2e tests run only with `RATCHET_E2E=1`, like
the TS (`describe.skipIf(!enabled)`).

### `packages/runtime/src/bench-contract.test.ts` → `tests/engine/runtime_bench_contract.rs` (4 TS, 4 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| bench instance contract between @ratchet/aws and @ratchet/bench / agree on the candidate user, the holdout user and the launcher path | `agree_on_the_candidate_user_the_holdout_user_and_the_launcher_path` |
| … / the user-data of a confirm instance sets up every user and the launcher the scripts check for | `the_user_data_of_a_confirm_instance_sets_up_every_user_and_the_launcher_the_scripts_check_for` |
| … / agree on the dependency-snapshot user, and the user-data of a deps instance sets it up and blocks metadata for it | `agree_on_the_dependency_snapshot_user_and_the_user_data_of_a_deps_instance_sets_it_up_and_blocks_metadata_for_it` |
| … / agree on the prefixes the deps identity is scoped to in infra/aws (reads bench-inputs/*/deps-base.tgz, writes bench-deps/*) | `agree_on_the_prefixes_the_deps_identity_is_scoped_to_in_infra_aws` |

### `packages/runtime/src/live-checks.test.ts` → `tests/engine/runtime_live_checks.rs` (5 TS, 5 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| createConnectionTester / aws: reports the account, passes region and profile through, and reports credential errors | `aws_reports_the_account_passes_region_and_profile_through_and_reports_credential_errors` |
| … / github: requires push rights and never leaks the token | `github_requires_push_rights_and_never_leaks_the_token` |
| … / anthropic / openai: probe the models endpoint with the key in a header, and map statuses | `anthropic_openai_probe_the_models_endpoint_with_the_key_in_a_header_and_map_statuses` |
| … / network errors are reported without the key | `network_errors_are_reported_without_the_key` |
| … / tofu: finds the module and an OpenTofu binary | `tofu_finds_the_module_and_an_opentofu_binary` |

### `packages/runtime/src/live.test.ts` → `tests/engine/runtime_live.rs` (12 TS, 12 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| createLiveRuntime / before the first apply: sandbox plan/apply/destroy work, everything else says what is missing | `before_the_first_apply_sandbox_plan_apply_destroy_work_everything_else_says_what_is_missing` |
| … / rebuilt from the ledger outputs and a full configuration, nothing is missing | `rebuilt_from_the_ledger_outputs_and_a_full_configuration_nothing_is_missing` |
| … / reports only what is still missing | `reports_only_what_is_still_missing` |
| createHybridRuntime / defaults to local agents and an invisible sandbox: no apply step, no runner AMI needed | `defaults_to_local_agents_and_an_invisible_sandbox_no_apply_step_no_runner_ami_needed` |
| … / is ready once GitHub and the bench image are set | `is_ready_once_github_and_the_bench_image_are_set` |
| … / without an applied sandbox a draft can be created: the sandbox is made at the first start, so only GitHub is missing | `without_an_applied_sandbox_a_draft_can_be_created_the_sandbox_is_made_at_the_first_start_so_only_github_is_missing` |
| … / runs the detection probes when no harnesses are given, and says what is missing when none is usable | `runs_the_detection_probes_when_no_harnesses_are_given_and_says_what_is_missing_when_none_is_usable` |
| … / a signed-out harness does not count as usable; a signed-in one does | `a_signed_out_harness_does_not_count_as_usable_a_signed_in_one_does` |
| … / agentsWhere cloud keeps the SSM path: the runner AMI is required again | `agents_where_cloud_keeps_the_ssm_path_the_runner_ami_is_required_again` |
| … / createLiveRuntime without localAgents is unchanged (cloud, no local control) and refuses agentsWhere local without them | `create_live_runtime_without_local_agents_is_unchanged_cloud_no_local_control_and_refuses_agents_where_local_without_them` |
| images found by tag, region from the repo / resolveLiveImages fills both AMIs from the tags for the app version; an explicit ref wins | `resolve_live_images_fills_both_amis_from_the_tags_for_the_app_version_an_explicit_ref_wins` |
| … / createLiveRuntimeFor uses environment.region of the repo, then the setting; the lookup is skipped before a sandbox exists | `create_live_runtime_for_uses_environment_region_of_the_repo_then_the_setting_the_lookup_is_skipped_before_a_sandbox_exists` |

### `packages/runtime/src/summary.test.ts` → `tests/engine/runtime_summary.rs` (4 TS, 4 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| buildSimulationSummary / captures project, baseline, best, gain, verdict counts, spend, value/ROI and the PR path | `captures_project_baseline_best_gain_verdict_counts_spend_value_roi_and_the_pr_path` |
| … / handles a run with no win, no value model and no PR | `handles_a_run_with_no_win_no_value_model_and_no_pr` |
| … / prToPath converts file URLs and passes other URLs through | `pr_to_path_converts_file_urls_and_passes_other_urls_through` |
| writeSimulationSummary / writes pretty JSON, creating parent directories, and leaves no temp file behind | `write_simulation_summary_writes_pretty_json_creating_parent_directories_and_leaves_no_temp_file_behind` |

### `packages/runtime/src/local-agents.int.test.ts` → `tests/engine/runtime_local_agents.rs` (1 TS, 1 Rust)

| TS `describe` / `it` | Rust test |
|---|---|
| local agents through the real engine and an automatic sandbox / runs a Claude Code agent in a worktree, verifies its commit, then removes the worktree and (after the idle window) the sandbox | `runs_a_claude_code_agent_in_a_worktree_verifies_its_commit_then_removes_the_worktree_and_after_the_idle_window_the_sandbox` |

### `packages/runtime/src/simulation.e2e.test.ts` → `tests/engine/runtime_simulation.rs` (1 TS, 1 Rust + 4 demo tests)

| TS `describe` / `it` | Rust test |
|---|---|
| simulation e2e (RATCHET_E2E=1) / runs two agents x 2 attempts to a verified win, drains every candidate, opens a PR and leaves nothing running | `runs_two_agents_x_2_attempts_to_a_verified_win_drains_every_candidate_opens_a_pr_and_leaves_nothing_running` |
| (none: the verdicts are documented in `crates/ratchet-agents/scripts/demo-*/README.md`) | `demo_search_claude_code_verdicts`, `demo_search_codex_verdicts`, `demo_cli_claude_code_verdicts`, `demo_forecast_claude_code_verdicts` |

The demo tests run ONE agent through its whole script on the real `LocalBenchRunner` and assert the verdict of every candidate in
submission order: the bad attempts exactly as the READMEs document them (`blocked` for the protected-path edit, the contract
break and the look-ahead leakage; `rejected` for the noise-level no-op; `failed_confirm` for the overfit), the good ones
`verified` (a few `verified or rejected`). The demo-search gains are within the machine's run-to-run spread (the README says about
±10%; a first run on a shared 4-vCPU container ended its no-op as `verified` and a lone memoize as `failed_confirm`), so there only
the `blocked` attempts are asserted exactly and every other candidate must be decided (verified / rejected / failed_confirm);
demo-cli and demo-forecast came out as documented on the first run (`verified, blocked, verified, rejected, verified,
failed_confirm, verified, verified` and `verified, blocked, verified, rejected, verified, blocked, verified, failed_confirm,
verified`). Two agents interleave non-deterministically in the TS as well, hence one agent.

### Differences on purpose

- Construction is `async` (the AWS SDK loads its configuration asynchronously); `createLiveRuntime`'s JavaScript `Proxy`
  (`unready` / `lateBound`) is a hand-written proxy per port trait (`CloudCompute`, `ArtifactStore`, `RunnerControl`,
  `BenchRunner`, `GitHost`) that fails with `PortError::Failed("not_configured: …")`; `EngineError::from(PortError)` turns that into
  the `not_configured` code with the TS message.
- `LiveOptions::settings` / `SimulationOptions::settings` overlay field by field (`overlay_settings`); in the TS the spread replaces
  the whole `sandbox` object.
- `ConnectionTester` takes injectable `HttpProbe` / process runner / `check_aws` / `check_github` / `exists` instead of `fetch` etc.
- `simulate()` is a library function (`runtime::simulate`); the command line is the CLI's / `ratchetd`'s job. `SimulationOptions`
  has `tsc` and `bin_dir` overrides (the TS hard-codes `<repo>/node_modules/...`).
- `AwsPriceBook::connect_with` was added to `ratchet-sandbox` (clock + fallback), because the SDK clients are not public there.

## Desktop main-process logic (`packages/desktop/src/main`)

The Electron main process is split three ways: the engine-side logic (secrets, live configuration, harness detection, the
keep-awake hook, and the router that sends commands to the runtime of an optimization, `engine.ts`) lives here in
`ratchet-engine::desktop`; everything that needs the API view models (settings store, AWS profile reader, `service.ts`) is
`ratchetd::real` (a dependency cycle otherwise: `ratchetd` depends on this crate behind its `engine` feature). Rust paths below are
`crates/ratchet-engine/…` unless they start with `ratchetd:`.

| TS file | TS case | Rust test |
|---|---|---|
| `product-engine.test.ts` | slugOf (5 cases) | `tests/engine/desktop_product.rs::slug_of_cases` |
| | builds one hybrid runtime per repo from Settings, the cached harnesses and the GitHub token | `desktop_product::real_runs::builds_one_hybrid_runtime_per_repo_from_settings_the_cached_harnesses_and_the_github_token` |
| | routes commands to the runtime of the optimization, by the repo in the ledger | `real_runs::routes_commands_to_the_runtime_of_the_optimization_by_the_repo_in_the_ledger` |
| | refuses without a GitHub sign-in, and says what is missing when the runtime is not ready | `real_runs::refuses_without_a_github_sign_in_and_says_what_is_missing_when_the_runtime_is_not_ready` |
| | a repo that is not on GitHub is rejected by name | `real_runs::a_repo_that_is_not_on_github_is_rejected_by_name` |
| | skips the image lookup without AWS credentials, and keeps an explicit AMI override | `real_runs::skips_the_image_lookup_without_aws_credentials_and_keeps_an_explicit_ami_override` |
| | a failed image lookup becomes a "what is missing" line, not a crash | `real_runs::a_failed_image_lookup_becomes_a_what_is_missing_line_not_a_crash` |
| | a missing cloud runner image only blocks Cloud (unattended) runs | `real_runs::a_missing_cloud_runner_image_only_blocks_cloud_runs_not_this_machine_runs` |
| | proposeTemplate goes to the runtime of that repo, with the owner/name | `real_runs::propose_template_goes_to_the_runtime_of_that_repo_with_the_owner_name` |
| | a changed setting rebuilds an idle runtime on next use, but never one that is running | `real_runs::a_changed_setting_rebuilds_an_idle_runtime_on_next_use_but_never_one_that_is_running` |
| | watches an environment that outlived the last session even with no repo open | `real_runs::watches_an_environment_that_outlived_the_last_session_even_with_no_repo_open` |
| | starts a demo in a simulation runtime with an automatic environment, created once per project | `demos::starts_a_demo_in_a_simulation_runtime_with_an_automatic_environment_created_once_per_project` |
| | routes a demo optimization to its simulation runtime and diffs with its git | `demos::routes_a_demo_optimization_to_its_simulation_runtime_and_diffs_with_its_git` |
| | works without any sign-in or AWS: a demo needs nothing | `demos::works_without_any_sign_in_or_aws_a_demo_needs_nothing` |
| | merges run-active from every runtime … forwards the rest | `events::merges_run_active_from_every_runtime_and_forwards_the_rest` |
| `machine.test.ts` | AWS profiles: lists profile and SSO session names with their kind, never a key | `ratchetd: real::aws_profiles::tests::lists_profile_and_sso_session_names_with_their_kind_never_a_key` |
| | AWS profiles: handles missing files and empty ones | `ratchetd: real::aws_profiles::tests::handles_missing_files_and_empty_ones` |
| | AWS profiles: reads AWS_CONFIG_FILE / AWS_SHARED_CREDENTIALS_FILE, else ~/.aws | `ratchetd: real::aws_profiles::tests::reads_the_env_overrides_else_the_aws_directory` |
| | HarnessCache: probes once, caches, and probes again on refresh | `src/desktop/harnesses.rs::tests::probes_once_caches_and_probes_again_on_refresh` |
| | HarnessCache: a failing probe is an empty result, not a crash | `src/desktop/harnesses.rs::tests::a_failing_probe_is_an_empty_result` |
| | powerSaveBlocker hook: starts on run-active and stops when the last run ends | `src/desktop/power.rs::tests::starts_on_run_active_and_stops_when_the_last_run_ends` |
| | githubSlug (it.each) · inspectLocalFolder (2) | ported with `ratchet-github` (`slug.rs`, `local_repo.rs`; see its PARITY.md) |
| `settings.test.ts` | stores secrets encrypted, round-trips them in main, and persists across instances | `ratchetd: real::settings::tests::secrets_go_to_the_secret_store_never_the_settings_file_and_persist` |
| | refuses to store secrets when the keychain is unavailable | `ratchetd: real::settings::tests::refuses_to_store_secrets_when_the_keychain_is_unavailable` |
| | treats environment-provided credentials as configured | `ratchetd: real::settings::tests::environment_credentials_count_as_configured` |
| | survives a corrupt settings file | `ratchetd: real::settings::tests::survives_a_corrupt_settings_file` |
| | getSettings exposes only configured yes/no, and no response contains the secret value | `ratchetd: tests/api/real_service.rs::secrets_never_leave::no_response_event_or_error_carries_a_stored_key_or_token` |
| `service.test.ts` | GitHub row (5) | `ratchetd: tests/api/real_service.rs::github_row::*` (5) |
| | repo picker and ratchet.yaml (5) | `…::repo_picker::*` (4; "without a folder dialog, picking returns nothing" has no Rust counterpart: the folder dialog is the app's) |
| | templates and the template pull request (3) | `…::templates::*` (3) |
| | creating an optimization (2) | `…::creating::*` (2, plus `invalid_requests_name_the_fields`) |
| | preflight: what is missing (4) | `…::preflight::*` (4) |
| | AWS and agents rows (4) | `…::aws_and_agents::*` (4) |
| | environments (read-only) and the teardown receipt (5) | `…::environments::*` (5; "an engine that cannot say" becomes "an empty ledger yields an empty view / ending an unknown environment is a 404": the Rust engine always supports the sandbox methods) |
| | secrets never reach the renderer (2) | `…::secrets_never_leave::*` (2; the IPC channel list becomes "the OpenAPI document has no route that reveals a secret or applies infrastructure") |
| (new) | provisional groups answer 501 | `…::provisional_groups::overview_connectors_and_environment_drafts_are_not_supported_yet` |

Not ported on purpose: `ipc.test.ts` (no IPC: the contract tests of `ratchetd` cover every route), `github.test.ts` (the REST client and
the device flow are `ratchet-github`), `dev-engine.test.ts` / `dev-git.test.ts` (the dev engine is `ratchetd`'s simulation, tested there),
`mappers.test.ts` (the mappers are `ratchetd::views`, tested there).

### Differences on purpose

- **Secrets**: Electron's `safeStorage` (ciphertext in `settings.json`) became a `SecretStore` (`desktop::secrets`): the macOS Keychain
  (`KeychainSecretStore`, `security-framework`, built on macOS only — it is not compiled on the Linux CI, so it is unverified there), a
  `FileSecretStore` (0600, for tests and for machines that opt in with `--secrets-file`) and a `MemorySecretStore`. The settings file never
  holds a secret; without a keychain `set_secret` is refused, as before.
- **`PowerSaveHook`** takes a `PowerSaveBlocker` the host supplies (the Mac app's IOKit assertion); `ratchetd serve` wires the hook to
  `run-active` with a no-op blocker, so the application has a hook point and nothing else changes.
- **`ProductEngine`** reaches runtimes through `RuntimeEngine` (an `EngineApi` that can also `run`) and the `LiveFactory` / `SimFactory` /
  `ImageResolver` seams instead of TS's `createRuntime` / `createSimulation` / `resolveImages` options; the recording fakes are in
  `desktop::testing` (feature `testing`).
- A missing cloud runner image is `LiveHandle::runner_problem` instead of a `WeakMap` keyed by the runtime object.
- `RealService` creates the demo environment through the same router (`create_demo`), so `POST /v1/optimizations` with `demo` runs the
  scripted agents on the simulation runtime exactly like `ratchetd serve --mode sim` does per optimization.

### Known flakiness (demo-search)

The demo-search tests (`RATCHET_E2E=1`) assert only: 7 to 9 candidates (the scripted patches build on each other, so a later patch that no
longer applies lands no commit), every candidate decided as verified / rejected / failed_confirm / blocked, at least one blocked and at
least one verified win. A strict per-candidate sequence is asserted for demo-cli and demo-forecast, whose gains are far above the noise.
The same machine noise is why the TS `simulation.e2e.test.ts` retries up to `MAX_RUNS` times.
