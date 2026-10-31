# ratchet-github parity with the TypeScript tests

Every TS test below is mapped to the Rust test that replaces it, or marked `dropped: reason`. Rust tests that have no TS
counterpart are listed at the end. Run everything with `cargo test -p ratchet-github`.

Two systematic differences:

- **Octokit mocks became a fake GitHub server.** The TS tests injected a fake Octokit / `fetch`. The Rust tests start an
  in-process fake GitHub (`testing::FakeGithubServer`, axum, a port of `startFakeGithub` from
  `packages/e2e/harness/fakes.mjs` plus branches and pull requests) and point the code at it with the same base-URL
  override (`RATCHET_GITHUB_API_URL` / `_WEB_URL`) the app uses. So the REST paths, headers and error mapping are exercised
  for real instead of being asserted against call records.
- **Independent fixtures.** `local.test.ts` shared one repo across ordered tests (`beforeAll`). Rust tests run in parallel,
  so each builds its own bare remote from `examples/demo-search` (cheap, 196 KB); a test that relied on an earlier one
  (main moved on) now does that step itself.

## `packages/github/src/local.test.ts` -> `tests/local.rs`

| TS test | Rust test |
|---|---|
| lists only commits with a real Ratchet-Candidate: true trailer, oldest first | `lists_only_commits_with_a_real_candidate_trailer_oldest_first` |
| respects sinceSha | `respects_since_sha` |
| createBranch branches from a ref and refuses to overwrite | `create_branch_branches_from_a_ref_and_refuses_to_overwrite` |
| changedFiles lists paths changed relative to base | `changed_files_lists_paths_changed_relative_to_base` |
| checkout materializes a sha without .git or repo side effects | `checkout_materializes_a_sha_without_git_dir_or_repo_side_effects` |
| openPullRequest squashes to the final verified tree, writes the body to a file | `open_pull_request_squashes_to_the_final_verified_tree_and_writes_the_body_to_a_file` |
| squash is parented at the merge-base when main has moved on | `squash_is_parented_at_the_merge_base_when_main_has_moved_on` |
| diff: per-file counts and hunks of a candidate against its merge base with main | `diff_per_file_counts_and_hunks_of_a_candidate_against_its_merge_base_with_main` |
| diff: binary files have no counts or patch; an unchanged tree has no files | `diff_binary_files_have_no_counts_or_patch_and_an_unchanged_tree_has_no_files` |
| remoteUrl is a file:// url | `remote_url_is_a_file_url` |

## `packages/github/src/commit-files.test.ts` -> `tests/local.rs`, `tests/github_host.rs`

| TS test | Rust test |
|---|---|
| LocalGitHost.commitFiles: creates a branch with one commit on the base that adds the files (and replaces existing ones) | `commit_files_creates_a_branch_with_one_commit_on_the_base_that_adds_and_replaces_files` (local.rs) |
| LocalGitHost.commitFiles: refuses an existing branch, unsafe or duplicate paths and an empty list, and leaves the remote alone | `commit_files_refuses_an_existing_branch_unsafe_or_duplicate_paths_and_an_empty_list_and_leaves_the_remote_alone` (local.rs) |
| LocalGitHost.commitFiles: the branch is a normal PR head: openPullRequest with no shas opens it as is | `commit_files_branch_is_a_normal_pr_head_and_open_pull_request_with_no_shas_opens_it_as_is` (local.rs) |
| LocalGitHost.commitFiles: can base the commit on a sha and handles multi-line content verbatim | `commit_files_can_base_the_commit_on_a_sha_and_keeps_multi_line_content_verbatim` (local.rs) |
| GitHubGitHost.commitFiles: pushes the branch and opens the PR from it | `commit_files_pushes_the_branch_and_opens_the_pr_from_it_no_squash_head_already_exists` (github_host.rs) |
| GitHubGitHost.commitFiles: refuses a branch that already exists on the remote | `commit_files_refuses_a_branch_that_already_exists_on_the_remote` (github_host.rs) |

## `packages/github/src/github.test.ts` -> `tests/github_host.rs`

