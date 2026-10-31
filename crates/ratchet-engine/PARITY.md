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
