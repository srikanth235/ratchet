# Parity: `packages/aws` -> `crates/ratchet-sandbox`

The TypeScript suites under `packages/aws/src` are the executable spec. Each TS test is mapped to the Rust test that
replaces it. All Rust tests live in `tests/suite/` (one module per TS test file, one test binary: each integration-test
binary links the whole AWS SDK) plus a few unit tests next to the code (`src/util.rs`, `src/civil.rs`).

Counts: **208** vitest cases (160 `it(...)` plus 6 `it.each` blocks that expand to 48 cases) -> **199** Rust tests
(193 in `tests/suite`, 6 unit tests in `src/`). The `it.each` blocks become one Rust test that loops over the same cases
(a failure names the case), which is why the Rust count is lower even though every TS case is covered, and
dozens of extra tests are new (marked "new" below). No TS test is dropped.

`packages/aws/src/sandbox/environment.test.ts` (29 cases: materializing drafted and custom environments, the two-stage
`TofuSandboxProvider`, the `infra:` module, the mock, `TofuEnvironmentChecker`) is not in the counts above: its 27 Rust tests are
`tests/suite/sandbox_environment.rs`, mapped in `crates/ratchet-env/PARITY.md`.

How the AWS SDK is faked: Rust SDK clients are built with `aws-smithy-mocks` rules (`mock_client!`), so the request
conversion code runs for real and the tests assert on the SDK input types, like `aws-sdk-client-mock` did. A client given
no rules panics on any call, which is how "no AWS call happens" is asserted. Real AWS is never called.

Security-relevant behaviour (user data, secret-ref validation, id validation, profile-per-purpose, IMDSv2 options,
artifact atomicity, reaper safety, the `infra:` override) is covered by the ported tests; the user-data scripts are also
pinned byte-for-byte by insta snapshots, and those 11 outputs were diffed against the TypeScript output when ported.

Naming: the ports are `ratchet_core::ports` traits (`CloudCompute`, `ArtifactStore`, `CostReconciler`, `PriceBook`,
`SandboxProvider`); the implementations keep their TS names (`AwsCloudCompute`, `MockCloudCompute`, `S3ArtifactStore`,
`FsArtifactStore`, `AwsCostReconciler`, `MockCostReconciler`, `AwsPriceBook`, `StaticPriceBook`,
`TofuSandboxProvider`, `MockSandboxProvider`). `ImageLookup` (`Ec2ImageLookup`/`MockImageLookup`) is new (TS had free functions) and
the one capability core has no port for.

## Unified with ratchet-core

