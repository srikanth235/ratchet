//! Port of `packages/desktop/src/main/connectors.test.ts` (25 cases), case by case; the TS name is in each doc comment.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use ratchet_agents::detect::{HarnessAuth, HarnessInfo, HarnessKind};
use ratchet_connectors::secrets::{FileSecretStore, MemorySecretStore, SecretStore};
use ratchet_connectors::*;
use ratchet_core::enums::AgentsWhere;
use support::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(a, b)| ((*a).into(), (*b).into()))
        .collect()
}

fn gh(login: &str, token: &str, via_device: bool) -> UpsertGithub {
    UpsertGithub::new(login, token, via_device)
}

fn aws(profile: &str) -> AddAwsConnectorInput {
    AddAwsConnectorInput {
        profile: profile.into(),
        region: None,
        monthly_cap_usd: None,
        name: None,
    }
}

fn key(agent: ConnectorAgent, api_key: &str, name: Option<&str>) -> AddAgentConnectorInput {
    AddAgentConnectorInput::ApiKey {
        agent,
        api_key: api_key.into(),
        name: name.map(str::to_owned),
    }
}

fn err_text<T: std::fmt::Debug>(r: Result<T>) -> String {
    r.unwrap_err().to_string()
}

/// `[name, isDefault]` of every connector.
fn names_defaults(list: &[ConnectorVm]) -> Vec<(String, bool)> {
    list.iter()
        .map(|c| (c.name().to_owned(), c.is_default()))
        .collect()
}

// =========================================================================================================
// GitHub accounts
// =========================================================================================================