| TS test | Rust test |
|---|---|
| reads candidates, creates branches and opens a squashed PR via Octokit | `reads_candidates_creates_branches_and_opens_a_squashed_pr_via_rest` (PR create asserted on the fake server's recorded pull request) |
| never puts the token in argv; it travels only in the git config env of network calls | `never_puts_the_token_in_argv_it_travels_only_in_the_git_config_env_of_network_calls` |
| diff: syncs the mirror, diffs against the merge base and never puts the token in argv (mocked git) | `diff_syncs_the_mirror_diffs_against_the_merge_base_and_never_puts_the_token_in_argv_mocked_git` |
| scrubs the token from git errors | `scrubs_the_token_from_git_errors` |
| createGitRunner redacts secrets from failure messages | `the_git_runner_redacts_secrets_from_failure_messages` |
| refreshes the existing PR when GitHub answers 422 | `refreshes_the_existing_pr_when_github_answers_422` (real 422 from the fake server on a duplicate head) |
| rethrows non-422 Octokit errors | `rethrows_non_422_rest_errors` |
| validates repo and token | `validates_repo_and_token` |

## `packages/github/src/access.test.ts` -> `tests/access.rs`

| TS test | Rust test |
|---|---|
| reports the repo, default branch and whether the token can push | `reports_the_repo_default_branch_and_whether_the_token_can_push` |
| gives actionable errors and never echoes the token | `gives_actionable_errors_and_never_echoes_the_token` |
| validates its inputs before any request | `validates_its_inputs_before_any_request` |

## `packages/github/src/squash.test.ts` -> `src/squash.rs` (unit tests)

| TS test | Rust test |
|---|---|
| builds message, trailers and sha list in ratchet order | `squash::tests::builds_message_trailers_and_sha_list_in_ratchet_order` (the inline snapshot became a literal) |
| rejects an empty win list | `squash::tests::rejects_an_empty_win_list` |

## `packages/github/src/report.test.ts` -> `tests/report.rs`

The TS tests seeded a SQLite ledger. `ratchet-ledger` does not exist yet, so `build_result_report` takes a plain
`ReportInput` (what the engine will read from the ledger) and the tests build the same data by hand. The expected Markdown
and JSON are the TS snapshots, extracted byte for byte into `tests/fixtures/report.md` / `report.json`.

| TS test | Rust test |
|---|---|
| buildResultReport: summarizes the run as data | `summarizes_the_run_as_data` |
| buildResultReport: renders markdown | `renders_markdown` (equals the TS snapshot) |
| buildResultReport: renders json that round-trips | `renders_json_that_round_trips` (equals the TS snapshot) |
| buildResultReport: handles an optimization with no wins | `handles_an_optimization_with_no_wins` |
| buildResultReport: throws for an unknown optimization | dropped: the ledger lookup is the caller's job now; the equivalent guard here is `a_move_for_an_unknown_candidate_is_an_error` |
| buildSquashPlanFromLedger: uses verified wins in ratchet order | `squash_plan_uses_verified_wins_in_ratchet_order` (via `build_squash_plan_from_report_input`) |
| direction-aware change: renders a throughput gain as +% | `renders_a_throughput_gain_as_plus_percent` |

## `packages/desktop/src/main/github.test.ts` -> `tests/api.rs`

| TS test | Rust test |
|---|---|
| starts the device flow with the client id and the repo scope, and parses the code | `starts_the_device_flow_with_the_client_id_and_the_repo_scope_and_parses_the_code` |
| knows the placeholder client id is not a real one | `knows_the_placeholder_client_id_is_not_a_real_one` |
| maps the device poll answer (6 `it.each` cases) | `maps_the_device_poll_answers` (loops the same 6 answers) |
| reads the account, lists repos most recently pushed first across pages, and filters | `reads_the_account_lists_repos_most_recently_pushed_first_across_pages_and_filters` |
| reads a file at a ref (base64) and returns null for a missing one | `reads_a_file_at_a_ref_base64_and_returns_none_for_a_missing_one` |
| turns HTTP failures into plain messages that never carry the token | `turns_http_failures_into_plain_messages_that_never_carry_the_token` |
| GithubSignIn: shows the code, opens the browser, polls until approved, then keeps the token in the keychain-backed store | `sign_in_shows_the_code_opens_the_browser_polls_until_approved_then_keeps_the_token` (the `SettingsStore` is the `AccountStore` trait; asserted with a test store) |
| GithubSignIn: ends in denied / expired / error with a message and stores nothing (3 `it.each` cases) | `sign_in_ends_with_a_message_and_stores_nothing` |
| GithubSignIn: expires on its own when the code outlives its time | `sign_in_expires_on_its_own_when_the_code_outlives_its_time` |
| GithubSignIn: cancel stops the loop; a second start replaces the first flow | `sign_in_cancel_stops_the_loop_and_a_second_start_replaces_the_first_flow` |

## `packages/desktop/src/main/machine.test.ts` and `product-engine.test.ts` -> unit tests

| TS test | Rust test |
|---|---|
| githubSlug (7 cases) | `slug::tests::github_slug_cases` |
| inspectLocalFolder: reads the GitHub remote, the branch and ratchet.yaml of a real repository | `local_repo::tests::reads_the_github_remote_the_branch_and_ratchet_yaml_of_a_real_repository` |
| inspectLocalFolder: explains a folder that cannot be used | `local_repo::tests::explains_a_folder_that_cannot_be_used` |
| slugOf (5 cases) | `slug::tests::slug_of_cases` |
| friendlyMissing (the two assertions in "refuses without a GitHub sign-in, and says what is missing…") | `slug::tests::friendly_missing_rewrites_known_lines_and_keeps_others` (the rest of that TS test is engine behaviour, not ported here) |

## `packages/e2e` (Electron + Playwright) -> not ported here

The e2e specs drive the Electron app and are replaced by XCUITest flows in the Mac app. Only their GitHub fake
(`harness/fakes.mjs: startFakeGithub`) is ported, as `testing::FakeGithubServer`; `FakeGithubServer::env()` yields the same
`RATCHET_GITHUB_API_URL` / `RATCHET_GITHUB_WEB_URL` / `RATCHET_GITHUB_CLIENT_ID` variables and `sign_in_end_to_end_against_the_fake_server`
walks the device flow against it (start, pending polls, approve, token stored).
`makeLocalRepo()` (a real repo with a GitHub origin) is covered by the `inspect_local_folder` tests, which build the same.

## Rust-only tests (no TS counterpart)

- `domain::tests`: the `Repo` view-model serializes like the TS `RepoVM`.
- `numfmt::tests`: JS number formatting (`412` not `412.0`, `toPrecision(5)`).
- `git::tests`: `parse_git_diff` unit test, sha detection.
- `mock::tests` (5): the in-memory `MockGitHost` (TS `FakeGit` has no test file; these pin the behaviour the engine tests rely on).
- `api.rs`: `lists_branches_and_reads_ratchet_yaml_from_the_default_branch` (the `branches` call and `inspect_github_repo` are
  additions), `the_mock_api_behaves_like_the_ts_fake`, `sign_in_end_to_end_against_the_fake_server`.
- `local_repo::tests::a_file_is_not_a_folder`, `report.rs`: `renders_a_report_without_a_baseline`.

## Gaps and known differences

- **No ledger.** `buildResultReport(ledger, id)` / `buildSquashPlanFromLedger` became pure functions of `ReportInput`; the
  `ratchet-engine` crate fills it from `ratchet-ledger`. `bench.run` is passed in (`OptimizationInfo::bench_run`) because
  config parsing lives in `ratchet-core`.
- **`friendlyMissing` is ported but is engine text**; it may move to `ratchet-engine`.
- **Number formatting** is `ratchet_core::numfmt` (exact V8 `toFixed` / `toPrecision` / `String(number)`, ties and exponent
  form included), so the old half-to-even tie difference is gone. Percentile stat names sort with plain byte order (TS used
  `localeCompare`) among stats not in the fixed order list.
- **octocrab was not used**, see the crate docs: everything is `reqwest` + `serde`.

## Unified with ratchet-core

`domain.rs` no longer holds stand-ins: `AgentKind`, `CandidateVerdict`, `Direction`, `SpendCategory`, the ids, `BenchStats`, `gain_percent`,
`CandidateCommit`, `FileDiff`, `CommitFile`, `CommitFilesInput`, `PullRequestInput` and the `GitHost` port (`port.rs` is gone) are
`ratchet_core`'s. Where the two disagreed:

- `GitHost::open_pull_request` returns `PullRequestRef` (was `PullRequestUrl`), `commit_files` returns `CommitRef { sha }` (TS: `{ sha }`), and
  `PullRequestInput` gained `commit_message` in core. `CommitFile::executable` is `Option<bool>` like TS; `FileDiff` counts are `u64`.
- Trait methods return `PortResult`; this crate's `Error` converts with its message verbatim and an HTTP failure keeps its status
  (`PortError::Http { status, .. }`, was `Error::status()`), which `rethrows_a_422_when_no_open_pr_matches` / `rethrows_non_422_rest_errors` check.
  The optional-operation `Unsupported` error is core's default `commit_files`.
- `BenchStats` keeps the evaluator's key order (core's `extra` is an `IndexMap` now), so the report JSON is still the TS snapshot byte for byte; the report's
  `stats` objects print whole numbers without a fraction through `ser_stats`.
- `GithubSignIn` takes a core `Clock` (`SignInDeps::clock`) instead of `sleep` / `now` closures; `MockGitHost` stamps commits with a core `Clock`.
- The report input rows (`OptimizationInfo`, `CandidateInfo`, `RatchetMove`, ...) are the projection the engine assembles from several ledger rows, not
  ledger rows: they stay here with core's ids and enums (`OptimizationStatus` instead of a string).
- `Repo` (TS `RepoVM`) and `GithubFlow` (`GithubFlowVM`) are desktop view-models. `ratchet-core` has no view-models (they belong with the OpenAPI contract of the API
  crate), so they stay in this crate for now; moving them is deferred to the ratchetd / API port.