`domain.rs` (the local copies of the ports' data types) and the local port traits are gone; every implementation here implements the core
trait, returns `PortResult`, takes core's `OptimizationId` / `AgentId`, and uses core's `Clock`. Consequences:

- `SystemClock` / `ManualClock` are `ratchet_core::ports::mock`'s (same behaviour: `sleep` advances the manual one); the crate's copy and its
  unit test are removed. The other mocks differ from core's of the same name and stay: `StaticPriceBook` knows the common c7i/m7i prices,
  `MockCostReconciler` can compute its answer from the query, `FsArtifactStore` is a directory (core's `MemoryArtifactStore` is not), and
  `MockSandboxProvider` is the TS `aws` mock (resources, contract outputs, teardown guard; core's records calls).
- `SandboxOutputs` is core's (the TS `AwsSandboxOutputs` merged into it, see core's PARITY); `AwsSandboxOutputs` stays as an alias.
  `BenchPurpose` is core's `BenchLaunchPurpose`.
- `Error` stays the crate's internal error (AWS codes drive the idempotency and retry rules); `From<Error> for PortError` keeps the message
  verbatim and a refused teardown as `PortError::TeardownBlocked`. `AwsCloudCompute` keeps its typed id-as-`&str` methods privately
  (`*_impl`) behind the port. Tests that matched on `Error::Tofu` / `Error::ContractViolation` of a provider call now match the message
  (`PortError::Failed`); `Reaper::reap` and `CostMeter::{snapshot, collect}` take `Option<&OptimizationId>`.
- New tests: `error::tests` (2), `fs_store_behaves_like_s3_for_put_get_list_by_prefix_under_the_runner_layout` (moved from ratchet-agents with the store), `is_usable_as_the_core_cloud_compute_port_object`, `is_usable_as_the_core_sandbox_provider_port_object_and_its_outputs_are_the_core_type`.

## artifacts.test.ts -> tests/suite/artifacts.rs

| TS test | Rust test |
|---|---|
| S3: put encrypts and prefixes | `s3_put_encrypts_and_prefixes` |
| S3: get returns body or null for NoSuchKey | `s3_get_returns_body_or_none_for_no_such_key` (+ `s3_get_treats_the_typed_no_such_key_error_as_missing_too`, new) |
| S3: list pages and strips the prefix | `s3_list_pages_and_strips_the_prefix` |
| S3: rejects key (4 cases) | `s3_rejects_bad_keys_before_any_request` |
| Fs: round-trips, lists by prefix, nested dirs | `fs_round_trips_lists_by_prefix_nested_dirs` |
| Fs: writes through a staging file ... | `fs_writes_through_a_staging_file_that_never_shows_up_as_a_key` |
| Fs: refuses path traversal | `fs_refuses_path_traversal` |
| (new) | `fs_never_lists_the_staging_dir_and_a_failed_write_leaves_nothing_behind`, `fs_readers_see_the_old_or_the_new_object_never_a_partial_one` (concurrent reader/writer) |

"Reader retry semantics" (a reader that sees a listed-but-empty object must retry instead of skipping it) live in
`packages/agents/src/artifacts.ts` (`readEvents`, `ArtifactInbox`, tested by `packages/agents/test/artifacts-race.test.ts`),
not in `packages/aws`: they belong to the ratchet-agents port. This crate provides the half they depend on: atomic
`.put-tmp` + rename writes, a `.put-tmp` that is never listed, and rename-over-existing for overwrites.

## cost.test.ts -> tests/suite/cost.rs

| TS test | Rust test |
|---|---|
| computes live spend per optimization / agent / role | `computes_live_spend_per_optimization_agent_role_from_lifetimes` |
| stops accruing at termination and applies the 60 s minimum | `stops_accruing_at_termination_and_applies_the_60_s_minimum` |
| collect() emits only new spend ... | `collect_emits_only_new_spend_ledger_shaped_aggregated_per_opt_agent_category` |
| does not double count ... | `does_not_double_count_instances_that_terminate_between_collects` |
| bench spend has no agentId; unpriced ... | `bench_spend_has_no_agent_id_unpriced_types_are_flagged_not_silently_priced` |
| stopped instance without timestamp ... | `stopped_instance_without_timestamp_stops_accruing_at_first_observation` |
| BudgetGuard: warns at 80%, trips at 100% | `warns_at_80_percent_trips_at_100_percent_each_exactly_once` |
| BudgetGuard: jumping straight past 100% | `jumping_straight_past_100_percent_trips_without_a_warn` |
| BudgetGuard: rejects a non-positive budget | `rejects_a_non_positive_budget_custom_warn_fraction` |

## cost-explorer.test.ts -> tests/suite/cost_explorer.rs

| TS test | Rust test |
|---|---|
| categoryForTags | `category_for_tags_maps_role_agent_tags_to_ledger_categories` |
| sends a DAILY UnblendedCost query ... | `sends_a_daily_unblended_cost_query_filtered_by_ratchet_opt_and_grouped_by_role_and_agent_tags` |
| can additionally require the sandbox tag | `can_additionally_require_the_sandbox_tag` |
| sums days and pages into ledger categories | `sums_days_and_pages_into_ledger_categories` |
| reports incomplete data while days are flagged Estimated | `reports_incomplete_data_while_days_are_flagged_estimated_unless_accept_estimated` |
| explains empty results | `explains_empty_results_tags_not_activated_data_not_there_yet` |
| rejects ids / non-numeric amounts / SDK errors | `rejects_bad_ids_inverted_windows_non_numeric_amounts_and_surfaces_sdk_errors` (+ `an_unparsable_amount_is_an_error_not_a_zero`, new) |
| MockCostReconciler | `mock_returns_canned_or_computed_actuals_and_records_queries` |

## identity.test.ts -> tests/suite/identity.rs

| TS test | Rust test |
|---|---|
| returns the caller identity | `returns_the_caller_identity` |
| fails when the credentials are for another account | `fails_when_the_credentials_are_for_another_account` |
| surfaces credential errors | `surfaces_credential_errors_e_g_an_expired_sso_session` |
| accepts an injected client | `accepts_an_injected_client_and_refuses_an_empty_account` |

## images.test.ts -> tests/suite/images.rs

| TS test | Rust test |
|---|---|
| imageVersionMatches | `matches_the_version_itself_or_the_version_plus_a_build_suffix_never_a_longer_version` |
| findRatchetImages: asks EC2 ... picks the newest | `asks_ec2_for_this_accounts_available_tagged_images_of_the_architecture_and_picks_the_newest_matching_the_version` |
| findRatchetImages: never picks the host-toolchain variant | `never_picks_the_host_toolchain_bench_variant_or_an_image_of_another_kind_and_honors_the_architecture` |
| findRatchetImages: reports nothing for a version that was not built | `reports_nothing_for_a_version_that_was_not_built_and_lists_what_exists` |
| findRatchetImages: rejects a version that could smuggle ... | `rejects_a_version_that_could_smuggle_anything_into_a_filter` |
| resolveImageRefs: tag lookup | `uses_the_tag_lookup_when_no_override_is_given_so_the_user_never_types_an_ami_id` |
| resolveImageRefs: override wins | `an_explicit_override_wins_and_is_not_looked_up_a_lookup_is_skipped_when_everything_is_overridden` |
| resolveImageRefs: only what is not overridden / needed | `looks_up_only_what_is_not_overridden_and_only_what_is_needed` |
| resolveImageRefs: actionable error | `fails_with_an_actionable_error_when_a_needed_image_is_not_published_for_this_version` (+ `says_found_none_when_nothing_of_that_kind_exists`, new) |
| resolveImageRefs: rejects an override that is not an ami id | `rejects_an_override_that_is_not_an_ami_id` |
| region: reads the repo's region defensively | `reads_the_repos_region_defensively_and_falls_back_to_the_setting_then_to_us_east_1` |
| region: refuses a value that is not a region name | `refuses_a_value_that_is_not_a_region_name_instead_of_launching_somewhere_else` |
| (new) | `the_mock_lookup_applies_the_same_selection_rules_and_records_lookups` |

## pricing.test.ts -> tests/suite/pricing.rs

| TS test | Rust test |
|---|---|
| StaticPriceBook: knows common types and supports overrides | `static_knows_common_c7i_m7i_types_and_supports_overrides` |
| StaticPriceBook: prices scale linearly | `static_prices_scale_linearly_with_size_for_c7i` |
| AwsPriceBook: exact Linux/shared/on-demand filters | `queries_the_pricing_api_with_exact_linux_shared_on_demand_filters` |
| AwsPriceBook: caches within ttl, dedupes concurrent lookups | `caches_within_ttl_refetches_after_and_dedupes_concurrent_lookups` |
| AwsPriceBook: errors when nothing matches, or falls back | `errors_when_nothing_matches_or_falls_back_when_configured` |
| parseOnDemandUsd | `parse_on_demand_usd_ignores_non_hourly_and_zero_dimensions` |

## reaper.test.ts -> tests/suite/reaper.rs

| TS test | Rust test |
|---|---|
| terminates instances past max lifetime and leaves young ones | `terminates_instances_past_max_lifetime_and_leaves_young_ones` |
| exactly at the limit is kept | `exactly_at_the_limit_is_kept` |
| terminates instances of stopped agents | `terminates_instances_of_stopped_agents_only_that_agent` |
| terminates everything of an ended optimization | `terminates_everything_of_an_ended_optimization_including_bench_no_agent` |
| optimization-ended takes precedence | `optimization_ended_takes_precedence_over_agent_stopped_and_lifetime` |
| terminates merely-stopped instances | `terminates_merely_stopped_instances_terminate_on_shutdown_missed` |
| enforces max_instances_per_agent | `enforces_max_instances_per_agent_keeps_oldest_kills_newest_reports_offender` |
| quota ignores runners and bench | `quota_ignores_runners_and_bench_no_quota_configured_means_no_enforcement` |
| is idempotent | `is_idempotent_second_run_finds_nothing_and_does_not_call_terminate_again` |
| skips instances already shutting down; scopes to one optimization | `skips_instances_already_shutting_down_scopes_to_one_optimization_when_asked` |
| records terminate failures in the report | `records_terminate_failures_in_the_report_instead_of_throwing` |
| supports async predicates | `supports_async_predicates` (the `ReaperPolicy` trait is async by construction) |
| (new) | `only_ever_touches_instances_the_compute_lists_as_tagged_and_never_terminated_ones` |

The TS tests monkey-patch `cloud.terminate`; the Rust tests wrap the mock in a `Spy` decorator that counts and fails
`terminate`. Predicates are the `ReaperPolicy` trait instead of two callbacks.

## compute/mock.test.ts -> tests/suite/compute_mock.rs

| TS test | Rust test |
|---|---|
| validates ids like the real one | `validates_ids_like_the_real_one` |
| identity lifecycle | `identity_lifecycle_create_is_idempotent_revoke_delete` |
| secrets live under /ratchet/<opt>/<agent>/ | `secrets_live_under_the_agent_path_and_delete_secrets_is_scoped` |
| launch requires an existing identity; boot delay | `launch_requires_an_existing_identity_instances_boot_after_the_delay` |
| listTagged filters; terminate idempotent | `list_tagged_filters_by_optimization_terminate_is_idempotent_and_stamps_terminated_at` |
| runCommand delegates to the injected handler | `run_command_delegates_to_the_injected_handler_and_needs_a_running_instance` |
| default handler; simulate helpers | `default_handler_succeeds_silently_helpers_simulate_experiments_shutdown_and_stopped` |
| (new) | `mock_instance_ids_are_valid_instance_ids_so_the_real_validators_accept_them` |

## sandbox/mock.test.ts -> tests/suite/sandbox_mock.rs

| TS test | Rust test |
|---|---|
| plans every sandbox resource with real addresses | `plans_every_sandbox_resource_with_real_addresses` |
| apply produces contract outputs | `apply_produces_contract_outputs_outputs_and_re_plan_reflect_it` |
| destroy honours the teardown guard | `destroy_honours_the_teardown_guard_like_the_real_provider` |
| can start with sandboxes already applied | `can_start_with_sandboxes_that_are_already_applied` |
| validates the sandbox id | `validates_the_sandbox_id` |
| (new) | `records_the_calls_it_receives_in_order` |

## sandbox/override.test.ts -> tests/suite/sandbox_override.rs

| TS test | Rust test |
|---|---|
| readInfraOverride: off unless the repo asks | `is_off_unless_the_repo_asks_for_it` |
| readInfraOverride: normalized directory | `returns_a_normalized_repository_relative_directory` |
| readInfraOverride: refuses (6 cases) | `refuses_paths_outside_the_repository_and_non_strings` |
| checkSandboxContract: accepts | `accepts_outputs_that_satisfy_the_contract` |
| checkSandboxContract: the module that ships with the app | `accepts_the_module_that_ships_with_the_app_its_outputs_parse_and_validate` |
| checkSandboxContract: names every missing output | `names_every_missing_output` |
| checkSandboxContract: requires the deps identity by default | `requires_the_dependency_image_identity_by_default_because_without_it_the_app_cannot_build_snapshots_or_images` |
| checkSandboxContract: checks shapes | `checks_shapes_arns_ids_path_and_prefix_conventions_limits` |
| checkSandboxContract: separations | `checks_the_separations_the_isolation_invariants_rely_on` |
| checkSandboxContract: garbage | `turns_garbage_into_one_clear_error` |
| TofuSandboxProvider validateContract: fails apply | `fails_apply_with_the_list_of_problems_when_the_module_does_not_satisfy_the_contract` |
| TofuSandboxProvider validateContract: passes / off | `passes_a_conforming_module_and_does_nothing_when_validation_is_off_the_module_that_ships_with_the_app` |
| materialize: checks out the protected ref | `checks_out_the_protected_ref_and_returns_the_module_directory` |
| materialize: refuses missing dir / no .tf / symlinks | `refuses_a_missing_directory_a_directory_without_tf_files` + `refuses_a_symlinked_module_and_a_symlinked_parent` (unix only) |
| materialize: refuses a module that can run commands | `refuses_a_module_that_can_run_commands_on_this_machine` |
| materialize: refuses a ref it cannot resolve | `refuses_a_ref_it_cannot_resolve_a_candidate_sha_is_never_used` |
| (new) | `validation_also_runs_when_outputs_are_read_later`, `refuses_a_commit_that_is_not_a_plain_sha_it_names_a_directory` |

Hardening beyond TS: `materialize_infra_override` refuses a resolved commit that is not a 7-64 char hex sha (the commit
names a directory under `dest_root`). Unchanged otherwise.

## sandbox/tofu.test.ts -> tests/suite/sandbox_tofu.rs

| TS test | Rust test |
|---|---|
| plan: init -> plan -out -> show -json ... | `plan_runs_init_plan_out_show_json_with_a_per_sandbox_dir_and_vars_file` |
| plan: exit code 0 is fine; 1 is an error | `plan_exit_code_0_no_changes_is_fine_1_is_an_error_with_stderr` |
| plan: validates the sandbox id and instance types | `plan_validates_the_sandbox_id_and_instance_types` |
| plan: rejects sandbox id (11 cases) at every entry point | `rejects_bad_sandbox_ids_at_every_provider_entry_point` |
| plan: nothing secret-shaped in the vars file | `does_not_put_anything_secret_shaped_in_the_vars_file` |
| apply: plans, applies the saved plan, maps outputs | `apply_plans_applies_the_saved_plan_and_maps_outputs_per_the_contract` |
| apply: outputs without deps_identity_ref | `parses_outputs_of_a_sandbox_applied_before_dependency_snapshots_the_identity_is_simply_absent` |
| apply: stops after a failed apply | `apply_stops_after_a_failed_apply` |
| apply: serializes concurrent operations | `serializes_concurrent_operations_on_the_same_sandbox` |
| apply: does not block different sandboxes | `does_not_block_different_sandboxes_on_each_other` |
| destroy: refuses while roles exist | `destroy_refuses_while_runtime_agent_roles_exist_and_runs_nothing` |
| destroy: destroys once roles are gone | `destroy_runs_once_the_roles_are_gone` |
| destroy: surfaces tofu failures | `destroy_surfaces_tofu_failures` |
| outputs: null for an unknown sandbox | `outputs_is_none_for_an_unknown_sandbox_without_spawning_tofu` |
| outputs: reads outputs (and region) | `outputs_reads_outputs_and_region_for_an_applied_sandbox` |
| pure parsers: mapActions | `map_actions_matches_the_terraform_action_pairs` |
| pure parsers: summarizePlan | `summarize_plan_tolerates_an_empty_plan` |
| pure parsers: parseTofuOutputs | `parse_tofu_outputs_empty_is_none_missing_output_is_a_clear_error` |
| (new) | `a_failed_operation_does_not_wedge_the_queue_for_that_sandbox`, `the_outputs_round_trip_through_json_in_the_typescript_shape`, `tofu_invocations_for_apply_and_destroy_are_pinned` (insta snapshot of every tofu command line, env and the vars file), `the_real_runner_captures_stdout_stderr_exit_code_env_and_cwd`, `the_real_runner_reports_a_missing_binary_as_an_error_and_closes_stdin`, `tofu_error_messages_keep_the_tail_of_stderr` |

As in TS, the `tofu` subprocess is faked (`FakeRunner`); the TS tests never ran the real binary, so neither do these.
The new `TokioProcessRunner` tests spawn `sh`/`cat`, not tofu. The `/tmp/claude-0/tofu/tofu` binary is therefore unused.

## compute/aws.test.ts -> tests/suite/compute_aws.rs and tests/suite/user_data.rs

| TS test | Rust test |
|---|---|
| id validation: rejects %j at every id entry point (17 cases) | `rejects_injection_at_every_id_entry_point` |
| id validation: rejects instance id (8 cases) | `rejects_bad_instance_ids_in_terminate_and_run_command` |
| id validation: rejects bad secret names | `rejects_bad_secret_names_including_dotdot_slash_and_wildcards` |
| id validation: accepts well-formed ids | `accepts_well_formed_ids` |
| id validation: shortens very long role names | `shortens_very_long_role_names_but_keeps_them_unique_and_at_most_64_chars` (+ pinned hash in `util::tests`) |
| createAgentIdentity: creates role under path ... | `creates_role_under_path_with_boundary_tags_policy_and_profile` |
| createAgentIdentity: idempotent | `create_agent_identity_is_idempotent_when_role_and_profile_already_exist` |
| createAgentIdentity: propagates unexpected IAM errors | `create_agent_identity_propagates_unexpected_iam_errors` |
| createAgentIdentity: rejects a malformed role path | `rejects_a_malformed_role_path` |
| kill switch: revoke attaches an inline deny-all | `revoke_attaches_an_inline_deny_all` |
| kill switch: deletes in dependency order | `deletes_in_dependency_order_profile_role_removal_profile_policies_role` |
| kill switch: idempotent when gone | `delete_is_idempotent_when_everything_is_already_gone` |
| kill switch: does not swallow other errors | `delete_does_not_swallow_other_errors` |
| listAgentRoles pages | `list_agent_roles_pages_through_the_agent_path` |
| secrets: puts a SecureString and tags it | `puts_a_secure_string_under_the_agent_path_and_tags_it` |
| secrets: deletes all, paging and batching by 10 | `deletes_all_secrets_under_the_agent_path_paging_and_batching_by_10` |
| secrets: none present does not call Delete | `delete_secrets_with_none_present_does_not_call_delete` |
| launch: runner user data lists several secret refs | `user_data::runner_user_data_lists_several_secret_refs_in_the_comma_format_the_runner_parses` |
| launch: launchRunner tags, IMDSv2, ... | `launch_runner_tags_imdsv2_terminate_on_shutdown_profile_network_user_data` |
| launch: bench (confirm) | `launch_bench_instance_confirm_uses_the_confirm_profile_the_private_bench_subnet_and_bench_sg_no_agent_tag` |
| launch: bench (baseline, bench) never get the sealed profile | `launch_bench_instance_baseline_and_bench_never_get_the_sealed_capable_profile` |
| launch: picks the profile by purpose, fails closed | `picks_the_profile_by_purpose_and_fails_closed` ("anything else" is unrepresentable: `BenchPurpose` is a closed enum and the JSON form `"confirm-ish"` is rejected, asserted in the same test) |
| launch: refuses a confirm launch without a confirm profile | `refuses_a_confirm_launch_without_a_confirm_profile_instead_of_falling_back_to_the_bench_one` |
| launch: refuses to share one profile | `refuses_to_share_one_profile_between_bench_and_confirm` |
| launch: refuses without a bench subnet | `refuses_to_launch_bench_instances_without_a_bench_subnet_no_fallback_to_the_agent_subnet` |
| launch: agent runners in the agent subnet | `launches_agent_runners_in_the_agent_subnet` |
| launch: accepts a bare profile name for bench | `accepts_a_bare_profile_name_for_bench` |
| deps: runs in the AGENT subnet ... | `deps::runs_in_the_agent_subnet_with_the_agent_runner_group_and_its_own_profile_never_the_bench_or_confirm_ones` |
| deps: keeps IMDSv2, terminate-on-shutdown, tags | `deps::keeps_imdsv2_terminate_on_shutdown_and_the_tags_the_reaper_and_the_teardown_orphan_check_rely_on` |
| deps: fails closed without a deps profile | `deps::fails_closed_without_a_deps_profile_no_launch_no_fallback_to_the_bench_identity_or_subnet` |
| deps: needs no bench subnet | `deps::needs_no_bench_subnet_it_is_not_a_bench_and_a_bench_launch_still_needs_one` |
| deps: refuses to share a profile | `deps::refuses_to_share_a_profile_between_the_snapshot_builder_and_the_benches` |
| deps: picks the deps profile for deps only | `deps::picks_the_deps_profile_for_deps_and_for_nothing_else` |
| deps user data: unprivileged user + IMDS block | `user_data::deps::creates_the_unprivileged_fetch_user_and_blocks_instance_metadata_for_it_v4_and_v6_before_anything_else_runs` |
| deps user data: none of the bench machinery | `user_data::deps::has_none_of_the_bench_machinery_no_candidate_launcher_no_sudo_rule_no_bench_or_holdout_user` |
| deps user data: shorter lifetime guard | `user_data::deps::has_a_shorter_lifetime_guard_than_a_bench_a_snapshot_build_is_minutes` |
| deps user data: proxy vars | `user_data::deps::without_a_proxy_writes_no_proxy_variables_with_one_the_agent_subnet_proxy_variables_and_nothing_sensitive` |
| deps user data: blocks IMDS for containers | `user_data::deps::blocks_instance_metadata_for_the_containers_it_builds_and_fetches_in_forwarded_traffic_before_docker_starts` |
| deps user data: docker daemon proxy | `user_data::deps::gives_the_docker_daemon_the_allow_list_proxy_only_when_there_is_one` |
| deps user data: reaches the launched instance | `deps::reaches_the_launched_instance_proxy_from_the_sandbox_outputs` |
| bench user data: both users + IMDS block | `user_data::bench_split::creates_both_unprivileged_users_and_blocks_imds_for_both_before_user_data_reports_done` |
| bench user data: hidepid, launcher, sudo rule | `user_data::bench_split::hides_each_user_from_the_other_hidepid_and_installs_the_launcher_and_sudo_rule_before_user_data_reports_done` |
| bench user data: docker engine | `user_data::bench_split::starts_the_docker_engine_the_repo_container_runs_on_skipped_on_images_without_docker_and_never_adds_a_user_to_the_docker_group` |
| bench user data: valid bash | `user_data::bench_split::the_launcher_scripts_and_the_user_data_are_valid_bash` |
| bench user data: sudo rule | `user_data::bench_split::the_sudo_rule_gives_ratchet_holdout_exactly_one_thing_the_launcher_script_as_ratchet_bench_without_a_password` |
| bench user data: clean environment launcher | `user_data::bench_split::the_launcher_starts_candidate_code_with_a_clean_environment_and_refuses_to_forward_the_holdout_or_credentials` |
| bench user data: refuses to run as anyone but the bench user | `user_data::bench_split::the_launcher_script_refuses_to_run_as_anyone_but_the_bench_user` |
| bench user data: constants match | `user_data::bench_split::the_constants_the_bench_scripts_rely_on_match` |
| runner user-data exports the proxy only when configured | `user_data::runner_user_data_exports_the_proxy_only_when_one_is_configured` |
| launchRunner passes the configured proxy | `launch_runner_passes_the_configured_proxy_into_user_data` |
| derives the compute config from sandbox outputs | `derives_the_compute_config_from_sandbox_outputs_bench_profile_for_bench_confirm_profile_for_confirm` |
| retries while the fresh instance profile propagates | `retries_while_the_fresh_instance_profile_propagates_then_succeeds` |
| does not retry unrelated errors; gives up | `does_not_retry_unrelated_errors_and_gives_up_after_the_retry_budget` |
| refuses invalid ids before calling AWS | `refuses_invalid_ids_before_calling_aws` |
| terminate: no-ops on empty, tolerates gone ids | `terminates_no_ops_on_empty_tolerates_already_gone_ids` |
| listTagged: filters, pages, infers roles | `list_tagged_filters_by_tag_pages_infers_roles_and_skips_untagged` |
| listTagged: tag-key filter | `list_tagged_without_an_optimization_uses_a_tag_key_filter` |
| toInstanceInfo ignores instances without ratchet:opt | `to_instance_info_ignores_instances_without_ratchet_opt` |
| runCommand: sends AWS-RunShellScript, polls | `sends_run_shell_script_with_env_exports_and_polls_to_completion` |
| runCommand: non-zero exit codes and timeouts | `returns_non_zero_exit_codes_and_timeouts_without_throwing` |
| runCommand: bad env names, past the deadline | `rejects_bad_env_names_and_gives_up_past_the_deadline` (+ `a_command_that_never_registers_is_an_error_after_the_deadline`, new) |
| preflight checks the account | `preflight_checks_the_account` |
| (new) | `user_data::snapshots` (11 insta snapshots, diffed against the TS output), `user_data::secret_values_never_appear_only_ssm_refs`, `user_data::proxy_urls_must_be_plain_ip_and_port`, `user_data::every_generated_script_is_valid_bash`, `user_data::bench_split::a_heredoc_delimiter_cannot_appear_inside_the_scripts_they_wrap` |

## Intentional differences from the TypeScript

- **Closed enums.** `BenchPurpose`, `AgentKind`, `InstanceRole`, ... are enums, so "unknown bench purpose" cannot be
  constructed; the JSON form is rejected by serde (asserted). The `default:` arm of `benchProfileFor` is gone.
- **No abort signals.** `ProcessRunner::run` takes no `AbortSignal`; cancel by dropping the future (`TokioProcessRunner` kills the
  child on drop). `Clock::sleep` takes core's optional `CancellationToken`, which nothing in `packages/aws` passes.
- **`RemoteCommand.env` is sorted** by name (a `BTreeMap`); TS exported in insertion order. Exports are independent.
- **SSM `ResponseCode`.** The Rust SDK reports an absent `ResponseCode` as `0`, TS saw `undefined`. A non-`Success`
  status with code `0` is therefore reported as exit code `1`, as TS did for an absent code. A `Success` is `0`.
- **Plan/outputs JSON** are `serde_json` values with `preserve_order`, so key order follows insertion like JS objects.
- **`SandboxOutputs`** (core's) merges TS `SandboxOutputs` and `AwsSandboxOutputs` (the engine only stores the AWS shape);
  `AwsSandboxOutputs` is a type alias. Whole-number floats serialize as JSON integers (`2`, not `2.0`) like JS.
- **`TofuSandboxProvider` error text** truncates stderr/stdout by characters, not UTF-16 units.
- **Pricing single-flight** uses a shared future (`futures::Shared`), so concurrent failing lookups share one result,
  including the fallback result, like the shared promise in TS.
- **IAM.** `aws-sdk-iam` is used in addition to the crates in the task list: `AwsCloudCompute` creates per-agent roles.

## Not ported / out of scope

- `packages/agents/src/artifacts.ts` reader retry semantics (ratchet-agents).
- Wiring `IamRoleLister` / `AwsCloudCompute` into the engine (ratchet-engine); the pieces are exported.
