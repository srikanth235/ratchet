# ratchet-connectors: parity with `packages/desktop/src/main/connectors*.ts`

The saved GitHub accounts, AWS profiles and agent credentials of the desktop app, and the choice remembered per repo, as a
crate. TypeScript sources: `connectors.ts` (the store), `connector-records.ts` (what is persisted), `shared/connectors.ts` (the
view models and inputs), the secret half of `settings.ts` (`setConnectorSecret`, `legacy`, `clearLegacy`), and
`live-settings.ts` (`validateLiveSetting` for the AWS profile and region, `validateSecretFormat`). The TypeScript stays the
executable spec until the cut-over.

| Rust | TypeScript |
|---|---|
| `model` (always compiled; `ToSchema` behind the `utoipa` feature) | `shared/connectors.ts`: `ConnectorVM` (a union tagged by `kind`), the inputs, `ConnectorTestVM`, `RepoConnections`, `RepoConnectionsVM`, `repoKey` |
| `ConnectorStore` | `ConnectorStore` |
| `ConnectorRecord`, `ConnectorsPersisted` (`connectors.json`, mode 0600) | `connector-records.ts` (`connectors` in `settings.json`) |
| `secrets` (`SecretStore`: Keychain / 0600 file / memory) | `SecretCipher` + `connectorSecrets` of `settings.ts` (Electron's `safeStorage` became the Keychain; the ciphertext in the settings file became one Keychain item per connector, account `connector:<id>`) |
| `ConnectorDeps` | `ConnectorDeps` (`github`, `harnesses`, `listAwsProfiles`, `tester`, `now`) plus `legacy` (the settings file the first versions kept the single account in) |
| `LegacySource` / `LegacyAccounts` | `SettingsStore.legacy()` / `clearLegacy()` |
| `ResolvedConnections` | `ResolvedConnections` (its `Debug` never prints a token or key) |
| `validate` | `validateLiveSetting('awsProfile' / 'awsRegion')`, `validateSecretFormat` |

## Differences on purpose

- **Secrets in the Keychain, not in a file.** The TypeScript kept `enc:` ciphertext in `settings.json`; the Rust store keeps the
  metadata in `connectors.json` and every token or key in the `SecretStore` (macOS Keychain, service `com.ratchet.app`). The
  old single-account secrets (`githubToken`, `anthropicApiKey`, `openaiApiKey`) are read from the same store and moved by the
  migration. "Keychain unavailable" is `SecretStore::available() == false`; "a secret that cannot be decrypted right now" is a
  `get` that fails (the migration then waits for the next launch).
- **The secret stores moved here** from `ratchet-engine` (the engine re-exports them): the connector store needs them and the
  engine now depends on this crate for `ResolvedConnections`.
- **Types are shared with the wire.** `model` is the one definition; `ratchetd` publishes it in its OpenAPI document, so the
  store and the API cannot drift. `UpdateConnectorInput.monthly_cap_usd` is `Option<Option<f64>>` (absent leaves the cap,
  `null` removes it), as `monthlyCapUsd?: number | null` is in TypeScript.
- **Probes** are the `ConnectorProbe` trait (`aws`, `anthropic`, `openai` returning `(ok, detail)`), the shape of
  `ConnectionTesterLike`. The "unreachable" heuristic also knows the wording of `reqwest` (`error sending request`, `dns error`).
- **`add_agent` takes `wait_for_check`** like the TypeScript option; with `false` it does not probe at all and the caller
  (`ratchetd`'s `ConnectorsHub`) probes in the background, where TypeScript started the probe unawaited inside the store.
- **Ids** are `prefix-` + 4 random bytes in hex (`rand`), like `randomBytes(4).toString('hex')`.
- `toLocaleString('en-US')` of the cap in a detail line is `usd_en_us` (thousands separators, at most two decimals).

## Test mapping

`tests/connectors.rs` (+ `tests/support.rs`, the fakes of `test-support.ts` and the `cipher`/`Tester` helpers of the test file).
25 TypeScript cases, one Rust test each (the TS name is in the doc comment), plus 4 Rust-only cases.

| TS case (`connectors.test.ts`) | Rust test |
|---|---|
| GitHub accounts › keeps several accounts; the first is the default and the default can move | `keeps_several_accounts_the_first_is_the_default_and_the_default_can_move` |
| › signing in again as the same account replaces its token; a connector id re-signs-in that connector | `signing_in_again_as_the_same_account_replaces_its_token` |
| › a pasted token is proven first (and a rejected one is not kept); a malformed one never reaches GitHub | `a_pasted_token_is_proven_first` |
| › a GITHUB_TOKEN in the environment is a connector that cannot be removed, and the default when nothing else is stored | `a_github_token_in_the_environment_is_a_connector_that_cannot_be_removed` |
| › an unusable keychain refuses to store a token and changes nothing | `an_unusable_keychain_refuses_to_store_a_token_and_changes_nothing` |
| AWS profiles › several profiles with a region and an optional monthly cap; detail and defaults | `several_profiles_with_a_region_and_an_optional_monthly_cap` |
| › validates the profile, region, cap and duplicates | `validates_the_profile_region_cap_and_duplicates` |
| › shows a profile that left ~/.aws as invalid, and probes: SSO that fails is expired, static keys invalid | `a_profile_that_left_aws_is_invalid_and_probes_sort_sso_from_keys` |
| agent credentials › detected CLI logins become connectors (installed ones only), logins before keys, with version and sign-in state | `detected_cli_logins_become_connectors_logins_before_keys` |
| › a CLI that is not installed has no login connector; removing one keeps it removed until it is added again | `a_cli_that_is_not_installed_has_no_login_connector` |
| › an API key is format-checked, checked with the provider, and shown only as its last characters; a rejected key is kept as invalid | `an_api_key_is_format_checked_probed_and_shown_only_as_its_tail` |
| › one default for the kind: the stored one, else the first usable (logins first) | `one_default_for_the_kind_the_stored_one_else_the_first_usable` |
| probes › GitHub: a revoked token is expired, a network failure leaves the status alone, a missing token is signed out | `github_revoked_is_expired_a_network_failure_changes_nothing` |
| › no secret value ever appears in a probe message | `no_secret_value_ever_appears_in_a_probe_message` |
| › refresh checks every connector and never throws | `refresh_checks_every_connector_and_never_throws` |
| where secrets live › tokens and keys are encrypted in the settings file; the metadata is plain; nothing leaks through a view model | `tokens_and_keys_live_in_the_secret_store_the_metadata_is_plain` |
| › survives a restart: the connectors, the defaults and the remembered selections come back | `survives_a_restart` |
| migration › the GitHub token, the AWS profile and the API keys become connectors, once; the old fields are emptied | `the_old_single_account_settings_become_connectors_once` |
| › waits for the keychain instead of losing a secret it cannot read yet | `migration_waits_for_the_keychain_instead_of_losing_a_secret` |
| › a fresh install has nothing to migrate | `a_fresh_install_has_nothing_to_migrate` |
| the choice remembered per repo › with nothing saved a repo uses the defaults | `with_nothing_saved_a_repo_uses_the_defaults` |
| › save then get, keyed without regard to case; resolve follows it | `save_then_get_keyed_without_regard_to_case_resolve_follows_it` |
| › a chosen login never gets a model key, even when one is stored | `a_chosen_login_never_gets_a_model_key` |
| › rejects choices that cannot run: unknown ids, a login in the cloud, two connectors for one agent, nothing chosen | `rejects_choices_that_cannot_run` |
| › removing a chosen connector falls back to the defaults for that part | `removing_a_chosen_connector_falls_back_to_the_defaults_for_that_part` |
| *(Rust only)* thousands separators in a cap | `caps_read_with_thousands_separators` |
| *(Rust only)* `null` vs absent in the update patch | `a_patch_tells_null_from_absent` |
| *(Rust only)* the wire form of a connector is the TS view model | `a_connector_serializes_like_the_ts_view_model` |
| *(Rust only)* `Debug` of what the engine receives hides secrets | `resolved_connections_do_not_debug_print_secrets` |

Also moved with the secret stores: `secrets::tests` (memory, file, unavailable) and, from `live-settings.ts`, `validate::tests`.
The device flow (`github-signin.ts`) stays in `ratchet-github` (`signin.rs`); it gained `start_for(connector_id)` and
`AccountStore::save` so a re-sign-in lands on a specific connector (`tests/api.rs::sign_in_for_a_connector_hands_its_id_to_the_store`).
The daemon side of the connectors group (`service.ts`) is `ratchetd::connectors_hub`; its tests are in `crates/ratchetd/PARITY.md`.
