//! `RealService` over recording fake runtimes: ports of `packages/desktop/src/main/service.test.ts` (GitHub, repo picker,
//! templates, creating, preflight, AWS and agents, environments, secrets) plus the provisional groups answering 501.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_agents::detect::{HarnessAuth as DAuth, HarnessInfo, HarnessKind as DKind};
use ratchet_core::enums::{AgentKind, AgentsWhere, Direction, SandboxProviderKind, SandboxStatus};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{
    CancellationToken, Clock, EngineEvent, PortError, PortResult, ProposeTemplateInput,
    ProposeTemplateResult, SandboxActivity, SandboxPhase, SandboxStateStatus,
    SandboxTeardownReceipt, TeardownRemoved, TeardownRun, TeardownTrigger,
};
use ratchet_engine::desktop::testing::{DiffGit, RecordingRuntime};
use ratchet_engine::desktop::{
    HarnessCache, HarnessProbe, LiveFactory, LiveHandle, MemorySecretStore, SimFactory, SimHandle,
};
use ratchet_github::{DevicePoll, MockGithubApi};
use ratchet_ledger::{CreateOptimizationInput as LedgerCreate, Ledger};
use ratchetd::api::*;
use ratchetd::real::{
    AwsProfileList, ConnectionProbe, RealService, RealServiceOptions, SettingsStore,
};
use ratchetd::service::{AppService, ServiceError};

const GH_TOKEN: &str = "ghp_TOPSECRETTOKENVALUE1234567890abcd";
const ANTHROPIC_KEY: &str = "sk-ant-api03-TOPSECRETANTHROPICKEY";
const OPENAI_KEY: &str = "sk-proj-TOPSECRETOPENAIKEYVALUE";
const DEVICE_TOKEN: &str = "gho_DEVICETOPSECRETTOKEN1234567";
const SECRETS: [&str; 4] = [GH_TOKEN, ANTHROPIC_KEY, OPENAI_KEY, DEVICE_TOKEN];