/// GitHub accounts › keeps several accounts; the first is the default and the default can move
#[tokio::test]
async fn keeps_several_accounts_the_first_is_the_default_and_the_default_can_move() {
    let m = make(Opts::default());
    let s = &m.store;
    let a = s.upsert_github(gh("srikanth235", GH_TOKEN, true)).unwrap();
    let b = s.upsert_github(gh("acme-org", ORG_TOKEN, false)).unwrap();
    let rows: Vec<_> = s
        .snapshot()
        .iter()
        .map(|c| {
            (
                c.name().to_owned(),
                github_source(c),
                c.is_default(),
                c.status(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "srikanth235".to_owned(),
                Some(GithubConnectorSource::Device),
                true,
                ConnectorStatus::Connected
            ),
            (
                "acme-org".to_owned(),
                Some(GithubConnectorSource::Token),
                false,
                ConnectorStatus::Connected
            ),
        ]
    );
    s.set_default(&b.id).unwrap();
    let defaults: Vec<bool> = s.snapshot().iter().map(ConnectorVm::is_default).collect();
    assert_eq!(defaults, [false, true]);
    assert_eq!(s.github_token(None).as_deref(), Some(ORG_TOKEN));
    assert_eq!(s.github_token(Some(&a.id)).as_deref(), Some(GH_TOKEN));
    s.remove(&b.id).unwrap(); // removing the default hands it to the one that is left
    assert_eq!(
        names_defaults(&s.snapshot()),
        vec![("srikanth235".to_owned(), true)]
    );
    assert_eq!(s.github_token(Some(&b.id)), None);
}

/// GitHub accounts › signing in again as the same account replaces its token; a connector id re-signs-in that connector
#[tokio::test]
async fn signing_in_again_as_the_same_account_replaces_its_token() {
    let m = make(Opts::default());
    let s = &m.store;
    let old = "gho_OLDTOKENOLDTOKENOLDTOKEN12";
    let new = "gho_NEWTOKENNEWTOKENNEWTOKEN12";
    let a = s.upsert_github(gh("octocat", old, true)).unwrap();
    s.upsert_github(gh("OctoCat", new, true)).unwrap();
    assert_eq!(s.snapshot().len(), 1);
    assert_eq!(s.github_token(Some(&a.id)).as_deref(), Some(new));
    s.upsert_github(UpsertGithub {
        connector_id: Some(a.id.clone()),
        ..gh("octocat-renamed", "gho_THIRDTOKENTHIRDTOKEN123456", true)
    })
    .unwrap();
    match &s.snapshot()[..] {
        [
            ConnectorVm::Github {
                id, login, name, ..
            },
        ] => {
            assert_eq!(id, &a.id);
            assert_eq!(login.as_deref(), Some("octocat-renamed"));
            assert_eq!(name, "octocat-renamed");
        }
        other => panic!("{other:?}"),
    }
    let r = s.upsert_github(UpsertGithub {
        connector_id: Some("nope".into()),
        ..gh("x", GH_TOKEN, true)
    });
    assert!(matches!(r, Err(ConnectorError::NotFound(_))), "{r:?}");
}

/// GitHub accounts › a pasted token is proven first (and a rejected one is not kept); a malformed one never reaches GitHub
#[tokio::test]
async fn a_pasted_token_is_proven_first() {
    let m = make(Opts::default());
    m.github.set_fail(Some(UserFailure::Rejected));
    let e = err_text(m.store.add_github_token(GH_TOKEN, None, false).await);
    assert!(e.contains("rejected the sign-in"), "{e}");
    assert!(m.store.snapshot().is_empty());
    let e = err_text(m.store.add_github_token("not-a-token", None, false).await);
    assert!(e.contains("GitHub tokens start with"), "{e}");
    m.github.set_fail(None);
    m.github.tokens_seen.lock().unwrap().clear();
    let rec = m
        .store
        .add_github_token(GH_TOKEN, Some("Work".into()), false)
        .await
        .unwrap();
    assert_eq!(rec.name, "Work");
    assert_eq!(
        rec.kind,
        RecordKind::Github {
            login: "octocat".into(),
            source: GithubStored::Token
        }
    );
    // the malformed token never left the machine; the good one was checked once
    assert_eq!(*m.github.tokens_seen.lock().unwrap(), vec![GH_TOKEN]);
}

/// GitHub accounts › a GITHUB_TOKEN in the environment is a connector that cannot be removed, and the default when nothing
/// else is stored
#[tokio::test]
async fn a_github_token_in_the_environment_is_a_connector_that_cannot_be_removed() {
    let m = make(Opts {
        env: env(&[("GITHUB_TOKEN", GH_TOKEN)]),
        ..Opts::default()
    });
    let s = &m.store;
    match &s.snapshot()[..] {
        [
            ConnectorVm::Github {
                id,
                source,
                is_default,
                status,
                ..
            },
        ] => {
            assert_eq!(id, "github-env");
            assert_eq!(*source, GithubConnectorSource::Env);
            assert!(*is_default);
            assert_eq!(*status, ConnectorStatus::Connected);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.github_token(None).as_deref(), Some(GH_TOKEN));
    let e = err_text(s.remove("github-env"));
    assert!(e.contains("environment"), "{e}");
    let own = s
        .upsert_github(gh("octocat", "gho_STOREDTOKENSTOREDTOKEN1234", true))
        .unwrap();
    // a stored account wins over the environment
    assert_eq!(
        s.default_of(ConnectorKind::Github).map(|r| r.id),
        Some(own.id)
    );
}

/// GitHub accounts › an unusable keychain refuses to store a token and changes nothing
#[tokio::test]
async fn an_unusable_keychain_refuses_to_store_a_token_and_changes_nothing() {
    let m = make(Opts {
        secrets: Arc::new(MemorySecretStore::unavailable()),
        ..Opts::default()
    });
    let r = m.store.upsert_github(gh("octocat", GH_TOKEN, true));
    assert!(
        matches!(&r, Err(ConnectorError::Keychain(m)) if m.contains("keychain")),
        "{r:?}"
    );
    assert!(m.store.snapshot().is_empty());
    assert!(!m.store.encryption_available());
}

// =========================================================================================================
// AWS profiles
// =========================================================================================================

/// AWS profiles › several profiles with a region and an optional monthly cap; detail and defaults
#[tokio::test]
async fn several_profiles_with_a_region_and_an_optional_monthly_cap() {
    let m = make(Opts::default());
    let s = &m.store;
    let prod = s
        .add_aws(
            AddAwsConnectorInput {
                region: Some("us-east-1".into()),
                monthly_cap_usd: Some(500.0),
                ..aws("acme-prod")
            },
            false,
        )
        .unwrap();
    s.add_aws(
        AddAwsConnectorInput {
            region: Some("us-west-2".into()),
            monthly_cap_usd: Some(100.0),
            ..aws("personal-dev")
        },
        false,
    )
    .unwrap();
    s.add_aws(
        AddAwsConnectorInput {
            region: Some("eu-west-1".into()),
            ..aws("acme-staging")
        },
        false,
    )
    .unwrap();
    let rows: Vec<_> = s
        .snapshot()
        .iter()
        .map(|c| (c.name().to_owned(), c.detail().to_owned(), c.is_default()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "acme-prod".to_owned(),
                "SSO · us-east-1 · cap $500 / month".to_owned(),
                true
            ),
            (
                "personal-dev".to_owned(),
                "~/.aws profile · us-west-2 · cap $100 / month".to_owned(),
                false
            ),
            (
                "acme-staging".to_owned(),
                "SSO · eu-west-1".to_owned(),
                false
            ),
        ]
    );
    s.update(
        &prod.id,
        UpdateConnectorInput {
            name: Some("Acme production".into()),
            region: Some("eu-west-2".into()),
            monthly_cap_usd: Some(None),
        },
    )
    .unwrap();
    match s.vm_of(&prod.id).unwrap() {
        ConnectorVm::Aws {
            name,
            region,
            monthly_cap_usd,
            ..
        } => {
            assert_eq!(name, "Acme production");
            assert_eq!(region, "eu-west-2");
            assert_eq!(monthly_cap_usd, None);
        }
        other => panic!("{other:?}"),
    }
}

/// AWS profiles › validates the profile, region, cap and duplicates
#[tokio::test]
async fn validates_the_profile_region_cap_and_duplicates() {
    let m = make(Opts::default());
    let s = &m.store;
    s.add_aws(aws("acme-prod"), false).unwrap();
    assert!(err_text(s.add_aws(aws("acme-prod"), false)).contains("already connected"));
    assert!(matches!(
        s.add_aws(aws("bad profile; rm -rf /"), false),
        Err(ConnectorError::Invalid(_))
    ));
    let region = AddAwsConnectorInput {
        region: Some("mars-north-9".into()),
        ..aws("x")
    };
    assert!(
        err_text(s.add_aws(region, false))
            .to_lowercase()
            .contains("region")
    );
    let cap = AddAwsConnectorInput {
        monthly_cap_usd: Some(-5.0),
        ..aws("y")
    };
    assert!(err_text(s.add_aws(cap, false)).contains("cap"));
    assert_eq!(s.snapshot().len(), 1);
    match &s.snapshot()[0] {
        ConnectorVm::Aws {
            region,
            monthly_cap_usd,
            ..
        } => {
            assert_eq!(region, "us-east-1");
            assert_eq!(*monthly_cap_usd, None);
        }
        other => panic!("{other:?}"),
    }
}

/// AWS profiles › shows a profile that left ~/.aws as invalid, and probes: SSO that fails is expired, static keys invalid
#[tokio::test]
async fn a_profile_that_left_aws_is_invalid_and_probes_sort_sso_from_keys() {
    let mut two = profiles();
    two.profiles.truncate(2);
    let m = make(Opts {
        aws: two,
        ..Opts::default()
    });
    let s = &m.store;
    let sso = s.add_aws(aws("acme-prod"), false).unwrap();
    let keys = s.add_aws(aws("personal-dev"), false).unwrap();
    let gone = s.add_aws(aws("acme-staging"), false).unwrap();
    let v = s.vm_of(&gone.id).unwrap();
    assert_eq!(v.status(), ConnectorStatus::Invalid);
    assert!(v.detail().contains("not found in ~/.aws"), "{}", v.detail());
    assert!(s.test(&sso.id).await.unwrap().ok);
    assert_eq!(
        s.vm_of(&sso.id).unwrap().status(),
        ConnectorStatus::Connected
    );
    m.tester.set_fail(Some("The SSO session has expired"));
    let r = s.test(&sso.id).await.unwrap();
    assert!(!r.ok);
    assert!(
        r.detail.contains("aws sso login --profile acme-prod"),
        "{}",
        r.detail
    );
    assert_eq!(s.vm_of(&sso.id).unwrap().status(), ConnectorStatus::Expired);
    s.test(&keys.id).await.unwrap();
    assert_eq!(
        s.vm_of(&keys.id).unwrap().status(),
        ConnectorStatus::Invalid
    );
}

// =========================================================================================================
// agent credentials
// =========================================================================================================

fn codex_signed_out() -> HarnessInfo {
    HarnessInfo {
        kind: HarnessKind::Codex,
        installed: true,
        version: Some("0.166".into()),
        path: Some("/bin/codex".into()),
        auth: HarnessAuth::SignedOut,
        hint: "x".into(),
    }
}

/// agent credentials › detected CLI logins become connectors (installed ones only), logins before keys, with version and
/// sign-in state
#[tokio::test]
async fn detected_cli_logins_become_connectors_logins_before_keys() {
    let mut c = claude();
    c.version = Some("2.1.314".into());
    let m = make(Opts {
        harnesses: Some(vec![c, codex_signed_out()]),
        ..Opts::default()
    });
    m.store
        .add_agent(
            key(
                ConnectorAgent::ClaudeCode,
                ANTHROPIC_KEY,
                Some("Claude Code · team key"),
            ),
            true,
        )
        .await
        .unwrap();
    let list = m.store.list().await;
    let rows: Vec<_> = agents_of(&list)
        .iter()
        .map(|c| {
            (
                c.name().to_owned(),
                c.detail().to_owned(),
                c.status(),
                c.is_default(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "Claude Code · your login".to_owned(),
                "Found on this Mac · v2.1.314".to_owned(),
                ConnectorStatus::Connected,
                true
            ),
            (
                "Claude Code · team key".to_owned(),
                format!(
                    "API key ····{} · also runs in the cloud",
                    last4(ANTHROPIC_KEY)
                ),
                ConnectorStatus::Valid,
                false
            ),
            (
                "Codex · your login".to_owned(),
                "Found on this Mac · v0.166 · run `codex login`".to_owned(),
                ConnectorStatus::SignedOut,
                false
            ),
        ]
    );
    let runs_on: Vec<Vec<AgentsWhere>> = agents_of(&list)
        .iter()
        .map(|c| match c {
            ConnectorVm::Agent { runs_on, .. } => runs_on.clone(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        runs_on,
        vec![
            vec![AgentsWhere::Local],
            vec![AgentsWhere::Local, AgentsWhere::Cloud],
            vec![AgentsWhere::Local]
        ]
    );
}

/// agent credentials › a CLI that is not installed has no login connector; removing one keeps it removed until it is added
/// again
#[tokio::test]
async fn a_cli_that_is_not_installed_has_no_login_connector() {
    let m = make(Opts::default());
    let s = &m.store;
    let names = |l: &[ConnectorVm]| -> Vec<String> {
        agents_of(l).iter().map(|c| c.name().to_owned()).collect()
    };
    assert_eq!(names(&s.list().await), ["Claude Code · your login"]);
    s.remove("agent-claude-code-login").unwrap();
    assert!(names(&s.list().await).is_empty());
    s.add_agent(
        AddAgentConnectorInput::Login {
            agent: ConnectorAgent::ClaudeCode,
        },
        true,
    )
    .await
    .unwrap();
    assert_eq!(names(&s.list().await), ["Claude Code · your login"]);
    let e = err_text(
        s.add_agent(
            AddAgentConnectorInput::Login {
                agent: ConnectorAgent::Codex,
            },
            true,
        )
        .await,
    );
    assert!(e.contains("not found"), "{e}");
}

/// agent credentials › an API key is format-checked, checked with the provider, and shown only as its last characters; a
/// rejected key is kept as invalid
#[tokio::test]
async fn an_api_key_is_format_checked_probed_and_shown_only_as_its_tail() {
    let m = make(Opts::default());
    let s = &m.store;
    let e = err_text(
        s.add_agent(key(ConnectorAgent::ClaudeCode, OPENAI_KEY, None), true)
            .await,
    );
    assert!(e.contains("Anthropic keys start with sk-ant-"), "{e}");
    let ok = s
        .add_agent(key(ConnectorAgent::ClaudeCode, ANTHROPIC_KEY, None), true)
        .await
        .unwrap();
    match s.vm_of(&ok.id).unwrap() {
        ConnectorVm::Agent {
            status, key_hint, ..
        } => {
            assert_eq!(status, ConnectorStatus::Valid);
            assert_eq!(key_hint, Some(format!("····{}", last4(ANTHROPIC_KEY))));
        }
        other => panic!("{other:?}"),
    }
    m.tester.set_fail(Some("invalid x-api-key"));
    let bad = s
        .add_agent(key(ConnectorAgent::Codex, OPENAI_KEY, None), true)
        .await
        .unwrap();
    let v = s.vm_of(&bad.id).unwrap();
    assert_eq!(v.status(), ConnectorStatus::Invalid);
    assert!(v.detail().contains("rejected"), "{}", v.detail());
    let json = serde_json::to_string(&s.snapshot()).unwrap();
    assert!(!json.contains(ANTHROPIC_KEY), "{json}");
    assert_eq!(
        s.secret_value(SecretName::AnthropicApiKey).as_deref(),
        Some(ANTHROPIC_KEY)
    );
}

/// agent credentials › one default for the kind: the stored one, else the first usable (logins first)
#[tokio::test]
async fn one_default_for_the_kind_the_stored_one_else_the_first_usable() {
    let mut signed_out_claude = claude();
    signed_out_claude.auth = HarnessAuth::SignedOut;
    signed_out_claude.hint = "x".into();
    let mut codex_in = codex_signed_out();
    codex_in.version = Some("1".into());
    codex_in.auth = HarnessAuth::SignedIn;
    codex_in.hint = "ok".into();
    let m = make(Opts {
        harnesses: Some(vec![signed_out_claude, codex_in]),
        ..Opts::default()
    });
    let s = &m.store;
    let list = s.list().await;
    let defaults: Vec<String> = list
        .iter()
        .filter(|c| c.is_default())
        .map(|c| c.name().to_owned())
        .collect();
    // the signed-out Claude login is skipped
    assert!(
        defaults.contains(&"Codex · your login".to_owned()),
        "{defaults:?}"
    );
    assert_eq!(
        list.iter()
            .filter(|c| c.kind() == ConnectorKind::Agent && c.is_default())
            .count(),
        1
    );
    let k = s
        .add_agent(key(ConnectorAgent::ClaudeCode, ANTHROPIC_KEY, None), true)
        .await
        .unwrap();
    s.set_default(&k.id).unwrap();
    let ids: Vec<String> = s
        .list()
        .await
        .iter()
        .filter(|c| c.kind() == ConnectorKind::Agent && c.is_default())
        .map(|c| c.id().to_owned())
        .collect();
    assert_eq!(ids, [k.id]);
}

// =========================================================================================================
// probes
// =========================================================================================================

/// probes › GitHub: a revoked token is expired, a network failure leaves the status alone, a missing token is signed out
#[tokio::test]
async fn github_revoked_is_expired_a_network_failure_changes_nothing() {
    let m = make(Opts::default());
    let s = &m.store;
    let a = s.upsert_github(gh("octocat", GH_TOKEN, true)).unwrap();
    assert_eq!(
        s.test(&a.id).await.unwrap(),
        TestOutcome {
            ok: true,
            detail: "signed in as octocat".into()
        }
    );
    m.github.set_fail(Some(UserFailure::Rejected));
    assert!(!s.test(&a.id).await.unwrap().ok);
    let v = s.vm_of(&a.id).unwrap();
    assert_eq!(v.status(), ConnectorStatus::Expired);
    assert!(v.detail().contains("sign in again"), "{}", v.detail());
    m.github.set_fail(Some(UserFailure::Offline));
    s.test(&a.id).await.unwrap();
    // unchanged
    assert_eq!(s.vm_of(&a.id).unwrap().status(), ConnectorStatus::Expired);
    m.github.set_fail(None);
    s.test(&a.id).await.unwrap();
    assert_eq!(s.vm_of(&a.id).unwrap().status(), ConnectorStatus::Connected);
}

/// probes › no secret value ever appears in a probe message
#[tokio::test]
async fn no_secret_value_ever_appears_in_a_probe_message() {
    let m = make(Opts::default());
    let k = m
        .store
        .add_agent(key(ConnectorAgent::ClaudeCode, ANTHROPIC_KEY, None), true)
        .await
        .unwrap();
    m.tester
        .set_fail(Some(&format!("401 for key {ANTHROPIC_KEY}")));
    let r = m.store.test(&k.id).await.unwrap();
    assert!(!r.detail.contains(ANTHROPIC_KEY), "{}", r.detail);
    assert!(r.detail.contains("***"), "{}", r.detail);
}

/// probes › refresh checks every connector and never throws
#[tokio::test]
async fn refresh_checks_every_connector_and_never_throws() {
    let m = make(Opts::default());
    let s = &m.store;
    s.upsert_github(gh("octocat", GH_TOKEN, true)).unwrap();
    s.add_aws(aws("acme-prod"), false).unwrap();
    m.tester.set_fail(Some("boom"));
    s.refresh().await;
    // the GitHub account is checked with GitHub; the CLI login is not probed
    assert_eq!(m.tester.calls(), ["aws:acme-prod"]);
    let status = s
        .snapshot()
        .iter()
        .find(|c| c.kind() == ConnectorKind::Aws)
        .map(ConnectorVm::status);
    assert_eq!(status, Some(ConnectorStatus::Expired));
}

// =========================================================================================================
// where secrets live
// =========================================================================================================

/// where secrets live › tokens and keys are encrypted in the settings file; the metadata is plain; nothing leaks through a
/// view model (here: the secrets are in the secret store, never in `connectors.json`)
#[tokio::test]
async fn tokens_and_keys_live_in_the_secret_store_the_metadata_is_plain() {
    let dir = tempfile::tempdir().unwrap();
    let file = tmp_file(&dir);
    let secret_file = dir.path().join("secrets.json");
    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(&secret_file));
    let m = make(Opts {
        secrets,
        file: Some(file.clone()),
        ..Opts::default()
    });
    let s = &m.store;
    s.upsert_github(gh("octocat", GH_TOKEN, true)).unwrap();
    s.add_agent(key(ConnectorAgent::ClaudeCode, ANTHROPIC_KEY, None), true)
        .await
        .unwrap();
    s.add_aws(
        AddAwsConnectorInput {
            monthly_cap_usd: Some(250.0),
            ..aws("acme-prod")
        },
        false,
    )
    .unwrap();
    let raw = contents(&file);
    for secret in [GH_TOKEN, ANTHROPIC_KEY] {
        assert!(!raw.contains(secret), "{raw}");
    }
    assert!(raw.contains("\"login\": \"octocat\""), "{raw}");
    assert!(raw.contains("\"monthlyCapUsd\": 250"), "{raw}");
    assert!(contents(&secret_file).contains(GH_TOKEN));
    let listed = serde_json::to_string(&s.list().await).unwrap();
    assert!(
        !listed.contains("ghp_") && !listed.contains("sk-ant"),
        "{listed}"
    );
    assert!(s.encryption_available());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the connectors file is private");
    }
}

/// where secrets live › survives a restart: the connectors, the defaults and the remembered selections come back
#[tokio::test]
async fn survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = tmp_file(&dir);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let first = make(Opts {
        secrets: secrets.clone(),
        file: Some(file.clone()),
        ..Opts::default()
    });
    let a = first
        .store
        .upsert_github(gh("octocat", GH_TOKEN, true))
        .unwrap();
    let b = first
        .store
        .upsert_github(gh("acme-org", ORG_TOKEN, false))
        .unwrap();
    first.store.set_default(&b.id).unwrap();
    let aws_rec = first.store.add_aws(aws("acme-prod"), false).unwrap();
    first.store.list().await;
    first
        .store
        .save_repo_connections(
            "Acme/Api",
            RepoConnections {
                github: a.id.clone(),
                aws: aws_rec.id.clone(),
                agents: vec![AgentSelection {
                    connector_id: "agent-claude-code-login".into(),
                    where_: AgentsWhere::Local,
                }],
            },
        )
        .unwrap();
    let second = make(Opts {
        secrets,
        file: Some(file),
        ..Opts::default()
    });
    assert_eq!(
        names_defaults(&second.store.snapshot()),
        vec![
            ("octocat".to_owned(), false),
            ("acme-org".to_owned(), true),
            ("acme-prod".to_owned(), true),
            ("Claude Code · your login".to_owned(), true),
        ]
    );
    assert_eq!(
        second.store.github_token(Some(&a.id)).as_deref(),
        Some(GH_TOKEN)
    );
    let saved = second.store.repo_connections("acme/api").saved.unwrap();
    assert_eq!((saved.github, saved.aws), (a.id, aws_rec.id));
}

// =========================================================================================================
// migration of the single-account Settings
// =========================================================================================================

/// migration of the single-account Settings › the GitHub token, the AWS profile and the API keys become connectors, once;
/// the old fields are emptied
#[tokio::test]
async fn the_old_single_account_settings_become_connectors_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = tmp_file(&dir);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let old = OldSettings::new(secrets.clone());
    old.set_secret(SecretName::GithubToken, GH_TOKEN);
    old.set_github_account("octocat", true);
    old.set_aws("acme-prod", "eu-west-2");
    old.set_secret(SecretName::AnthropicApiKey, ANTHROPIC_KEY);
    old.set_secret(SecretName::OpenaiApiKey, OPENAI_KEY);

    let m = make(Opts {
        secrets: secrets.clone(),
        file: Some(file.clone()),
        legacy: Some(old.clone()),
        ..Opts::default()
    });
    let list = m.store.list().await;
    let rows: Vec<_> = list
        .iter()
        .map(|c| (c.kind(), c.name().to_owned(), c.is_default()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (ConnectorKind::Github, "octocat".to_owned(), true),
            (ConnectorKind::Aws, "acme-prod".to_owned(), true),
            (
                ConnectorKind::Agent,
                "Claude Code · your login".to_owned(),
                true
            ),
            (
                ConnectorKind::Agent,
                "Claude Code · API key".to_owned(),
                false
            ),
            (ConnectorKind::Agent, "Codex · API key".to_owned(), false),
        ]
    );
    match list
        .iter()
        .find(|c| c.kind() == ConnectorKind::Aws)
        .unwrap()
    {
        ConnectorVm::Aws {
            region, profile, ..
        } => assert_eq!(
            (region.as_str(), profile.as_str()),
            ("eu-west-2", "acme-prod")
        ),
        other => panic!("{other:?}"),
    }
    match list
        .iter()
        .find(|c| c.kind() == ConnectorKind::Github)
        .unwrap()
    {
        ConnectorVm::Github { source, login, .. } => {
            assert_eq!(*source, GithubConnectorSource::Device);
            assert_eq!(login.as_deref(), Some("octocat"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(m.store.github_token(None).as_deref(), Some(GH_TOKEN));
    assert_eq!(
        m.store.secret_value(SecretName::AnthropicApiKey).as_deref(),
        Some(ANTHROPIC_KEY)
    );
    assert_eq!(
        m.store.secret_value(SecretName::OpenaiApiKey).as_deref(),
        Some(OPENAI_KEY)
    );
    assert!(old.is_empty(), "the old fields are emptied");

    // a second launch does not migrate again
    let again = make(Opts {
        secrets,
        file: Some(file),
        legacy: Some(old),
        ..Opts::default()
    });
    let list = again.store.list().await;
    assert_eq!(
        list.iter()
            .filter(|c| c.kind() != ConnectorKind::Agent)
            .count(),
        2
    );
    assert_eq!(agents_of(&list).len(), 3);
}

/// migration of the single-account Settings › waits for the keychain instead of losing a secret it cannot read yet
#[tokio::test]
async fn migration_waits_for_the_keychain_instead_of_losing_a_secret() {
    let dir = tempfile::tempdir().unwrap();
    let file = tmp_file(&dir);
    let locked_store = LockableStore::new();
    let secrets: Arc<dyn SecretStore> = locked_store.clone();
    let old = OldSettings::new(secrets.clone());
    old.set_secret(SecretName::GithubToken, GH_TOKEN);
    old.set_aws("acme-prod", "");
    locked_store.lock(true);
    let locked = make(Opts {
        secrets: secrets.clone(),
        file: Some(file.clone()),
        legacy: Some(old.clone()),
        harnesses: None,
        ..Opts::default()
    });
    assert!(locked.store.snapshot().is_empty());
    locked_store.lock(false);
    let later = make(Opts {
        secrets,
        file: Some(file),
        legacy: Some(old),
        harnesses: None,
        ..Opts::default()
    });
    let kinds: Vec<ConnectorKind> = later
        .store
        .snapshot()
        .iter()
        .map(ConnectorVm::kind)
        .collect();
    assert_eq!(kinds, [ConnectorKind::Github, ConnectorKind::Aws]);
    assert_eq!(later.store.github_token(None).as_deref(), Some(GH_TOKEN));
}

/// migration of the single-account Settings › a fresh install has nothing to migrate
#[tokio::test]
async fn a_fresh_install_has_nothing_to_migrate() {
    let dir = tempfile::tempdir().unwrap();
    let file = tmp_file(&dir);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let old = OldSettings::new(secrets.clone());
    let m = make(Opts {
        secrets,
        file: Some(file.clone()),
        legacy: Some(old),
        harnesses: None,
        ..Opts::default()
    });
    assert!(m.store.snapshot().is_empty());
    assert!(contents(&file).contains("\"migrated\": true"));
}

// =========================================================================================================
// the choice remembered per repo
// =========================================================================================================

struct Three {
    m: Made,
    g1: String,
    g2: String,
    a1: String,
    a2: String,
    key: String,
    login: String,
}

async fn three() -> Three {
    let m = make(Opts::default());
    let s = &m.store;
    let g1 = s.upsert_github(gh("octocat", GH_TOKEN, true)).unwrap().id;
    let g2 = s
        .upsert_github(gh("acme-org", ORG_TOKEN, false))
        .unwrap()
        .id;
    let a1 = s
        .add_aws(
            AddAwsConnectorInput {
                region: Some("us-east-1".into()),
                ..aws("acme-prod")
            },
            false,
        )
        .unwrap()
        .id;
    let a2 = s
        .add_aws(
            AddAwsConnectorInput {
                region: Some("us-west-2".into()),
                monthly_cap_usd: Some(100.0),
                ..aws("personal-dev")
            },
            false,
        )
        .unwrap()
        .id;
    s.list().await;
    let key = s
        .add_agent(key(ConnectorAgent::ClaudeCode, ANTHROPIC_KEY, None), true)
        .await
        .unwrap()
        .id;
    Three {
        m,
        g1,
        g2,
        a1,
        a2,
        key,
        login: "agent-claude-code-login".into(),
    }
}

fn sel(id: &str, where_: AgentsWhere) -> AgentSelection {
    AgentSelection {
        connector_id: id.into(),
        where_,
    }
}

/// the choice remembered per repo › with nothing saved a repo uses the defaults
#[tokio::test]
async fn with_nothing_saved_a_repo_uses_the_defaults() {
    let t = three().await;
    let s = &t.m.store;
    assert_eq!(
        s.repo_connections("acme/api"),
        RepoConnectionsVm {
            repo: "acme/api".into(),
            saved: None,
            effective: EffectiveConnections {
                github: Some(t.g1.clone()),
                aws: Some(t.a1.clone()),
                agents: vec![sel(&t.login, AgentsWhere::Local)],
            },
        }
    );
    let r = s.resolve(Some("acme/api")).await;
    let g = r.github.as_ref().unwrap();
    assert_eq!((g.id.as_str(), g.token.as_str()), (t.g1.as_str(), GH_TOKEN));
    let a = r.aws.as_ref().unwrap();
    assert_eq!(
        (a.profile.as_str(), a.region.as_str()),
        ("acme-prod", "us-east-1")
    );
    let agents: Vec<_> = r
        .agents
        .iter()
        .map(|a| (a.agent, a.source, a.where_, a.api_key.clone()))
        .collect();
    assert_eq!(
        agents,
        vec![(
            ConnectorAgent::ClaudeCode,
            AgentConnectorSource::Login,
            AgentsWhere::Local,
            None
        )]
    );
    assert_eq!(
        r.cloud_keys
            .get(&ConnectorAgent::ClaudeCode)
            .map(String::as_str),
        Some(ANTHROPIC_KEY)
    );
    assert_eq!(r.cloud_keys.len(), 1);
}

/// the choice remembered per repo › save then get, keyed without regard to case; resolve follows it
#[tokio::test]
async fn save_then_get_keyed_without_regard_to_case_resolve_follows_it() {
    let t = three().await;
    let s = &t.m.store;
    let chosen = RepoConnections {
        github: t.g2.clone(),
        aws: t.a2.clone(),
        agents: vec![sel(&t.key, AgentsWhere::Cloud)],
    };
    let saved = s
        .save_repo_connections("Acme/Shop-API", chosen.clone())
        .unwrap();
    assert_eq!(saved.repo, "acme/shop-api");
    assert_eq!(saved.saved.as_ref(), Some(&chosen));
    assert_eq!(s.repo_connections("acme/SHOP-api").saved, saved.saved);
    assert!(s.repo_connections("acme/other").saved.is_none());
    let r = s.resolve(Some("acme/shop-api")).await;
    let g = r.github.as_ref().unwrap();
    assert_eq!(g.login.as_deref(), Some("acme-org"));
    assert_eq!(g.token, ORG_TOKEN);
    let a = r.aws.as_ref().unwrap();
    assert_eq!(
        (a.profile.as_str(), a.region.as_str(), a.monthly_cap_usd),
        ("personal-dev", "us-west-2", Some(100.0))
    );
    assert_eq!(r.agents.len(), 1);
    let ag = &r.agents[0];
    assert_eq!(
        (ag.agent, ag.source, ag.where_, ag.api_key.as_deref()),
        (
            ConnectorAgent::ClaudeCode,
            AgentConnectorSource::ApiKey,
            AgentsWhere::Cloud,
            Some(ANTHROPIC_KEY)
        )
    );
    assert_eq!(
        s.where_of(&saved.saved.unwrap().agents).unwrap(),
        Some(AgentsWhere::Cloud)
    );
    // another repo is unaffected
    assert_eq!(
        s.resolve(Some("acme/other")).await.github.map(|g| g.id),
        Some(t.g1.clone())
    );
}

/// the choice remembered per repo › a chosen login never gets a model key, even when one is stored
#[tokio::test]
async fn a_chosen_login_never_gets_a_model_key() {
    let t = three().await;
    let s = &t.m.store;
    s.save_repo_connections(
        "acme/api",
        RepoConnections {
            github: t.g1.clone(),
            aws: t.a1.clone(),
            agents: vec![sel(&t.login, AgentsWhere::Local)],
        },
    )
    .unwrap();
    assert!(s.resolve(Some("acme/api")).await.cloud_keys.is_empty());
}

/// the choice remembered per repo › rejects choices that cannot run: unknown ids, a login in the cloud, two connectors for
/// one agent, nothing chosen
#[tokio::test]
async fn rejects_choices_that_cannot_run() {
    let t = three().await;
    let s = &t.m.store;
    let ok = |github: &str, aws: &str, agents: Vec<AgentSelection>| RepoConnections {
        github: github.into(),
        aws: aws.into(),
        agents,
    };
    let login_local = vec![sel(&t.login, AgentsWhere::Local)];
    let e = err_text(s.save_repo_connections("a/b", ok("nope", &t.a1, login_local.clone())));
    assert!(e.contains("GitHub account"), "{e}");
    let e = err_text(s.save_repo_connections("a/b", ok(&t.g1, &t.g1, login_local)));
    assert!(e.contains("AWS"), "{e}");
    let e = err_text(s.save_repo_connections("a/b", ok(&t.g1, &t.a1, vec![])));
    assert!(e.contains("at least one agent"), "{e}");
    let e = err_text(s.save_repo_connections(
        "a/b",
        ok(&t.g1, &t.a1, vec![sel(&t.login, AgentsWhere::Cloud)]),
    ));
    assert!(e.contains("cannot run in the cloud"), "{e}");
    let e = err_text(s.save_repo_connections(
        "a/b",
        ok(
            &t.g1,
            &t.a1,
            vec![
                sel(&t.login, AgentsWhere::Local),
                sel(&t.key, AgentsWhere::Local),
            ],
        ),
    ));
    assert!(e.contains("one Claude Code connector"), "{e}");
    assert!(s.repo_connections("a/b").saved.is_none());
    let e = err_text(s.where_of(&[
        sel(&t.login, AgentsWhere::Local),
        sel(&t.key, AgentsWhere::Cloud),
    ]));
    assert!(e.contains("Run all agents"), "{e}");
}

/// the choice remembered per repo › removing a chosen connector falls back to the defaults for that part
#[tokio::test]
async fn removing_a_chosen_connector_falls_back_to_the_defaults_for_that_part() {
    let t = three().await;
    let s = &t.m.store;
    s.save_repo_connections(
        "acme/api",
        RepoConnections {
            github: t.g2.clone(),
            aws: t.a2.clone(),
            agents: vec![sel(&t.login, AgentsWhere::Local)],
        },
    )
    .unwrap();
    s.remove(&t.a2).unwrap();
    let v = s.repo_connections("acme/api");
    assert!(v.saved.is_none());
    assert_eq!(v.effective.github.as_deref(), Some(t.g2.as_str()));
    assert_eq!(v.effective.aws.as_deref(), Some(t.a1.as_str()));
    assert_eq!(
        s.resolve(Some("acme/api")).await.aws.map(|a| a.profile),
        Some("acme-prod".to_owned())
    );
}

// =========================================================================================================
// extras of the port (not in the TS file)
// =========================================================================================================

/// A cap of $1,234.5 reads with a thousands separator, like `toLocaleString('en-US')`.
#[tokio::test]
async fn caps_read_with_thousands_separators() {
    let m = make(Opts::default());
    let r = m
        .store
        .add_aws(
            AddAwsConnectorInput {
                monthly_cap_usd: Some(1234.5),
                ..aws("acme-prod")
            },
            false,
        )
        .unwrap();
    assert_eq!(
        m.store.vm_of(&r.id).unwrap().detail(),
        "SSO · us-east-1 · cap $1,234.5 / month"
    );
}

/// The update patch tells "remove the cap" (`null`) from "leave it" (absent) on the wire.
#[test]
fn a_patch_tells_null_from_absent() {
    let remove: UpdateConnectorInput = serde_json::from_str(r#"{"monthlyCapUsd": null}"#).unwrap();
    let keep: UpdateConnectorInput = serde_json::from_str(r#"{"name": "x"}"#).unwrap();
    let set: UpdateConnectorInput = serde_json::from_str(r#"{"monthlyCapUsd": 50}"#).unwrap();
    assert_eq!(remove.monthly_cap_usd, Some(None));
    assert_eq!(keep.monthly_cap_usd, None);
    assert_eq!(set.monthly_cap_usd, Some(Some(50.0)));
}

/// The wire form of a connector is the TS `ConnectorVM`: `kind` tags the union, fields are camelCase, nullable fields are
/// always present.
#[test]
fn a_connector_serializes_like_the_ts_view_model() {
    let v = ConnectorVm::Aws {
        id: "aws-1".into(),
        name: "acme".into(),
        detail: "d".into(),
        status: ConnectorStatus::SignedOut,
        is_default: true,
        profile: "acme".into(),
        region: "us-east-1".into(),
        auth: AwsAuthStyle::Sso,
        monthly_cap_usd: None,
    };
    assert_eq!(
        serde_json::to_value(&v).unwrap(),
        serde_json::json!({
            "kind": "aws", "id": "aws-1", "name": "acme", "detail": "d", "status": "signed-out", "isDefault": true,
            "profile": "acme", "region": "us-east-1", "auth": "sso", "monthlyCapUsd": null
        })
    );
    let sel = serde_json::to_value(sel("c", AgentsWhere::Cloud)).unwrap();
    assert_eq!(
        sel,
        serde_json::json!({"connectorId": "c", "where": "cloud"})
    );
    let login: AddAgentConnectorInput =
        serde_json::from_str(r#"{"agent":"codex","source":"login"}"#).unwrap();
    assert_eq!(
        login,
        AddAgentConnectorInput::Login {
            agent: ConnectorAgent::Codex
        }
    );
    let k: AddAgentConnectorInput =
        serde_json::from_str(r#"{"agent":"claude-code","source":"api-key","apiKey":"sk-ant-x"}"#)
            .unwrap();
    assert!(matches!(k, AddAgentConnectorInput::ApiKey { .. }));
}

/// Secrets never show in `Debug` output of what the engine receives.
#[tokio::test]
async fn resolved_connections_do_not_debug_print_secrets() {
    let t = three().await;
    let r = t.m.store.resolve(Some("acme/api")).await;
    let dbg = format!("{r:?}");
    for secret in [GH_TOKEN, ANTHROPIC_KEY] {
        assert!(!dbg.contains(secret), "{dbg}");
    }
}