/// A ratchet.yaml as a repo would declare it: a template, a container, a machine, a region and one extra protected glob.
const YAML_OK: &str = "version: 0
objective: { name: Checkout latency, metric: http_req_duration.p95, direction: minimize }
evaluator: { template: http-latency }
environment: { image: .devcontainer/Dockerfile, machine: c7i.4xlarge, region: eu-west-2 }
protected: [docs/**]
";

/// Real time for the Last-Event/flow clock, but `sleep` returns at once (the device-flow poll interval is 5 s).
struct FastClock;

#[async_trait]
impl Clock for FastClock {
    fn now(&self) -> i64 {
        SystemClock.now()
    }
    async fn sleep(&self, _ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()> {
        tokio::time::sleep(Duration::from_millis(1)).await;
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(PortError::Cancelled);
        }
        Ok(())
    }
}

#[derive(Default)]
struct FakeProbe {
    calls: Mutex<Vec<(String, String)>>,
    fail_with: Mutex<Option<String>>,
}

impl FakeProbe {
    fn reply(&self, target: &str, input: String, ok: &str) -> (bool, String) {
        self.calls.lock().unwrap().push((target.to_owned(), input));
        match self.fail_with.lock().unwrap().clone() {
            Some(m) => (false, m),
            None => (true, ok.to_owned()),
        }
    }
}

#[async_trait]
impl ConnectionProbe for FakeProbe {
    async fn aws(&self, region: &str, profile: &str) -> (bool, String) {
        self.reply(
            "aws",
            format!("{region}/{profile}"),
            "signed in to account 123456789012",
        )
    }
    async fn anthropic(&self, key: &str) -> (bool, String) {
        self.reply("anthropic", key.to_owned(), "Anthropic accepted the key")
    }
    async fn openai(&self, key: &str) -> (bool, String) {
        self.reply("openai", key.to_owned(), "OpenAI accepted the key")
    }
}

fn claude(over: impl FnOnce(&mut HarnessInfo)) -> HarnessInfo {
    let mut h = HarnessInfo {
        kind: DKind::ClaudeCode,
        installed: true,
        version: Some("2.1.4".into()),
        path: Some("/bin/claude".into()),
        auth: DAuth::SignedIn,
        hint: "signed in".into(),
    };
    over(&mut h);
    h
}

fn codex(over: impl FnOnce(&mut HarnessInfo)) -> HarnessInfo {
    let mut h = HarnessInfo {
        kind: DKind::Codex,
        installed: false,
        version: None,
        path: None,
        auth: DAuth::SignedOut,
        hint: "Install Codex: npm i -g @openai/codex".into(),
    };
    over(&mut h);
    h
}

struct ScriptedProbe {
    probes: Arc<std::sync::atomic::AtomicUsize>,
    found: Box<dyn Fn(usize) -> Vec<HarnessInfo> + Send + Sync>,
}

#[async_trait]
impl HarnessProbe for ScriptedProbe {
    async fn login_path(&self) -> Result<Option<String>, String> {
        Ok(Some("/bin".into()))
    }
    async fn detect(&self, _: Option<&str>) -> Result<Vec<HarnessInfo>, String> {
        let n = self.probes.fetch_add(1, Ordering::SeqCst) + 1;
        Ok((self.found)(n))
    }
}

fn receipt(trigger: TeardownTrigger) -> SandboxTeardownReceipt {
    SandboxTeardownReceipt {
        sandbox_id: "ratchet-sandbox".into(),
        region: "us-east-1".into(),
        trigger,
        created_at: Some(1_000),
        // 2026-10-30T12:00:00Z
        destroyed_at: 1_793_361_600_000,
        runs: vec![TeardownRun {
            optimization_id: OptimizationId::new("opt_demo_ingest"),
            status: "completed".into(),
            spend_usd: 4.2,
        }],
        spend_usd: 4.2,
        spend_by_category: [("tokens".to_owned(), 3.0), ("bench_runner".to_owned(), 1.2)].into(),
        removed: TeardownRemoved {
            instances: vec!["i-1".into(), "i-2".into()],
            agent_identities: vec!["a1".into()],
            secrets: vec!["s1".into()],
            resources: vec!["bucket".into(), "vpc".into()],
        },
        nothing_running: true,
        orphans: vec![],
        errors: vec![],
        summary: "Removed 2 instances; nothing is left running.".into(),
    }
}

fn activity(over: impl FnOnce(&mut SandboxActivity)) -> SandboxActivity {
    let mut a = SandboxActivity {
        sandbox_id: "ratchet-sandbox".into(),
        status: SandboxStateStatus::Applied,
        region: Some("eu-west-2".into()),
        auto: true,
        active_runs: vec![],
        instances_running: Some(2),
        spend_usd: 1.5,
        idle_since: Some(5_000),
        destroy_at: Some(1_805_000),
        last_receipt: None,
    };
    over(&mut a);
    a
}

type Sims = Arc<Mutex<Vec<(String, Arc<RecordingRuntime>)>>>;
type Hook = Arc<dyn Fn(&Arc<RecordingRuntime>) + Send + Sync>;

#[derive(Default)]
struct Opts {
    env: HashMap<String, String>,
    harnesses: Option<Vec<HarnessInfo>>,
    missing: Vec<String>,
    on_live: Option<Hook>,
}

struct Setup {
    service: RealService,
    ledger: Ledger,
    settings: Arc<SettingsStore>,
    gh: Arc<MockGithubApi>,
    probe: Arc<FakeProbe>,
    opened: Arc<Mutex<Vec<String>>>,
    live: Arc<Mutex<Vec<Arc<RecordingRuntime>>>>,
    sims: Sims,
    probes: Arc<std::sync::atomic::AtomicUsize>,
    _dir: tempfile::TempDir,
}

async fn setup(o: Opts) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let settings = Arc::new(SettingsStore::new(
        None,
        Arc::new(MemorySecretStore::new()),
        o.env.clone(),
    ));
    let gh = Arc::new(MockGithubApi::new());
    let probe = Arc::new(FakeProbe::default());
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let live: Arc<Mutex<Vec<Arc<RecordingRuntime>>>> = Arc::default();
    let sims: Sims = Arc::default();
    let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let found = o.harnesses.clone();
    let harnesses = Arc::new(HarnessCache::new(Arc::new(ScriptedProbe {
        probes: probes.clone(),
        found: Box::new(move |_| {
            found
                .clone()
                .unwrap_or_else(|| vec![claude(|_| {}), codex(|_| {})])
        }),
    })));
    // the ledger is opened by the service; the fake runtimes need it, so they get it from their first call
    let ledger_cell: Arc<Mutex<Option<Ledger>>> = Arc::default();
    let create_runtime: LiveFactory = {
        let (live, cell, missing, hook) = (
            live.clone(),
            ledger_cell.clone(),
            o.missing.clone(),
            o.on_live.clone(),
        );
        Arc::new(move |opts| {
            let (live, cell, missing, hook) =
                (live.clone(), cell.clone(), missing.clone(), hook.clone());
            Box::pin(async move {
                let ledger = cell.lock().unwrap().clone().expect("ledger");
                let n = live.lock().unwrap().len();
                let engine = RecordingRuntime::new(&format!("live{n}"), &ledger);
                if let Some(h) = &hook {
                    h(&engine);
                }
                live.lock().unwrap().push(engine.clone());
                Ok(LiveHandle {
                    engine,
                    repo_url: opts
                        .live
                        .github_repo
                        .as_ref()
                        .map(|r| format!("https://github.com/{r}.git")),
                    missing,
                    agents_where: AgentsWhere::Local,
                    runner_problem: None,
                    disposer: Arc::new(|| Box::pin(async {})),
                })
            })
        })
    };
    let create_simulation: SimFactory = {
        let (sims, cell) = (sims.clone(), ledger_cell.clone());
        Arc::new(move |o| {
            let (sims, cell) = (sims.clone(), cell.clone());
            Box::pin(async move {
                let ledger = cell.lock().unwrap().clone().expect("ledger");
                let project = o.project.as_str().to_owned();
                let engine = RecordingRuntime::new(&format!("sim-{project}"), &ledger);
                sims.lock().unwrap().push((project.clone(), engine.clone()));
                Ok(SimHandle {
                    engine,
                    repo_url: format!("/work/{project}/remote.git"),
                    base_ref: "main".into(),
                    git: Arc::new(DiffGit),
                    disposer: Arc::new(|| Box::pin(async {})),
                })
            })
        })
    };
    let mut opts = RealServiceOptions::new(settings.clone(), dir.path().join("state"));
    opts.repo_root = dir.path().to_path_buf();
    opts.work_root = dir.path().join("sim");
    opts.app_version = Some("1.2.3".into());
    opts.clock = Some(Arc::new(FastClock));
    opts.github = Some(gh.clone());
    let op = opened.clone();
    opts.open_external = Some(Arc::new(move |u| op.lock().unwrap().push(u.to_owned())));
    opts.harnesses = harnesses;
    opts.aws_profiles = Some(Arc::new(|| AwsProfileList {
        profiles: vec![
            AwsProfileVm {
                name: "default".into(),
                kind: AwsProfileKind::Keys,
                source: AwsProfileSource::Credentials,
                sso_session: None,
            },
            AwsProfileVm {
                name: "corp-sso".into(),
                kind: AwsProfileKind::Sso,
                source: AwsProfileSource::Config,
                sso_session: Some("corp".into()),
            },
        ],
        sso_sessions: vec!["corp".into()],
        files_found: true,
    }));
    opts.connection_probe = Some(probe.clone());
    // never look the AMIs up in a real AWS account (the test machine may even have credentials)
    opts.resolve_images = Some(Arc::new(|q: ratchet_engine::desktop::ImageQuery| {
        Box::pin(async move {
            Ok(ratchet_sandbox::ResolvedImages {
                bench: q
                    .need
                    .contains(&ratchet_sandbox::images::RatchetImageKind::Bench)
                    .then(|| "ami-0bench00000000000a".to_owned()),
                runner: q
                    .need
                    .contains(&ratchet_sandbox::images::RatchetImageKind::Runner)
                    .then(|| "ami-0runner0000000000a".to_owned()),
                ..Default::default()
            })
        })
    }));
    opts.create_runtime = Some(create_runtime);
    opts.create_simulation = Some(create_simulation);
    let service = RealService::new(opts).await.unwrap();
    *ledger_cell.lock().unwrap() = Some(service.ledger().clone());
    Setup {
        ledger: service.ledger().clone(),
        service,
        settings,
        gh,
        probe,
        opened,
        live,
        sims,
        probes,
        _dir: dir,
    }
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(60)).await;
}

fn common() -> CreateOptimizationInput {
    CreateOptimizationInput {
        name: None,
        repo_url: String::new(),
        base_ref: "main".into(),
        demo: None,
        agents: vec![AgentKind::ClaudeCode],
        agents_where: None,
        budget_usd: 50.0,
        time_limit_seconds: 7200,
        plateau_evals: 25,
        connections: None,
        environment_draft_id: None,
    }
}

async fn make_opt(ledger: &Ledger, id: &str, repo: &str) {
    let mut c = LedgerCreate::new(
        repo,
        "main",
        "latency_ms",
        Direction::Minimize,
        0.03,
        10.0,
        600,
        ratchetd::sim::DEFAULT_CONFIG_YAML,
    );
    c.id = Some(OptimizationId::new(id));
    ledger.create_optimization(c).await.unwrap();
}

fn err_fields(e: &ServiceError) -> Vec<String> {
    match e {
        ServiceError::InvalidFields { fields, .. } => {
            fields.iter().map(|f| f.path.clone()).collect()
        }
        _ => vec![],
    }
}

mod github_row {
    use super::*;

    /// signed out by default; the device flow shows a code, then signs in once approved
    #[tokio::test]
    async fn signed_out_by_default_the_device_flow_shows_a_code_then_signs_in_once_approved() {
        let s = setup(Opts::default()).await;
        assert_eq!(
            s.service.github_status().await.unwrap(),
            GithubStatusVm {
                signed_in: false,
                login: None,
                source: None,
                client_id_configured: true,
                flow: None
            }
        );
        s.gh.with(|g| {
            g.polls = [
                DevicePoll::Pending,
                DevicePoll::Ok {
                    token: DEVICE_TOKEN.into(),
                },
            ]
            .into()
        });
        let st = s.service.github_sign_in().await.unwrap();
        let flow = st.flow.unwrap();
        assert_eq!(
            (flow.state, flow.user_code.as_str()),
            (GithubFlowState::Pending, "ABCD-1234")
        );
        assert_eq!(
            *s.opened.lock().unwrap(),
            ["https://github.com/login/device"]
        );
        let mut events = s.service.subscribe();
        settle().await;
        assert_eq!(
            s.service.github_status().await.unwrap(),
            GithubStatusVm {
                signed_in: true,
                login: Some("octocat".into()),
                source: Some(GithubSource::Device),
                client_id_configured: true,
                flow: None
            }
        );
        assert_eq!(
            s.service.connector_store().github_token(None).as_deref(),
            Some(DEVICE_TOKEN)
        );
        assert!(
            s.service.product().invalidations() > 0,
            "runtimes built before the sign-in are rebuilt"
        );
        assert!(s.service.settings().await.unwrap().secrets.github_token);
        let mut saw = false;
        while let Ok(e) = events.try_recv() {
            saw |= e == ApiEvent::Github { signed_in: true };
        }
        assert!(saw, "a github event is published");
    }

    /// a denied sign-in is reported with its reason, and sign out clears the token and the account
    #[tokio::test]
    async fn a_denied_sign_in_is_reported_and_sign_out_clears_the_token_and_the_account() {
        let s = setup(Opts::default()).await;
        s.gh.with(|g| g.polls = [DevicePoll::Denied].into());
        s.service.github_sign_in().await.unwrap();
        settle().await;
        let flow = s.service.github_status().await.unwrap().flow.unwrap();
        assert_eq!(flow.state, GithubFlowState::Denied);
        assert!(flow.message.unwrap().contains("denied"));
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        let st = s.service.github_status().await.unwrap();
        assert_eq!(
            (st.signed_in, st.login.as_deref(), st.source),
            (true, Some("octocat"), Some(GithubSource::Token))
        );
        let out = s.service.github_sign_out().await.unwrap();
        assert_eq!((out.signed_in, out.login, out.flow), (false, None, None));
        assert!(!s.service.settings().await.unwrap().secrets.github_token);
        assert!(s.service.connector_store().snapshot().is_empty());
    }

    /// a pasted token (Advanced) is proven against GitHub first, and a rejected one is not kept
    #[tokio::test]
    async fn a_pasted_token_is_proven_against_github_first_and_a_rejected_one_is_not_kept() {
        let s = setup(Opts::default()).await;
        s.gh.with(|g| g.user_fails = true);
        let e = s
            .service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("rejected the sign-in"), "{e}");
        assert!(!s.service.settings().await.unwrap().secrets.github_token);
        s.gh.with(|g| g.user_fails = false);
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        assert_eq!(
            s.service.github_status().await.unwrap().source,
            Some(GithubSource::Token)
        );
        s.service
            .clear_secret(SecretName::GithubToken)
            .await
            .unwrap();
        assert!(s.service.connector_store().snapshot().is_empty());
    }

    /// a token from the environment counts as signed in; its login is looked up once
    #[tokio::test]
    async fn a_token_from_the_environment_counts_as_signed_in_its_login_is_looked_up_once() {
        let s = setup(Opts {
            env: [("GITHUB_TOKEN".to_owned(), GH_TOKEN.to_owned())].into(),
            ..Default::default()
        })
        .await;
        let st = s.service.github_status().await.unwrap();
        assert_eq!(
            (st.signed_in, st.login.as_deref(), st.source),
            (true, Some("octocat"), Some(GithubSource::Env))
        );
        s.service.github_status().await.unwrap();
        assert_eq!(
            s.gh.with(|g| g.tokens_seen.iter().filter(|t| *t == GH_TOKEN).count()),
            1
        );
    }

    /// "Test connection" for GitHub is a read of the account
    #[tokio::test]
    async fn test_connection_for_github_is_a_read_of_the_account() {
        let s = setup(Opts::default()).await;
        let r = s
            .service
            .test_connection(ConnectionTarget::Github)
            .await
            .unwrap();
        assert_eq!(
            (r.ok, r.detail.as_str()),
            (false, "Sign in with GitHub first.")
        );
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        assert_eq!(
            s.service
                .test_connection(ConnectionTarget::Github)
                .await
                .unwrap(),
            ConnectionResultVm {
                target: ConnectionTarget::Github,
                ok: true,
                detail: "signed in as octocat".into()
            }
        );
    }
}

mod repo_picker {
    use super::*;

    /// lists the account repositories (needs the sign-in) and searches them
    #[tokio::test]
    async fn lists_the_account_repositories_needs_the_sign_in_and_searches_them() {
        let s = setup(Opts::default()).await;
        let e = s.service.list_repos(None, None).await.unwrap_err();
        assert!(e.to_string().contains("Sign in with GitHub"), "{e}");
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        let names = |v: Vec<RepoVm>| v.into_iter().map(|r| r.full_name).collect::<Vec<_>>();
        assert_eq!(
            names(s.service.list_repos(None, None).await.unwrap()),
            ["acme/api", "acme/web"]
        );
        assert_eq!(
            names(
                s.service
                    .list_repos(Some("web".into()), None)
                    .await
                    .unwrap()
            ),
            ["acme/web"]
        );
    }

    /// reads ratchet.yaml from the default branch and summarizes what the repo declares
    #[tokio::test]
    async fn reads_ratchet_yaml_from_the_default_branch_and_summarizes_what_the_repo_declares() {
        let s = setup(Opts::default()).await;
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        s.gh.with(|g| {
            g.files
                .insert("acme/web@trunk:ratchet.yaml".into(), YAML_OK.into())
        });
        let r = s
            .service
            .inspect_repo(
                RepoSource::Github {
                    repo: "acme/web".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            (r.repo.as_deref(), r.base_ref.as_str(), r.problem.clone()),
            (Some("acme/web"), "trunk", None)
        );
        assert!(r.errors.is_empty() && r.templates.is_empty());
        let c = r.config.clone().unwrap();
        assert_eq!(c.name.as_deref(), Some("Checkout latency"));
        assert_eq!(c.metric, "http_req_duration.p95");
        assert_eq!(c.direction, Direction::Minimize);
        // the protected set is the automatic one plus the repo's own list, with a reason each
        let globs: Vec<_> = c
            .protected_entries
            .iter()
            .map(|e| e.glob.as_str())
            .collect();
        for want in [
            "ratchet.yaml",
            "bench/**",
            ".devcontainer/Dockerfile",
            "docs/**",
        ] {
            assert!(globs.contains(&want), "{want} in {globs:?}");
        }
        assert!(c.protected_entries.iter().all(|e| !e.reason.is_empty()));
        assert_eq!(c.protected_globs, ["docs/**"]);
        assert_eq!(r.config_yaml.as_deref(), Some(YAML_OK));
        // the repo's region is remembered for the runtime it will get
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
    }

    /// offers the templates when the repo has no ratchet.yaml, and shows why an invalid one is invalid
    #[tokio::test]
    async fn offers_the_templates_when_the_repo_has_no_ratchet_yaml_and_shows_why_an_invalid_one_is_invalid()
     {
        let s = setup(Opts::default()).await;
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        let none = s
            .service
            .inspect_repo(
                RepoSource::Github {
                    repo: "acme/api".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert!(none.config_yaml.is_none() && none.config.is_none());
        assert_eq!(none.base_ref, "main");
        let ids: Vec<_> = none.templates.iter().map(|t| t.id.as_str()).collect();
        for want in ["http-latency", "cli-cold-start", "forecast-backtest"] {
            assert!(ids.contains(&want), "{want} in {ids:?}");
        }
        assert!(
            none.templates
                .iter()
                .all(|t| t.kind == ratchet_core::enums::TemplateKind::Template)
        );
        s.gh.with(|g| {
            g.files
                .insert("acme/api@main:ratchet.yaml".into(), "version: 3\n".into())
        });
        let bad = s
            .service
            .inspect_repo(
                RepoSource::Github {
                    repo: "acme/api".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert!(bad.config.is_none());
        assert!(!bad.errors.is_empty());
        assert!(bad.templates.is_empty());
        let e = s
            .service
            .inspect_repo(
                RepoSource::Github {
                    repo: "acme/ghost".into(),
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(e, ServiceError::NotFound(_)) && e.to_string().contains("not found"),
            "{e}"
        );
    }

    /// a local folder: its GitHub remote and ratchet.yaml, or the reason it cannot be used
    #[tokio::test]
    async fn a_local_folder_its_github_remote_and_ratchet_yaml_or_the_reason_it_cannot_be_used() {
        let s = setup(Opts::default()).await;
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let st = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&st.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["remote", "add", "origin", "git@github.com:acme/api.git"]);
        std::fs::write(dir.path().join("ratchet.yaml"), YAML_OK).unwrap();
        let ok = s
            .service
            .inspect_repo(
                RepoSource::Local {
                    path: dir.path().display().to_string(),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(ok.repo.as_deref(), Some("acme/api"));
        assert_eq!(ok.problem, None);
        assert_eq!(ok.config.unwrap().metric, "http_req_duration.p95");
        let scratch = tempfile::tempdir().unwrap();
        let bad = s
            .service
            .inspect_repo(
                RepoSource::Local {
                    path: scratch.path().display().to_string(),
                },
                None,
            )
            .await
            .unwrap();
        assert!(bad.repo.is_none() && bad.config.is_none() && bad.templates.is_empty());
        assert!(bad.problem.is_some());
    }
}

mod templates {
    use super::*;

    /// lists the gallery: runnable templates, then the demos
    #[tokio::test]
    async fn lists_the_gallery_runnable_templates_then_the_demos() {
        let s = setup(Opts::default()).await;
        let t = s.service.list_templates(None).await.unwrap();
        let ids = |k: ratchet_core::enums::TemplateKind| {
            t.iter()
                .filter(|x| x.kind == k)
                .map(|x| x.id.clone())
                .collect::<Vec<_>>()
        };
        let templates = ids(ratchet_core::enums::TemplateKind::Template);
        for want in [
            "http-latency",
            "cli-cold-start",
            "test-suite-time",
            "build-size",
            "memory",
            "forecast-backtest",
            "llm-eval",
        ] {
            assert!(
                templates.iter().any(|i| i == want),
                "{want} in {templates:?}"
            );
        }
        assert_eq!(
            ids(ratchet_core::enums::TemplateKind::Demo),
            ["demo-search", "demo-cli", "demo-forecast"]
        );
        let http = t.iter().find(|x| x.id == "http-latency").unwrap();
        assert_eq!(http.holdout.as_deref(), Some("hidden request mix"));
        assert_eq!(http.machine, "c7i.2xlarge");
        assert_eq!(http.objective.metric, "http_req_duration.p95");
    }

    fn proposal(repo: &str) -> ProposeTemplateInput {
        ProposeTemplateInput {
            repo_url: repo.into(),
            base_ref: "main".into(),
            template_id: "http-latency".into(),
            metric: None,
            direction: None,
            value_usd: None,
            overwrite: None,
        }
    }

    /// proposeTemplate opens the pull request through the engine and returns the link
    #[tokio::test]
    async fn propose_template_opens_the_pull_request_through_the_engine_and_returns_the_link() {
        let hook: Hook = Arc::new(|e| {
            *e.propose.lock().unwrap() = Some(Ok(ProposeTemplateResult {
                pr_url: "https://github.com/acme/api/pull/12".into(),
                branch: "ratchet/template-http-latency".into(),
                sha: "abc1234".into(),
                template_id: "http-latency".into(),
                files: vec![
                    "ratchet.yaml".into(),
                    "bench/http-latency.mjs".into(),
                    "RATCHET.md".into(),
                ],
            }));
        });
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        let e = s
            .service
            .propose_template(proposal("acme/api"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("Sign in with GitHub"), "{e}");
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        let r = s
            .service
            .propose_template(proposal("https://github.com/acme/api.git"))
            .await
            .unwrap();
        assert_eq!(r.pr_url, "https://github.com/acme/api/pull/12");
        assert_eq!(
            r.files,
            ["ratchet.yaml", "bench/http-latency.mjs", "RATCHET.md"]
        );
        // the runtime of the repo got owner/name resolved to the runtime's remote
        let seen = s.live.lock().unwrap()[0].proposals.lock().unwrap()[0].clone();
        assert_eq!(
            (
                seen.repo_url.as_str(),
                seen.base_ref.as_str(),
                seen.template_id.as_str()
            ),
            ("https://github.com/acme/api.git", "main", "http-latency")
        );
        let e = s
            .service
            .propose_template(proposal("/tmp/x"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("not a GitHub repository"), "{e}");
    }

    /// surfaces the engine refusing (files already exist)
    #[tokio::test]
    async fn surfaces_the_engine_refusing_files_already_exist() {
        let hook: Hook = Arc::new(|e| {
            *e.propose.lock().unwrap() = Some(Err(PortError::Failed(
                "ratchet.yaml already exists on main".into(),
            )));
        });
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        let e = s
            .service
            .propose_template(proposal("acme/api"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("already exists"), "{e}");
    }
}

mod creating {
    use super::*;

    /// a real run uses the owner/name and the chosen place for the agents; what is missing stops it before anything is created
    #[tokio::test]
    async fn a_real_run_uses_the_owner_name_and_the_chosen_place_for_the_agents() {
        let hook: Hook = Arc::new(|e| *e.create_id.lock().unwrap() = Some("opt_x".into()));
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        // the images are looked up (by the stub resolver) only when AWS is set up; a profile makes that true on any machine
        s.service.set_aws_profile("default").await.unwrap();
        let mut i = common();
        i.repo_url = "https://github.com/acme/api.git".into();
        i.agents_where = Some(AgentsWhere::Cloud);
        assert_eq!(s.service.create_optimization(i).await.unwrap().id, "opt_x");
        let seen = s.live.lock().unwrap()[0].created.lock().unwrap()[0].clone();
        // the runtime is scoped to the repo; its remote is what the ledger records
        assert_eq!(seen.repo_url, "https://github.com/acme/api.git");
        assert_eq!(
            (
                seen.base_ref.as_str(),
                seen.agents_where,
                seen.budget_usd,
                seen.time_limit_seconds
            ),
            ("main", Some(AgentsWhere::Cloud), 50.0, 7200)
        );

        let blocked = setup(Opts {
            missing: vec!["set the GitHub repository and token (Settings)".into()],
            ..Default::default()
        })
        .await;
        blocked
            .service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        let mut j = common();
        j.repo_url = "acme/api".into();
        let e = blocked.service.create_optimization(j).await.unwrap_err();
        assert!(matches!(e, ServiceError::NotReady(_)));
        assert!(
            e.to_string().starts_with("Not ready: Sign in with GitHub"),
            "{e}"
        );
    }

    /// a demo goes to the simulation runtime and needs no repository
    #[tokio::test]
    async fn a_demo_goes_to_the_simulation_runtime_and_needs_no_repository() {
        let s = setup(Opts::default()).await;
        let mut i = common();
        i.demo = Some(DemoId::Forecast);
        let created = s.service.create_optimization(i).await.unwrap();
        let sims = s.sims.lock().unwrap();
        assert_eq!(sims[0].0, "forecast");
        let seen = sims[0].1.created.lock().unwrap()[0].clone();
        assert_eq!(seen.name.as_deref(), Some("SKU demand forecast (demo)"));
        assert_eq!(seen.base_ref, "main");
        assert_eq!(seen.repo_url, "/work/forecast/remote.git");
        assert!(created.id.starts_with("opt"), "{}", created.id);
    }

    #[tokio::test]
    async fn invalid_requests_name_the_fields() {
        let s = setup(Opts::default()).await;
        let mut i = common();
        i.agents.clear();
        i.budget_usd = 0.0;
        i.repo_url = "acme/api".into();
        let e = s.service.create_optimization(i).await.unwrap_err();
        assert_eq!(err_fields(&e), ["agents", "budgetUsd"]);
        let mut j = common();
        j.repo_url = "/home/me/project".into();
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        let e = s.service.create_optimization(j).await.unwrap_err();
        assert!(e.to_string().contains("not a GitHub repository"), "{e}");
    }
}

mod preflight {
    use super::*;

    fn input(agents: &[AgentKind], w: AgentsWhere, source: PreflightSource) -> PreflightInput {
        PreflightInput {
            repo: None,
            agents: agents.to_vec(),
            agents_where: w,
            source,
        }
    }
    fn failing(items: &[PreflightItem]) -> Vec<&str> {
        items
            .iter()
            .filter(|i| i.state == PreflightState::Fail)
            .map(|i| i.id.as_str())
            .collect()
    }

    /// a demo needs nothing
    #[tokio::test]
    async fn a_demo_needs_nothing() {
        let s = setup(Opts::default()).await;
        let r = s
            .service
            .preflight(input(
                &[AgentKind::ClaudeCode],
                AgentsWhere::Local,
                PreflightSource::Demo,
            ))
            .await
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].id.as_str(), r[0].state), ("demo", PreflightState::Ok));
    }

    /// a real run names each missing thing, then turns green as it is fixed
    #[tokio::test]
    async fn a_real_run_names_each_missing_thing_then_turns_green_as_it_is_fixed() {
        let s = setup(Opts::default()).await; // claude signed in, codex not installed
        let before = s
            .service
            .preflight(input(
                &[AgentKind::ClaudeCode, AgentKind::Codex],
                AgentsWhere::Local,
                PreflightSource::Github,
            ))
            .await
            .unwrap();
        assert_eq!(failing(&before), ["github", "aws", "agent-codex"]);
        assert!(
            before
                .iter()
                .find(|i| i.id == "agent-codex")
                .unwrap()
                .detail
                .contains("npm i -g @openai/codex")
        );
        let c = before.iter().find(|i| i.id == "agent-claude-code").unwrap();
        assert_eq!(c.state, PreflightState::Ok);
        assert!(c.detail.contains("2.1.4"));
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        s.service.set_aws_profile("default").await.unwrap();
        let after = s
            .service
            .preflight(input(
                &[AgentKind::ClaudeCode],
                AgentsWhere::Local,
                PreflightSource::Local,
            ))
            .await
            .unwrap();
        assert!(
            after.iter().all(|i| i.state == PreflightState::Ok),
            "{after:?}"
        );
        assert_eq!(
            after.iter().find(|i| i.id == "github").unwrap().detail,
            "signed in as octocat"
        );
        assert_eq!(
            after.iter().find(|i| i.id == "aws").unwrap().detail,
            "default \u{b7} us-east-1"
        );
    }

    /// a signed-out harness is missing unless an API key is in the environment (which detection already reports)
    #[tokio::test]
    async fn a_signed_out_harness_is_missing_unless_an_api_key_is_in_the_environment() {
        let s = setup(Opts {
            harnesses: Some(vec![claude(|h| {
                h.auth = DAuth::SignedOut;
                h.hint = "Run `claude` and sign in".into();
            })]),
            ..Default::default()
        })
        .await;
        let r = s
            .service
            .preflight(input(
                &[AgentKind::ClaudeCode],
                AgentsWhere::Local,
                PreflightSource::Github,
            ))
            .await
            .unwrap();
        let c = r.iter().find(|i| i.id == "agent-claude-code").unwrap();
        assert_eq!(
            (c.state, c.detail.as_str()),
            (PreflightState::Fail, "Run `claude` and sign in")
        );
        let keyed = setup(Opts {
            harnesses: Some(vec![claude(|h| h.auth = DAuth::ApiKey)]),
            ..Default::default()
        })
        .await;
        let r = keyed
            .service
            .preflight(input(
                &[AgentKind::ClaudeCode],
                AgentsWhere::Local,
                PreflightSource::Github,
            ))
            .await
            .unwrap();
        let c = r.iter().find(|i| i.id == "agent-claude-code").unwrap();
        assert_eq!(c.state, PreflightState::Ok);
        assert!(c.detail.contains("API key"));
    }

    /// cloud runners need API keys, not a local login
    #[tokio::test]
    async fn cloud_runners_need_api_keys_not_a_local_login() {
        let s = setup(Opts::default()).await;
        let r = s
            .service
            .preflight(input(
                &[AgentKind::ClaudeCode, AgentKind::Codex],
                AgentsWhere::Cloud,
                PreflightSource::Github,
            ))
            .await
            .unwrap();
        assert_eq!(
            failing(&r),
            ["github", "aws", "key-claude-code", "key-codex"]
        );
        s.service
            .set_secret(SecretName::AnthropicApiKey, ANTHROPIC_KEY)
            .await
            .unwrap();
        let r = s
            .service
            .preflight(input(
                &[AgentKind::ClaudeCode],
                AgentsWhere::Cloud,
                PreflightSource::Github,
            ))
            .await
            .unwrap();
        assert_eq!(
            r.iter().find(|i| i.id == "key-claude-code").unwrap().state,
            PreflightState::Ok
        );
    }
}

mod aws_and_agents {
    use super::*;

    /// lists profiles by name, remembers the choice and the kind of sign-in, defaults the region
    #[tokio::test]
    async fn lists_profiles_by_name_remembers_the_choice_and_the_kind_of_sign_in_defaults_the_region()
     {
        let s = setup(Opts::default()).await;
        let before = s.service.aws_profiles().await.unwrap();
        assert_eq!(
            (
                before.selected.as_str(),
                before.region.as_str(),
                before.default_region.as_str(),
                before.files_found,
                before.sso_sessions.clone()
            ),
            ("", "us-east-1", "us-east-1", true, vec!["corp".to_owned()])
        );
        assert_eq!(
            before
                .profiles
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["default", "corp-sso"]
        );
        let st = s.service.set_aws_profile("corp-sso").await.unwrap();
        assert_eq!(st.aws_profile, "corp-sso");
        assert_eq!(s.settings.live().aws_auth, AwsAuthKind::Sso);
        assert!(s.service.product().invalidations() > 0);
        assert_eq!(s.service.aws_profiles().await.unwrap().selected, "corp-sso");
        let saved = s
            .service
            .save_advanced_settings(LiveSettingsInput {
                aws_region: Some("eu-west-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(saved.aws_region, "eu-west-1");
        let e = s
            .service
            .save_advanced_settings(LiveSettingsInput {
                aws_region: Some("mars".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(err_fields(&e), ["awsRegion"]);
    }

    /// "Test connection" passes the profile and region, adds the SSO hint, and never invents a profile
    #[tokio::test]
    async fn test_connection_passes_the_profile_and_region_adds_the_sso_hint_and_never_invents_a_profile()
     {
        let s = setup(Opts::default()).await;
        let r = s
            .service
            .test_connection(ConnectionTarget::Aws)
            .await
            .unwrap();
        assert_eq!(
            (r.ok, r.detail.as_str()),
            (false, "Choose an AWS profile first.")
        );
        s.service.set_aws_profile("corp-sso").await.unwrap();
        s.service
            .save_advanced_settings(LiveSettingsInput {
                aws_region: Some("eu-west-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            s.service
                .test_connection(ConnectionTarget::Aws)
                .await
                .unwrap()
                .ok
        );
        assert_eq!(
            s.probe.calls.lock().unwrap().last().unwrap(),
            &("aws".to_owned(), "eu-west-1/corp-sso".to_owned())
        );
        *s.probe.fail_with.lock().unwrap() =
            Some("The SSO session associated with this profile has expired".into());
        let r = s
            .service
            .test_connection(ConnectionTarget::Aws)
            .await
            .unwrap();
        assert!(
            r.detail.contains("aws sso login --profile corp-sso"),
            "{}",
            r.detail
        );
    }

    /// detects the harnesses once and again on demand
    #[tokio::test]
    async fn detects_the_harnesses_once_and_again_on_demand() {
        let s = setup(Opts::default()).await;
        let kinds = |v: Vec<HarnessVm>| {
            v.into_iter()
                .map(|h| (h.kind, h.installed))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(s.service.harnesses(false).await.unwrap()),
            [(HarnessKind::ClaudeCode, true), (HarnessKind::Codex, false)]
        );
        s.service.harnesses(false).await.unwrap();
        assert_eq!(s.probes.load(Ordering::SeqCst), 1);
        let before = s.service.product().invalidations();
        s.service.harnesses(true).await.unwrap();
        assert_eq!(s.probes.load(Ordering::SeqCst), 2);
        assert!(
            s.service.product().invalidations() > before,
            "a re-detect rebuilds idle runtimes"
        );
    }

    /// the developer overrides are validated and kept apart
    #[tokio::test]
    async fn the_developer_overrides_are_validated_and_kept_apart() {
        let s = setup(Opts::default()).await;
        let e = s
            .service
            .save_advanced_settings(LiveSettingsInput {
                bench_image_ref: Some("not-an-ami".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(err_fields(&e), ["benchImageRef"]);
        let e = s
            .service
            .save_advanced_settings(LiveSettingsInput {
                tofu_binary: Some("tofu; rm -rf /".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(err_fields(&e), ["tofuBinary"]);
        let st = s
            .service
            .save_advanced_settings(LiveSettingsInput {
                module_dir: Some("/srv/infra/aws".into()),
                bench_image_ref: Some("ami-0123456789abcdef0".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            (
                st.advanced.values.module_dir.as_str(),
                st.advanced.values.bench_image_ref.as_str()
            ),
            ("/srv/infra/aws", "ami-0123456789abcdef0")
        );
        assert_eq!(s.settings.live().bench_image_ref, "ami-0123456789abcdef0");
        assert_eq!(st.advanced.defaults.tofu_binary, "tofu");
    }
}

mod environments {
    use super::*;

    async fn record_sandbox(ledger: &Ledger) {
        let mut r = ratchet_core::ledger::RecordSandboxInput::new(
            "ratchet-sandbox",
            SandboxProviderKind::Aws,
            "eu-west-2",
        );
        r.status = Some(SandboxStatus::Applied);
        ledger.record_sandbox(r).await.unwrap();
    }

    /// shows the running environment with phase, region, cost and when it removes itself
    #[tokio::test]
    async fn shows_the_running_environment_with_phase_region_cost_and_when_it_removes_itself() {
        let hook: Hook = Arc::new(|e| {
            *e.activity.lock().unwrap() = Some(activity(|a| {
                a.active_runs = vec![OptimizationId::new("opt_demo_search")];
                a.destroy_at = None;
            }));
        });
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        record_sandbox(&s.ledger).await;
        let e = s.service.environments().await.unwrap();
        assert_eq!(e.idle_minutes, 30);
        assert_eq!(e.environments.len(), 1);
        let v = &e.environments[0];
        assert_eq!(
            (v.sandbox_id.as_str(), v.region.as_deref(), v.phase),
            (
                "ratchet-sandbox",
                Some("eu-west-2"),
                EnvironmentPhase::Ready
            )
        );
        assert_eq!(
            (
                v.active_runs.clone(),
                v.instances_running,
                v.spend_usd,
                v.can_end,
                v.simulated
            ),
            (
                vec!["opt_demo_search".to_owned()],
                Some(2),
                1.5,
                false,
                false
            )
        );
        let calls = s.live.lock().unwrap()[0].calls();
        assert!(!calls.iter().any(|c| c.starts_with("end:")));
    }

    /// idle shows the destroy time; ending it now asks the engine and returns the new picture with the receipt
    #[tokio::test]
    async fn idle_shows_the_destroy_time_ending_it_now_asks_the_engine_and_returns_the_new_picture_with_the_receipt()
     {
        let r = receipt(TeardownTrigger::Manual);
        let r2 = r.clone();
        let hook: Hook = Arc::new(move |e| {
            *e.activity.lock().unwrap() = Some(activity(|_| {}));
            *e.end_receipt.lock().unwrap() = Some(r2.clone());
            e.destroy_on_end.store(true, Ordering::SeqCst);
            *e.activity_after_end.lock().unwrap() = Some(activity(|a| {
                a.status = SandboxStateStatus::Destroyed;
                a.last_receipt = Some(r2.clone());
            }));
        });
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        record_sandbox(&s.ledger).await;
        let idle = s
            .service
            .environments()
            .await
            .unwrap()
            .environments
            .remove(0);
        assert_eq!(
            (idle.phase, idle.destroy_at, idle.can_end),
            (EnvironmentPhase::Idle, Some(1_805_000), true)
        );
        let after = s.service.end_environment("ratchet-sandbox").await.unwrap();
        assert!(after.environments.is_empty());
        let l = after.last_receipt.unwrap();
        assert_eq!(
            (
                l.sandbox_id.as_str(),
                l.trigger,
                l.nothing_running,
                l.spend_usd
            ),
            ("ratchet-sandbox", ReceiptTrigger::Manual, true, 4.2)
        );
        assert_eq!(
            (
                l.removed.instances,
                l.removed.agent_identities,
                l.removed.secrets
            ),
            (2, 1, 1)
        );
        assert_eq!(
            l.spend_by_category
                .iter()
                .map(|c| (c.label.as_str(), c.usd))
                .collect::<Vec<_>>(),
            [("Bench runner", 1.2), ("Model tokens", 3.0)]
        );
    }

    /// follows the sandbox events for the phase it shows
    #[tokio::test]
    async fn follows_the_sandbox_events_for_the_phase_it_shows() {
        let hook: Hook = Arc::new(|e| *e.activity.lock().unwrap() = Some(activity(|_| {})));
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        record_sandbox(&s.ledger).await;
        s.service.environments().await.unwrap(); // builds the runtime that will emit
        s.live.lock().unwrap()[0].emit(EngineEvent::Sandbox {
            sandbox_id: "ratchet-sandbox".into(),
            phase: SandboxPhase::Destroying,
            region: Some("eu-west-2".into()),
            text: "removing the environment".into(),
        });
        settle().await;
        let v = s
            .service
            .environments()
            .await
            .unwrap()
            .environments
            .remove(0);
        assert_eq!(
            (v.phase, v.text.as_str()),
            (EnvironmentPhase::Destroying, "removing the environment")
        );
    }

    /// Result gets the receipt of the run and the calibration in plain words
    #[tokio::test]
    async fn result_gets_the_receipt_of_the_run_and_the_calibration_in_plain_words() {
        let cal = ratchet_core::bench::calibrate::calibrate(&ratchet_core::CalibrationInput {
            samples: &[100.0, 109.0, 91.0, 104.0, 96.0],
            repeats: None,
            min_gain: None,
            noise_factor: None,
        })
        .unwrap();
        let (c2, r2) = (cal.clone(), receipt(TeardownTrigger::Idle));
        let hook: Hook = Arc::new(move |e| {
            *e.calibration.lock().unwrap() = Some(c2.clone());
            e.receipts
                .lock()
                .unwrap()
                .insert("opt_demo_ingest".into(), r2.clone());
        });
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        make_opt(&s.ledger, "opt_demo_ingest", "acme/api").await;
        make_opt(&s.ledger, "opt_demo_search", "acme/api").await;
        let d = s.service.get_optimization("opt_demo_ingest").await.unwrap();
        let rc = d.receipt.unwrap();
        assert_eq!(rc.summary, "Removed 2 instances; nothing is left running.");
        assert!(rc.nothing_running);
        assert_eq!(rc.destroyed_at, 1_793_361_600_000);
        assert_eq!(d.calibration.as_deref(), Some(cal.explanation.as_str()));
        assert!(
            s.service
                .get_optimization("opt_demo_search")
                .await
                .unwrap()
                .receipt
                .is_none()
        );
    }

    /// an engine that cannot say (no sandbox support) yields an empty, honest view
    #[tokio::test]
    async fn an_empty_ledger_yields_an_empty_honest_view_and_ending_an_unknown_environment_is_a_404()
     {
        let s = setup(Opts::default()).await;
        let e = s.service.environments().await.unwrap();
        assert!(e.environments.is_empty() && e.last_receipt.is_none());
        let err = s
            .service
            .end_environment("ratchet-sandbox")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)), "{err}");
    }
}

mod secrets_never_leave {
    use super::*;

    /// no response, event or error carries a stored key or token
    #[tokio::test]
    async fn no_response_event_or_error_carries_a_stored_key_or_token() {
        let s = setup(Opts::default()).await;
        let mut events = s.service.subscribe();
        let mut seen: Vec<String> = vec![];
        let mut dump = |v: &dyn erased::Dump| seen.push(v.dump());
        s.gh.with(|g| {
            g.polls = [DevicePoll::Ok {
                token: DEVICE_TOKEN.into(),
            }]
            .into()
        });
        dump(&s.service.github_sign_in().await.unwrap());
        settle().await;
        dump(&s.service.github_status().await.unwrap());
        dump(
            &s.service
                .set_secret(SecretName::GithubToken, GH_TOKEN)
                .await
                .unwrap(),
        );
        dump(
            &s.service
                .set_secret(SecretName::AnthropicApiKey, ANTHROPIC_KEY)
                .await
                .unwrap(),
        );
        dump(
            &s.service
                .set_secret(SecretName::OpenaiApiKey, OPENAI_KEY)
                .await
                .unwrap(),
        );
        dump(
            &s.service
                .save_advanced_settings(LiveSettingsInput {
                    aws_region: Some("eu-west-1".into()),
                    bench_image_ref: Some("ami-0fedcba9876543210".into()),
                    ..Default::default()
                })
                .await
                .unwrap(),
        );
        dump(&s.service.set_aws_profile("default").await.unwrap());
        dump(&s.service.settings().await.unwrap());
        dump(&s.service.environments().await.unwrap());
        dump(&s.service.list_repos(None, None).await.unwrap());
        dump(&s.service.list_templates(None).await.unwrap());
        dump(&s.service.aws_profiles().await.unwrap());
        dump(&s.service.harnesses(false).await.unwrap());
        dump(&s.service.harnesses(true).await.unwrap());
        dump(
            &s.service
                .list_optimizations(KpiRange::Days30)
                .await
                .unwrap(),
        );
        for t in [
            ConnectionTarget::Aws,
            ConnectionTarget::Github,
            ConnectionTarget::Anthropic,
            ConnectionTarget::Openai,
        ] {
            dump(&s.service.test_connection(t).await.unwrap());
        }
        dump(
            &s.service
                .preflight(PreflightInput {
                    agents: vec![AgentKind::ClaudeCode, AgentKind::Codex],
                    agents_where: AgentsWhere::Local,
                    source: PreflightSource::Github,
                    repo: None,
                })
                .await
                .unwrap(),
        );
        dump(
            &s.service
                .inspect_repo(
                    RepoSource::Github {
                        repo: "acme/api".into(),
                    },
                    None,
                )
                .await
                .unwrap(),
        );
        *s.probe.fail_with.lock().unwrap() = Some(format!(
            "rejected {ANTHROPIC_KEY} and {OPENAI_KEY} and {GH_TOKEN}"
        ));
        for t in [
            ConnectionTarget::Aws,
            ConnectionTarget::Anthropic,
            ConnectionTarget::Openai,
        ] {
            dump(&s.service.test_connection(t).await.unwrap());
        }
        let failing = [
            s.service
                .set_secret(SecretName::GithubToken, &format!("bad {GH_TOKEN}"))
                .await
                .map(|_| ()),
            s.service
                .set_secret(SecretName::AnthropicApiKey, OPENAI_KEY)
                .await
                .map(|_| ()),
            s.service
                .inspect_repo(
                    RepoSource::Github {
                        repo: "acme/ghost".into(),
                    },
                    None,
                )
                .await
                .map(|_| ()),
        ];
        for r in failing {
            let e = r.unwrap_err();
            seen.push(serde_json::to_string(&e.into_api_error()).unwrap());
        }
        while let Ok(e) = events.try_recv() {
            seen.push(serde_json::to_string(&e).unwrap());
        }
        let all = seen.join("\n");
        for secret in SECRETS {
            assert!(!all.contains(secret), "{secret} leaked");
        }
        assert_eq!(
            s.service.settings().await.unwrap().secrets,
            SecretsPresent {
                anthropic_api_key: true,
                openai_api_key: true,
                github_token: true
            }
        );
    }

    mod erased {
        pub trait Dump {
            fn dump(&self) -> String;
        }
        impl<T: serde::Serialize> Dump for T {
            fn dump(&self) -> String {
                serde_json::to_string(self).unwrap()
            }
        }
    }

    /// exposes no method that returns a secret, and no way to apply or destroy infrastructure (no such route)
    #[test]
    fn the_api_has_no_route_that_returns_a_secret_or_applies_infrastructure() {
        let doc: serde_json::Value =
            serde_json::from_str(&ratchetd::openapi::openapi_json()).unwrap();
        let paths: Vec<&str> = doc["paths"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(
            !paths.iter().any(|p| {
                let l = p.to_lowercase();
                l.contains("reveal")
                    || l.contains("getsecret")
                    || l.contains("apply")
                    || l.contains("teardown") && l.contains("sandbox")
            }),
            "{paths:?}"
        );
        // a secret route only accepts a value (PUT) or clears it (DELETE); nothing reads it back
        let secret = doc["paths"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(p, _)| p.contains("secret"))
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(secret.get("get").is_none(), "{secret}");
    }
}

mod connectors {
    use ratchet_connectors::SecretName as ConnectorSecret;

    use super::*;

    const ORG: &str = "github_pat_ORGTOKENORGTOKENORGTOKEN12";

    fn aws(profile: &str) -> AddAwsConnectorInput {
        AddAwsConnectorInput {
            profile: profile.into(),
            region: None,
            monthly_cap_usd: None,
            name: None,
        }
    }

    fn org_token(name: &str) -> AddGithubConnectorInput {
        AddGithubConnectorInput {
            token: ORG.into(),
            name: Some(name.into()),
        }
    }

    fn id_of(v: &ConnectorsVm, name: &str) -> String {
        v.connectors
            .iter()
            .find(|c| c.name() == name)
            .unwrap_or_else(|| panic!("no connector named {name}"))
            .id()
            .to_owned()
    }

    /// connectors › lists detected logins with the saved accounts, grouped by kind, with one default per kind
    #[tokio::test]
    async fn lists_detected_logins_with_the_saved_accounts_grouped_by_kind() {
        let s = setup(Opts::default()).await;
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        s.gh.with(|g| g.login = "acme-org".into());
        s.service
            .add_github_connector(org_token("acme-org"))
            .await
            .unwrap();
        s.service
            .add_aws_connector(AddAwsConnectorInput {
                region: Some("us-west-2".into()),
                monthly_cap_usd: Some(100.0),
                ..aws("default")
            })
            .await
            .unwrap();
        let vm = s.service.list_connectors().await.unwrap();
        assert!(vm.encryption_available && vm.client_id_configured && vm.github_flow.is_none());
        let rows: Vec<_> = vm
            .connectors
            .iter()
            .map(|c| (c.kind(), c.name().to_owned(), c.is_default(), c.status()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    ConnectorKind::Github,
                    "octocat".to_owned(),
                    true,
                    ConnectorStatus::Connected
                ),
                (
                    ConnectorKind::Github,
                    "acme-org".to_owned(),
                    false,
                    ConnectorStatus::Connected
                ),
                (
                    ConnectorKind::Aws,
                    "default".to_owned(),
                    true,
                    ConnectorStatus::Connected
                ),
                (
                    ConnectorKind::Agent,
                    "Claude Code \u{00b7} your login".to_owned(),
                    true,
                    ConnectorStatus::Connected
                ),
            ]
        );
        let json = serde_json::to_string(&vm).unwrap();
        assert!(!json.contains(GH_TOKEN) && !json.contains(ORG), "{json}");
    }

    /// connectors › every change rebuilds what depends on the accounts
    #[tokio::test]
    async fn every_change_rebuilds_what_depends_on_the_accounts() {
        let s = setup(Opts::default()).await;
        let vm = s.service.add_aws_connector(aws("default")).await.unwrap();
        let id = id_of(&vm, "default");
        let before = s.service.product().invalidations();
        s.service
            .update_connector(
                &id,
                UpdateConnectorInput {
                    name: Some("prod".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        s.service.set_default_connector(&id).await.unwrap();
        s.service.remove_connector(&id).await.unwrap();
        assert_eq!(s.service.product().invalidations() - before, 3);
    }

    /// connectors › adding an AWS connector only offers profiles that exist in ~/.aws, and checks the profile
    #[tokio::test]
    async fn adding_an_aws_connector_only_offers_profiles_that_exist_and_checks_the_profile() {
        let s = setup(Opts::default()).await;
        let e = s.service.add_aws_connector(aws("nope")).await.unwrap_err();
        assert!(e.to_string().contains("not in ~/.aws"), "{e}");
        let vm = s
            .service
            .add_aws_connector(AddAwsConnectorInput {
                region: Some("eu-west-1".into()),
                ..aws("corp-sso")
            })
            .await
            .unwrap();
        settle().await; // the probe runs in the background
        assert_eq!(
            s.probe.calls.lock().unwrap().last().cloned(),
            Some(("aws".to_owned(), "eu-west-1/corp-sso".to_owned()))
        );
        let c = &vm.connectors[0];
        assert_eq!(
            (c.kind(), c.status(), c.detail()),
            (
                ConnectorKind::Aws,
                ConnectorStatus::Connected,
                "SSO \u{00b7} eu-west-1"
            )
        );
        *s.probe.fail_with.lock().unwrap() = Some("token expired".into());
        let t = s.service.test_connector(c.id()).await.unwrap();
        assert_eq!(t.connector.unwrap().status(), ConnectorStatus::Expired);
        let e = s
            .service
            .add_aws_connector(aws("corp-sso"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("already connected"), "{e}");
    }

    /// connectors › the device flow adds an account; signing in again with a connector id replaces that account token
    #[tokio::test]
    async fn the_device_flow_adds_an_account_and_signing_in_again_replaces_its_token() {
        let s = setup(Opts::default()).await;
        let started = s
            .service
            .start_github_connector_sign_in(StartGithubSignInInput::default())
            .await
            .unwrap();
        let flow = started.github_flow.unwrap();
        assert_eq!(
            (flow.state, flow.user_code.as_str()),
            (GithubFlowState::Pending, "ABCD-1234")
        );
        assert!(
            s.service
                .cancel_github_connector_sign_in()
                .await
                .unwrap()
                .github_flow
                .is_none()
        );
        s.gh.with(|g| {
            g.polls = [DevicePoll::Ok {
                token: DEVICE_TOKEN.into(),
            }]
            .into()
        });
        s.service
            .start_github_connector_sign_in(StartGithubSignInInput::default())
            .await
            .unwrap();
        settle().await;
        let done = s.service.list_connectors().await.unwrap();
        assert!(done.github_flow.is_none());
        let account = &done.connectors[0];
        match account {
            ConnectorVm::Github { login, source, .. } => {
                assert_eq!(login.as_deref(), Some("octocat"));
                assert_eq!(*source, GithubConnectorSource::Device);
            }
            other => panic!("{other:?}"),
        }
        let refreshed = "gho_REFRESHEDTOKENREFRESHED123456";
        s.gh.with(|g| {
            g.polls = [DevicePoll::Ok {
                token: refreshed.into(),
            }]
            .into()
        });
        s.service
            .start_github_connector_sign_in(StartGithubSignInInput {
                connector_id: Some(account.id().to_owned()),
            })
            .await
            .unwrap();
        settle().await;
        let list = s.service.list_connectors().await.unwrap();
        assert_eq!(
            list.connectors
                .iter()
                .filter(|c| c.kind() == ConnectorKind::Github)
                .count(),
            1
        );
        assert_eq!(
            s.service
                .connector_store()
                .github_token(Some(account.id()))
                .as_deref(),
            Some(refreshed)
        );
        let e = s
            .service
            .start_github_connector_sign_in(StartGithubSignInInput {
                connector_id: Some("ghost".into()),
            })
            .await
            .unwrap_err();
        assert!(e.to_string().contains("no longer exists"), "{e}");
    }

    /// connectors › rename, default and remove; a login that is removed can be added back
    #[tokio::test]
    async fn rename_default_and_remove_a_login_that_is_removed_can_be_added_back() {
        let s = setup(Opts::default()).await;
        let vm = s
            .service
            .add_github_connector(AddGithubConnectorInput {
                token: GH_TOKEN.into(),
                name: None,
            })
            .await
            .unwrap();
        s.gh.with(|g| g.login = "acme-org".into());
        let second = s
            .service
            .add_github_connector(org_token("acme-org"))
            .await
            .unwrap();
        let org = id_of(&second, "acme-org");
        s.service
            .update_connector(
                &org,
                UpdateConnectorInput {
                    name: Some("Acme organisation".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let v = s.service.set_default_connector(&org).await.unwrap();
        let default_github = v
            .connectors
            .iter()
            .find(|c| c.is_default() && c.kind() == ConnectorKind::Github)
            .map(|c| c.name().to_owned());
        assert_eq!(default_github.as_deref(), Some("Acme organisation"));
        let e = s
            .service
            .update_connector(
                vm.connectors[0].id(),
                UpdateConnectorInput {
                    name: Some("Acme organisation".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(e.to_string().contains("already a connector named"), "{e}");
        let left = s.service.remove_connector(&org).await.unwrap();
        let github: Vec<_> = left
            .connectors
            .iter()
            .filter(|c| c.kind() == ConnectorKind::Github)
            .map(|c| (c.name().to_owned(), c.is_default()))
            .collect();
        assert_eq!(github, [("octocat".to_owned(), true)]);
        let login_id = left
            .connectors
            .iter()
            .find(|c| c.kind() == ConnectorKind::Agent)
            .unwrap()
            .id()
            .to_owned();
        let none = s.service.remove_connector(&login_id).await.unwrap();
        assert!(
            !none
                .connectors
                .iter()
                .any(|c| c.kind() == ConnectorKind::Agent)
        );
        let back = s
            .service
            .add_agent_connector(AddAgentConnectorInput::Login {
                agent: ConnectorAgent::ClaudeCode,
            })
            .await
            .unwrap();
        assert!(
            back.connectors
                .iter()
                .any(|c| c.kind() == ConnectorKind::Agent)
        );
    }

    /// connectors › remembers the choice per repo and rebuilds that repo runtime
    #[tokio::test]
    async fn remembers_the_choice_per_repo_and_rebuilds_that_repo_runtime() {
        let s = setup(Opts::default()).await;
        s.service
            .add_github_connector(AddGithubConnectorInput {
                token: GH_TOKEN.into(),
                name: None,
            })
            .await
            .unwrap();
        s.gh.with(|g| g.login = "acme-org".into());
        s.service
            .add_github_connector(org_token("acme-org"))
            .await
            .unwrap();
        s.service.add_aws_connector(aws("default")).await.unwrap();
        s.service.add_aws_connector(aws("corp-sso")).await.unwrap();
        let all = s.service.list_connectors().await.unwrap();
        let empty = s.service.repo_connections("acme/api").await.unwrap();
        assert_eq!(empty.repo, "acme/api");
        assert!(empty.saved.is_none());
        assert_eq!(empty.effective.github, Some(id_of(&all, "octocat")));
        assert_eq!(empty.effective.aws, Some(id_of(&all, "default")));
        let sel = RepoConnections {
            github: id_of(&all, "acme-org"),
            aws: id_of(&all, "corp-sso"),
            agents: vec![AgentSelection {
                connector_id: "agent-claude-code-login".into(),
                where_: AgentsWhere::Local,
            }],
        };
        let before = s.service.product().invalidations();
        let saved = s
            .service
            .save_repo_connections("acme/api", sel.clone())
            .await
            .unwrap();
        assert_eq!(saved.saved.as_ref(), Some(&sel));
        assert_eq!(s.service.product().invalidations() - before, 1);
        let got = s.service.repo_connections("ACME/API").await.unwrap();
        assert_eq!(
            got.effective,
            EffectiveConnections {
                github: Some(sel.github.clone()),
                aws: Some(sel.aws.clone()),
                agents: sel.agents.clone()
            }
        );
        let ghost = RepoConnections {
            github: "ghost".into(),
            ..sel
        };
        let e = s
            .service
            .save_repo_connections("acme/api", ghost)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("GitHub account"), "{e}");
    }

    /// connectors › createOptimization with connections remembers them and runs the agents where they were chosen
    #[tokio::test]
    async fn create_with_connections_remembers_them_and_runs_the_agents_where_they_were_chosen() {
        let hook: Hook = Arc::new(|e| *e.create_id.lock().unwrap() = Some("opt_x".into()));
        let s = setup(Opts {
            on_live: Some(hook),
            ..Default::default()
        })
        .await;
        s.service
            .add_github_connector(AddGithubConnectorInput {
                token: GH_TOKEN.into(),
                name: None,
            })
            .await
            .unwrap();
        s.service.add_aws_connector(aws("default")).await.unwrap();
        s.service
            .add_agent_connector(AddAgentConnectorInput::ApiKey {
                agent: ConnectorAgent::ClaudeCode,
                api_key: ANTHROPIC_KEY.into(),
                name: None,
            })
            .await
            .unwrap();
        let v = s.service.list_connectors().await.unwrap();
        let (github, aws_id) = (id_of(&v, "octocat"), id_of(&v, "default"));
        let key = id_of(&v, "Claude Code \u{00b7} API key");
        let mut common = common();
        common.repo_url = "https://github.com/acme/api.git".into();
        common.agents = vec![AgentKind::Codex];
        let chosen = RepoConnections {
            github: github.clone(),
            aws: aws_id.clone(),
            agents: vec![AgentSelection {
                connector_id: key.clone(),
                where_: AgentsWhere::Cloud,
            }],
        };
        s.service
            .create_optimization(CreateOptimizationInput {
                connections: Some(chosen.clone()),
                ..common.clone()
            })
            .await
            .unwrap();
        let created = |s: &Setup| -> Vec<_> {
            s.live
                .lock()
                .unwrap()
                .iter()
                .flat_map(|r| r.created.lock().unwrap().clone())
                .collect()
        };
        let seen = created(&s);
        // the connections win over `agents`
        assert_eq!(
            (seen[0].agents.clone(), seen[0].agents_where),
            (vec![AgentKind::ClaudeCode], Some(AgentsWhere::Cloud))
        );
        assert_eq!(
            s.service
                .connector_store()
                .repo_connections("acme/api")
                .saved,
            Some(chosen.clone())
        );
        // mixing places is refused
        let mixed = RepoConnections {
            agents: vec![
                AgentSelection {
                    connector_id: key.clone(),
                    where_: AgentsWhere::Cloud,
                },
                AgentSelection {
                    connector_id: "agent-codex-login".into(),
                    where_: AgentsWhere::Local,
                },
            ],
            ..chosen
        };
        assert!(
            s.service
                .create_optimization(CreateOptimizationInput {
                    connections: Some(mixed),
                    ..common.clone()
                })
                .await
                .is_err()
        );
        // without connections: the remembered choice stays, the request decides the rest
        s.service
            .create_optimization(CreateOptimizationInput {
                agents: vec![AgentKind::ClaudeCode],
                agents_where: Some(AgentsWhere::Local),
                ..common
            })
            .await
            .unwrap();
        let seen = created(&s);
        assert_eq!(
            (seen[1].agents.clone(), seen[1].agents_where),
            (vec![AgentKind::ClaudeCode], Some(AgentsWhere::Local))
        );
    }

    /// connectors › preflight checks the connectors remembered for the repo
    #[tokio::test]
    async fn preflight_checks_the_connectors_remembered_for_the_repo() {
        let s = setup(Opts::default()).await;
        s.service
            .add_github_connector(AddGithubConnectorInput {
                token: GH_TOKEN.into(),
                name: None,
            })
            .await
            .unwrap();
        s.gh.with(|g| g.login = "acme-org".into());
        s.service
            .add_github_connector(org_token("acme-org"))
            .await
            .unwrap();
        s.service.add_aws_connector(aws("default")).await.unwrap();
        s.service
            .add_aws_connector(AddAwsConnectorInput {
                region: Some("eu-west-1".into()),
                ..aws("corp-sso")
            })
            .await
            .unwrap();
        let all = s.service.list_connectors().await.unwrap();
        s.service
            .save_repo_connections(
                "acme/api",
                RepoConnections {
                    github: id_of(&all, "acme-org"),
                    aws: id_of(&all, "corp-sso"),
                    agents: vec![AgentSelection {
                        connector_id: "agent-claude-code-login".into(),
                        where_: AgentsWhere::Local,
                    }],
                },
            )
            .await
            .unwrap();
        let input = |repo: Option<&str>| PreflightInput {
            agents: vec![AgentKind::ClaudeCode],
            agents_where: AgentsWhere::Local,
            source: PreflightSource::Github,
            repo: repo.map(str::to_owned),
        };
        let detail = |items: &[PreflightItem], id: &str| {
            items.iter().find(|i| i.id == id).unwrap().detail.clone()
        };
        let r = s.service.preflight(input(Some("acme/api"))).await.unwrap();
        assert_eq!(detail(&r, "aws"), "corp-sso \u{00b7} eu-west-1");
        assert_eq!(detail(&r, "github"), "signed in as acme-org");
        let r = s.service.preflight(input(None)).await.unwrap();
        assert_eq!(detail(&r, "aws"), "default \u{00b7} us-east-1");
    }

    /// connectors › the old single-account calls still work on top of the connectors
    #[tokio::test]
    async fn the_old_single_account_calls_still_work_on_top_of_the_connectors() {
        let s = setup(Opts::default()).await;
        s.service
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .await
            .unwrap();
        s.service.set_aws_profile("corp-sso").await.unwrap();
        s.service
            .save_advanced_settings(LiveSettingsInput {
                aws_region: Some("eu-west-2".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let st = s.service.settings().await.unwrap();
        assert_eq!(
            (
                st.aws_profile.as_str(),
                st.aws_region.as_str(),
                st.secrets.github_token
            ),
            ("corp-sso", "eu-west-2", true)
        );
        let aws_rows = |s: &Setup| -> Vec<(String, String)> {
            s.service
                .connector_store()
                .snapshot()
                .into_iter()
                .filter_map(|c| match c {
                    ConnectorVm::Aws {
                        profile, region, ..
                    } => Some((profile, region)),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            aws_rows(&s),
            [("corp-sso".to_owned(), "eu-west-2".to_owned())]
        );
        // another profile: added, and the default moves to it
        s.service.set_aws_profile("default").await.unwrap();
        assert_eq!(aws_rows(&s).len(), 2);
        assert_eq!(s.service.settings().await.unwrap().aws_profile, "default");
        s.service.set_aws_profile("").await.unwrap();
        assert_eq!(s.service.settings().await.unwrap().aws_profile, "corp-sso");
        // the keys of the first Settings are connectors too
        s.service
            .set_secret(SecretName::AnthropicApiKey, ANTHROPIC_KEY)
            .await
            .unwrap();
        assert!(
            s.service
                .settings()
                .await
                .unwrap()
                .secrets
                .anthropic_api_key
        );
        assert_eq!(
            s.service
                .connector_store()
                .secret_value(ConnectorSecret::AnthropicApiKey)
                .as_deref(),
            Some(ANTHROPIC_KEY)
        );
        s.service
            .clear_secret(SecretName::AnthropicApiKey)
            .await
            .unwrap();
        assert!(
            !s.service
                .settings()
                .await
                .unwrap()
                .secrets
                .anthropic_api_key
        );
    }

    /// The settings file of the first versions (one token, one profile, one key) becomes connectors at the first start, and the
    /// daemon's single-account calls see them.
    #[tokio::test]
    async fn the_single_account_settings_of_the_first_versions_are_migrated_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsStore::new(
            Some(dir.path().join("settings.json")),
            secrets.clone(),
            HashMap::new(),
        ));
        settings
            .set_secret(SecretName::GithubToken, GH_TOKEN)
            .unwrap();
        settings.set_github_account("octocat", GithubSource::Device);
        settings.set_aws_profile("corp-sso");
        let mut opts = RealServiceOptions::new(settings.clone(), dir.path().join("state"));
        opts.github = Some(Arc::new(MockGithubApi::new()));
        opts.harnesses = Arc::new(HarnessCache::new(Arc::new(ScriptedProbe {
            probes: Arc::default(),
            found: Box::new(|_| vec![claude(|_| {}), codex(|_| {})]),
        })));
        let service = RealService::new(opts).await.unwrap();
        let st = service.settings().await.unwrap();
        assert_eq!(
            (st.aws_profile.as_str(), st.secrets.github_token),
            ("corp-sso", true)
        );
        let gh = service.github_status().await.unwrap();
        assert_eq!(
            (gh.signed_in, gh.login.as_deref(), gh.source),
            (true, Some("octocat"), Some(GithubSource::Device))
        );
        // the old fields are emptied, the token lives under the connector
        assert_eq!(settings.aws_profile(), "");
        assert!(!settings.has_secret(SecretName::GithubToken));
        assert!(dir.path().join("connectors.json").exists());
        service.shutdown().await;
    }
}

mod provisional_groups {
    use super::*;

    #[tokio::test]
    async fn environment_drafts_are_not_supported_yet() {
        let s = setup(Opts::default()).await;
        assert!(matches!(
            s.service.get_environment_draft("x").await,
            Err(ServiceError::Unsupported(_))
        ));
        assert_eq!(s.service.engine().kind, EngineKind::Real);
    }
}
