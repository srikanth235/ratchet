//! `packages/desktop/src/main/product-engine.test.ts`: the router that sends every command to the runtime of its
//! optimization (one hybrid runtime per GitHub repo, one simulation runtime per demo project).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratchet_agents::detect::{HarnessAuth, HarnessInfo, HarnessKind};
use ratchet_connectors::{
    AgentConnectorSource, ConnectorAgent, ResolvedAgent, ResolvedAws, ResolvedConnections,
    ResolvedGithub,
};
use ratchet_core::enums::{AgentKind, AgentsWhere, SandboxStatus};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::{
    EngineCreateOptimizationInput, EngineEvent, ProposeTemplateInput, SandboxPhase,
};
use ratchet_engine::desktop::testing::{DiffGit, RecordingRuntime};
use ratchet_engine::desktop::{
    HarnessCache, HarnessProbe, ImageLookupFailure, ImageQuery, ImageResolver, LiveAppConfig,
    LiveFactory, LiveHandle, LivePaths, LiveSettingsSource, LiveSnapshot, ProductEngine,
    ProductOptions, SimFactory, SimHandle,
};
use ratchet_engine::{EngineErrorCode, EngineResult};
use ratchet_ledger::{Ledger, LedgerOptions};
use ratchet_sandbox::images::RatchetImageKind;
use ratchet_sandbox::{ImageLookupError, ResolvedImages};

use async_trait::async_trait;

type Chosen = Box<dyn Fn(Option<&str>) -> ResolvedConnections + Send + Sync>;

struct TestSettings {
    live: Mutex<LiveSnapshot>,
    token: Mutex<Option<String>>,
    /// Connectors the daemon resolved for a repo (the default: the single token and profile of `live` / `token`).
    chosen: Mutex<Option<Chosen>>,
    env_keys: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl LiveSettingsSource for TestSettings {
    async fn connections(&self, repo: Option<&str>) -> ResolvedConnections {
        if let Some(f) = self.chosen.lock().unwrap().as_ref() {
            return f(repo);
        }
        let l = self.live();
        ResolvedConnections {
            github: self.secret("githubToken").map(|token| ResolvedGithub {
                id: "settings".into(),
                name: "GitHub".into(),
                login: None,
                token,
            }),
            aws: (!l.aws_profile.is_empty()).then(|| ResolvedAws {
                id: "settings".into(),
                name: l.aws_profile.clone(),
                profile: l.aws_profile,
                region: l.aws_region,
                monthly_cap_usd: None,
            }),
            agents: Vec::new(),
            cloud_keys: Default::default(),
        }
    }
    fn env_secret(&self, name: &str) -> Option<String> {
        self.env_keys.lock().unwrap().get(name).cloned()
    }
    fn live(&self) -> LiveSnapshot {
        self.live.lock().unwrap().clone()
    }
    fn secret(&self, name: &str) -> Option<String> {
        (name == "githubToken")
            .then(|| self.token.lock().unwrap().clone())
            .flatten()
    }
}

struct Probe;

fn harness(kind: HarnessKind, auth: HarnessAuth) -> HarnessInfo {
    HarnessInfo {
        kind,
        installed: true,
        version: Some("2.0.0".into()),
        path: Some(format!("/bin/{}", kind.as_str())),
        auth,
        hint: "ok".into(),
    }
}

#[async_trait]
impl HarnessProbe for Probe {
    async fn login_path(&self) -> Result<Option<String>, String> {
        Ok(Some("/login/bin".into()))
    }
    async fn detect(&self, _: Option<&str>) -> Result<Vec<HarnessInfo>, String> {
        Ok(vec![
            harness(HarnessKind::ClaudeCode, HarnessAuth::SignedIn),
            harness(HarnessKind::Codex, HarnessAuth::SignedOut),
        ])
    }
}

/// What a built live runtime was asked to be (the interesting part of `HybridOptions`).
#[derive(Debug, Clone)]
struct LiveSeen {
    github_repo: Option<String>,
    github_token: Option<String>,
    region: String,
    aws_profile: Option<String>,
    sandbox_none: bool,
    app_version: Option<String>,
    login_path: Option<String>,
    bench_image_ref: Option<String>,
    tofu_binary: String,
    module_dir: String,
    harnesses: Vec<(HarnessKind, HarnessAuth)>,
    /// The API keys handed to the local Claude Code / Codex adapters.
    local_keys: (Option<String>, Option<String>),
    /// What the runner secret source answers for (GITHUB_TOKEN, ANTHROPIC_API_KEY, OPENAI_API_KEY).
    runner_secrets: (Option<String>, Option<String>, Option<String>),
}

struct LiveRec {
    seen: LiveSeen,
    engine: Arc<RecordingRuntime>,
    disposed: Arc<AtomicBool>,
}

struct SimRec {
    project: String,
    auto: Option<bool>,
    sandbox_id: String,
    engine: Arc<RecordingRuntime>,
    disposed: Arc<AtomicBool>,
}

type Resolver =
    Box<dyn Fn(&ImageQuery) -> Result<ResolvedImages, ImageLookupFailure> + Send + Sync>;

struct Built {
    product: ProductEngine,
    ledger: Ledger,
    settings: Arc<TestSettings>,
    live: Arc<Mutex<Vec<LiveRec>>>,
    sims: Arc<Mutex<Vec<SimRec>>>,
    image_calls: Arc<Mutex<Vec<(String, String, Vec<RatchetImageKind>, Option<String>)>>>,
    configs: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    missing: Arc<Mutex<Vec<String>>>,
    resolver: Arc<Mutex<Resolver>>,
    _dir: tempfile::TempDir,
}

#[derive(Clone, Copy)]
struct BuildOpts {
    token: bool,
    aws: bool,
    seed_aws_sandbox: bool,
}

impl Default for BuildOpts {
    fn default() -> Self {
        Self {
            token: true,
            aws: true,
            seed_aws_sandbox: false,
        }
    }
}

async fn build(o: BuildOpts) -> Built {
    let dir = tempfile::tempdir().unwrap();
    let state_root = dir.path().to_path_buf();
    let ledger = Ledger::open_memory(LedgerOptions::default()).await.unwrap();
    let settings = Arc::new(TestSettings {
        live: Mutex::new(LiveSnapshot {
            aws_profile: if o.aws {
                "sandbox".into()
            } else {
                String::new()
            },
            ..LiveSnapshot::default()
        }),
        token: Mutex::new(o.token.then(|| "gho_TOKENTOKENTOKENTOKEN1234".to_owned())),
        chosen: Mutex::new(None),
        env_keys: Mutex::new(HashMap::new()),
    });
    if o.seed_aws_sandbox {
        let mut r = ratchet_core::ledger::RecordSandboxInput::new(
            "ratchet-sandbox",
            ratchet_core::enums::SandboxProviderKind::Aws,
            "us-east-1",
        );
        r.status = Some(SandboxStatus::Applied);
        ledger.record_sandbox(r).await.unwrap();
    }
    let live: Arc<Mutex<Vec<LiveRec>>> = Arc::default();
    let sims: Arc<Mutex<Vec<SimRec>>> = Arc::default();
    let image_calls: Arc<Mutex<Vec<_>>> = Arc::default();
    let configs: Arc<Mutex<HashMap<String, serde_json::Value>>> = Arc::default();
    let missing: Arc<Mutex<Vec<String>>> = Arc::default();
    let resolver: Arc<Mutex<Resolver>> = Arc::new(Mutex::new(Box::new(|_q| {
        Ok(ResolvedImages {
            bench: Some("ami-0bench00000000000a".into()),
            ..Default::default()
        })
    })));

    let create_runtime: LiveFactory = {
        let (live, ledger, missing) = (live.clone(), ledger.clone(), missing.clone());
        Arc::new(move |opts| {
            let (live, ledger, missing) = (live.clone(), ledger.clone(), missing.clone());
            Box::pin(async move {
                let n = live.lock().unwrap().len();
                let engine = RecordingRuntime::new(&format!("live{n}"), &ledger);
                let disposed = Arc::new(AtomicBool::new(false));
                let seen = LiveSeen {
                    github_repo: opts.live.github_repo.clone(),
                    github_token: opts.live.github_token.clone(),
                    region: opts.live.region.clone(),
                    aws_profile: opts.live.aws_profile.clone(),
                    sandbox_none: opts.live.sandbox.is_none(),
                    app_version: opts.live.app_version.clone(),
                    login_path: opts.login_path.clone(),
                    bench_image_ref: opts.live.bench_image_ref.clone(),
                    tofu_binary: opts.live.tofu_binary.display().to_string(),
                    module_dir: opts.live.module_dir.display().to_string(),
                    harnesses: opts
                        .harnesses
                        .clone()
                        .unwrap_or_default()
                        .iter()
                        .map(|h| (h.kind, h.auth))
                        .collect(),
                    local_keys: (
                        opts.local_agents
                            .adapters
                            .claude
                            .as_ref()
                            .and_then(|c| c.api_key.clone()),
                        opts.local_agents
                            .adapters
                            .codex
                            .as_ref()
                            .and_then(|c| c.api_key.clone()),
                    ),
                    runner_secrets: {
                        let src = opts.live.secrets.clone();
                        let get = |n: &str| src.as_ref().and_then(|f| f(AgentKind::ClaudeCode, n));
                        (
                            get("GITHUB_TOKEN"),
                            get("ANTHROPIC_API_KEY"),
                            get("OPENAI_API_KEY"),
                        )
                    },
                };
                let d = disposed.clone();
                let handle = LiveHandle {
                    engine: engine.clone(),
                    repo_url: seen
                        .github_repo
                        .as_ref()
                        .map(|r| format!("https://github.com/{r}.git")),
                    missing: missing.lock().unwrap().clone(),
                    agents_where: AgentsWhere::Local,
                    runner_problem: None,
                    disposer: Arc::new(move || {
                        let d = d.clone();
                        Box::pin(async move { d.store(true, Ordering::SeqCst) })
                    }),
                };
                live.lock().unwrap().push(LiveRec {
                    seen,
                    engine,
                    disposed,
                });
                Ok(handle)
            })
        })
    };
    let create_simulation: SimFactory = {
        let (sims, ledger) = (sims.clone(), ledger.clone());
        Arc::new(move |o| {
            let (sims, ledger) = (sims.clone(), ledger.clone());
            Box::pin(async move {
                let project = o.project.as_str().to_owned();
                let engine = RecordingRuntime::new(&format!("sim-{project}"), &ledger);
                let disposed = Arc::new(AtomicBool::new(false));
                let sandbox_id = o
                    .settings
                    .sandbox
                    .sandbox_id
                    .as_ref()
                    .map(|f| f("us-east-1"))
                    .unwrap_or_default();
                let d = disposed.clone();
                let handle = SimHandle {
                    engine: engine.clone(),
                    repo_url: format!("/work/{project}/remote.git"),
                    base_ref: "main".into(),
                    git: Arc::new(DiffGit),
                    disposer: Arc::new(move || {
                        let d = d.clone();
                        Box::pin(async move { d.store(true, Ordering::SeqCst) })
                    }),
                };
                sims.lock().unwrap().push(SimRec {
                    project,
                    auto: o.settings.sandbox.auto,
                    sandbox_id,
                    engine,
                    disposed,
                });
                Ok(handle)
            })
        })
    };
    let resolve_images: ImageResolver = {
        let (calls, resolver) = (image_calls.clone(), resolver.clone());
        Arc::new(move |q| {
            let (calls, resolver) = (calls.clone(), resolver.clone());
            Box::pin(async move {
                calls.lock().unwrap().push((
                    q.app_version.clone(),
                    q.region.clone(),
                    q.need.clone(),
                    q.profile.clone(),
                ));
                (resolver.lock().unwrap())(&q)
            })
        })
    };
    let cfgs = configs.clone();
    let product = ProductEngine::new(ProductOptions {
        ledger: ledger.clone(),
        repo_root: state_root.clone(),
        work_root: state_root.join("sim"),
        sim_idle_ms: None,
        clock: None,
        create_simulation: Some(create_simulation),
        live: Some(LiveAppConfig {
            settings: settings.clone(),
            state_root: state_root.clone(),
            defaults: LivePaths {
                tofu_binary: "tofu".into(),
                module_dir: "/m".into(),
            },
            harnesses: Arc::new(HarnessCache::new(Arc::new(Probe))),
            app_version: Some("1.2.3".into()),
            config_for: Arc::new(move |s| cfgs.lock().unwrap().get(s).cloned()),
            env: Some(HashMap::new()),
            create_runtime: Some(create_runtime),
            resolve_images: Some(resolve_images),
        }),
    });
    Built {
        product,
        ledger,
        settings,
        live,
        sims,
        image_calls,
        configs,
        missing,
        resolver,
        _dir: dir,
    }
}

fn input(repo_url: &str) -> EngineCreateOptimizationInput {
    EngineCreateOptimizationInput {
        name: None,
        repo_url: repo_url.into(),
        base_ref: "main".into(),
        config_path: None,
        agents: vec![AgentKind::ClaudeCode],
        budget_usd: 10.0,
        time_limit_seconds: 600,
        plateau_evals: 5,
        sandbox_id: None,
        agents_where: None,
    }
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(30)).await;
}

fn err_of<T: std::fmt::Debug>(r: EngineResult<T>) -> ratchet_engine::EngineError {
    r.expect_err("expected an error")
}

/// product-engine.test.ts › slugOf
#[test]
fn slug_of_cases() {
    for (repo, slug) in [
        ("acme/api", Some("acme/api")),
        ("https://github.com/acme/api.git", Some("acme/api")),
        ("git@github.com:acme/api", Some("acme/api")),
        ("/work/search/remote.git", None),
        ("C:\\x\\y", None),
    ] {
        assert_eq!(ratchet_github::slug_of(repo).as_deref(), slug, "{repo}");
    }
}

mod real_runs {
    use super::*;

    /// builds one hybrid runtime per repo from Settings, the cached harnesses and the GitHub token (main process only)
    #[tokio::test]
    async fn builds_one_hybrid_runtime_per_repo_from_settings_the_cached_harnesses_and_the_github_token()
     {
        let b = build(BuildOpts::default()).await;
        b.configs.lock().unwrap().insert(
            "acme/api".into(),
            serde_json::json!({"environment": {"region": "eu-west-2"}}),
        );
        let mut i = input("acme/api");
        i.agents_where = Some(AgentsWhere::Local);
        let id = b.product.create_optimization(&i).await.unwrap();
        {
            let live = b.live.lock().unwrap();
            assert_eq!(live.len(), 1);
            let s = &live[0].seen;
            assert_eq!(s.github_repo.as_deref(), Some("acme/api"));
            assert_eq!(
                s.github_token.as_deref(),
                Some("gho_TOKENTOKENTOKENTOKEN1234")
            );
            assert_eq!(s.region, "eu-west-2");
            assert_eq!(s.aws_profile.as_deref(), Some("sandbox"));
            assert!(s.sandbox_none);
            assert_eq!(s.app_version.as_deref(), Some("1.2.3"));
            assert_eq!(s.login_path.as_deref(), Some("/login/bin"));
            assert_eq!(s.bench_image_ref.as_deref(), Some("ami-0bench00000000000a"));
            assert_eq!(
                (s.tofu_binary.as_str(), s.module_dir.as_str()),
                ("tofu", "/m")
            );
            assert_eq!(
                s.harnesses,
                [
                    (HarnessKind::ClaudeCode, HarnessAuth::SignedIn),
                    (HarnessKind::Codex, HarnessAuth::SignedOut)
                ]
            );
        }
        {
            let calls = b.image_calls.lock().unwrap();
            assert_eq!(
                calls[0],
                (
                    "1.2.3".to_owned(),
                    "eu-west-2".to_owned(),
                    vec![RatchetImageKind::Bench],
                    Some("sandbox".to_owned())
                )
            );
            assert_eq!(calls[1].2, vec![RatchetImageKind::Runner]);
        }
        // the ledger records the runtime's remote; the second call reuses the runtime
        let opt = b.ledger.get_optimization(&id).await.unwrap().unwrap();
        assert_eq!(opt.repo, "https://github.com/acme/api.git");
        b.product
            .create_optimization(&input("https://github.com/acme/api.git"))
            .await
            .unwrap();
        assert_eq!(b.live.lock().unwrap().len(), 1);
        b.product
            .create_optimization(&input("acme/other"))
            .await
            .unwrap();
        assert_eq!(b.live.lock().unwrap().len(), 2);
        b.product.dispose().await;
    }

    fn agent(
        id: &str,
        agent: ConnectorAgent,
        source: AgentConnectorSource,
        where_: AgentsWhere,
        key: Option<&str>,
    ) -> ResolvedAgent {
        ResolvedAgent {
            connector_id: id.into(),
            name: id.into(),
            agent,
            source,
            where_,
            api_key: key.map(str::to_owned),
        }
    }

    /// engine.ts › live: the connectors chosen for the repo decide the token, the AWS profile and region, and the keys of the
    /// agents (a key chosen for a local agent goes to its adapter; a login runs with no key at all)
    #[tokio::test]
    async fn the_connectors_chosen_for_the_repo_decide_the_token_the_profile_and_the_agent_keys() {
        let b = build(BuildOpts::default()).await;
        *b.settings.chosen.lock().unwrap() = Some(Box::new(|repo| {
            let (token, profile, region) = match repo {
                Some("acme/api") => ("ghp_ORGTOKEN", "corp-sso", "eu-central-1"),
                _ => ("ghp_PERSONAL", "personal", "us-west-2"),
            };
            ResolvedConnections {
                github: Some(ResolvedGithub {
                    id: format!("gh-{token}"),
                    name: "gh".into(),
                    login: None,
                    token: token.into(),
                }),
                aws: Some(ResolvedAws {
                    id: "aws".into(),
                    name: profile.into(),
                    profile: profile.into(),
                    region: region.into(),
                    monthly_cap_usd: None,
                }),
                agents: vec![
                    agent(
                        "claude-key",
                        ConnectorAgent::ClaudeCode,
                        AgentConnectorSource::ApiKey,
                        AgentsWhere::Local,
                        Some("sk-ant-local"),
                    ),
                    agent(
                        "codex-login",
                        ConnectorAgent::Codex,
                        AgentConnectorSource::Login,
                        AgentsWhere::Local,
                        None,
                    ),
                ],
                cloud_keys: [(ConnectorAgent::ClaudeCode, "sk-ant-local".to_owned())].into(),
            }
        }));
        b.settings
            .env_keys
            .lock()
            .unwrap()
            .insert("openaiApiKey".into(), "sk-env-openai".into());
        b.product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        b.product
            .create_optimization(&input("acme/web"))
            .await
            .unwrap();
        let live = b.live.lock().unwrap();
        assert_eq!(
            live.len(),
            2,
            "one runtime per repo, each from its own connectors"
        );
        let (api, web) = (&live[0].seen, &live[1].seen);
        assert_eq!(api.github_token.as_deref(), Some("ghp_ORGTOKEN"));
        assert_eq!(
            (api.aws_profile.as_deref(), api.region.as_str()),
            (Some("corp-sso"), "eu-central-1")
        );
        assert_eq!(web.github_token.as_deref(), Some("ghp_PERSONAL"));
        assert_eq!(
            (web.aws_profile.as_deref(), web.region.as_str()),
            (Some("personal"), "us-west-2")
        );
        // the AMI lookup used the chosen profile and region
        let calls = b.image_calls.lock().unwrap();
        assert_eq!(
            (calls[0].1.as_str(), calls[0].3.as_deref()),
            ("eu-central-1", Some("corp-sso"))
        );
        // a key chosen for a local agent goes to its adapter; a chosen login gets none
        assert_eq!(api.local_keys, (Some("sk-ant-local".to_owned()), None));
        // the runner secrets: the chosen token and key; no environment key next to a chosen login
        assert_eq!(
            api.runner_secrets,
            (
                Some("ghp_ORGTOKEN".to_owned()),
                Some("sk-ant-local".to_owned()),
                None
            )
        );
    }

    /// engine.ts › live: a cloud agent falls back to the environment's key only when no connector holds one
    #[tokio::test]
    async fn a_cloud_agent_falls_back_to_the_environment_key_when_no_connector_holds_one() {
        let b = build(BuildOpts::default()).await;
        b.settings
            .env_keys
            .lock()
            .unwrap()
            .insert("anthropicApiKey".into(), "sk-ant-env".into());
        b.product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        let live = b.live.lock().unwrap();
        assert_eq!(live[0].seen.runner_secrets.1.as_deref(), Some("sk-ant-env"));
        assert_eq!(live[0].seen.local_keys, (None, None));
    }

    /// engine.ts › live: no connector with a GitHub token is "sign in first", whatever the old settings say
    #[tokio::test]
    async fn no_chosen_github_account_is_sign_in_first() {
        let b = build(BuildOpts::default()).await;
        *b.settings.chosen.lock().unwrap() = Some(Box::new(|_| ResolvedConnections {
            github: None,
            aws: None,
            agents: Vec::new(),
            cloud_keys: Default::default(),
        }));
        let e = err_of(b.product.create_optimization(&input("acme/api")).await);
        assert_eq!(e.code, EngineErrorCode::NotConfigured);
        assert!(
            e.message.starts_with("Sign in with GitHub first"),
            "{}",
            e.message
        );
    }

    /// routes commands to the runtime of the optimization, by the repo in the ledger
    #[tokio::test]
    async fn routes_commands_to_the_runtime_of_the_optimization_by_the_repo_in_the_ledger() {
        let b = build(BuildOpts::default()).await;
        let a = b
            .product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        let o = b
            .product
            .create_optimization(&input("acme/other"))
            .await
            .unwrap();
        b.product.start(o.as_str()).await.unwrap();
        b.product.stop(a.as_str(), Some("x")).await.unwrap();
        b.product.open_pull_request(a.as_str()).await.unwrap();
        let agent = b.ledger.list_agents(&o).await.unwrap()[0].id.to_string();
        b.product.pause_agent(&agent).await.unwrap();
        b.product.message_agent(&agent, "hi").await.unwrap();
        let live = b.live.lock().unwrap();
        assert_eq!(
            live[0].engine.calls_except_create(),
            [format!("stop:{a}"), format!("pr:{a}")]
        );
        assert_eq!(
            live[1].engine.calls_except_create(),
            [
                format!("start:{o}"),
                format!("pause:{agent}"),
                format!("message:{agent}:hi")
            ]
        );
        drop(live);
        b.product.dispose().await;
    }

    /// refuses without a GitHub sign-in, and says what is missing when the runtime is not ready
    #[tokio::test]
    async fn refuses_without_a_github_sign_in_and_says_what_is_missing_when_the_runtime_is_not_ready()
     {
        let none = build(BuildOpts {
            token: false,
            ..Default::default()
        })
        .await;
        let e = err_of(none.product.create_optimization(&input("acme/api")).await);
        assert!(e.message.contains("Sign in with GitHub"), "{e}");
        let b = build(BuildOpts::default()).await;
        *b.missing.lock().unwrap() = vec![
            "set the bench AMI (Settings)".into(),
            "install and sign in to Claude Code or Codex on this machine (Settings > Agents)"
                .into(),
        ];
        let e = err_of(b.product.create_optimization(&input("acme/api")).await);
        assert_eq!(e.code, EngineErrorCode::NotConfigured);
        assert!(
            e.message
                .starts_with("Not ready: The evaluator image for this app version was not found"),
            "{e}"
        );
        assert!(
            e.message
                .contains("Install and sign in to Claude Code or Codex"),
            "{e}"
        );
        assert_eq!(b.product.missing_for("acme/api").await.unwrap().len(), 2);
        assert_eq!(
            ratchet_github::friendly_missing(&["something else".to_owned()]),
            ["something else"]
        );
        b.product.dispose().await;
    }

    /// a repo that is not on GitHub is rejected by name
    #[tokio::test]
    async fn a_repo_that_is_not_on_github_is_rejected_by_name() {
        let b = build(BuildOpts::default()).await;
        let e = err_of(
            b.product
                .create_optimization(&input("/home/me/project"))
                .await,
        );
        assert!(e.message.contains("not a GitHub repository"), "{e}");
        b.product.dispose().await;
    }

    /// skips the image lookup without AWS credentials, and keeps an explicit AMI override
    #[tokio::test]
    async fn skips_the_image_lookup_without_aws_credentials_and_keeps_an_explicit_ami_override() {
        let no_aws = build(BuildOpts {
            aws: false,
            ..Default::default()
        })
        .await;
        no_aws
            .product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        assert!(no_aws.image_calls.lock().unwrap().is_empty());
        assert_eq!(no_aws.live.lock().unwrap()[0].seen.bench_image_ref, None);
        let over = build(BuildOpts::default()).await;
        over.settings.live.lock().unwrap().bench_image_ref = "ami-0123456789abcdef0".into();
        over.product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        let calls = over.image_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "only the runner image is looked up");
        assert_eq!(calls[0].2, vec![RatchetImageKind::Runner]);
        assert_eq!(
            over.live.lock().unwrap()[0].seen.bench_image_ref.as_deref(),
            Some("ami-0123456789abcdef0")
        );
    }

    fn lookup_error(kind: RatchetImageKind, found: &[&str]) -> ImageLookupFailure {
        ImageLookupFailure::Lookup(
            ImageLookupError {
                kind,
                version: "1.2.3".into(),
                region: "us-east-1".into(),
                arch: "x86_64".into(),
                found: found.iter().map(|s| (*s).to_owned()).collect(),
            }
            .to_string(),
        )
    }

    /// a failed image lookup becomes a "what is missing" line, not a crash
    #[tokio::test]
    async fn a_failed_image_lookup_becomes_a_what_is_missing_line_not_a_crash() {
        let b = build(BuildOpts::default()).await;
        *b.resolver.lock().unwrap() =
            Box::new(|_| Err(lookup_error(RatchetImageKind::Bench, &["0.0.1"])));
        let e = err_of(b.product.create_optimization(&input("acme/api")).await);
        assert!(
            e.message.starts_with("Not ready: ") && e.message.contains("1.2.3"),
            "{e}"
        );
        assert_eq!(b.live.lock().unwrap().len(), 1);
        b.product.dispose().await;
    }

    /// a missing cloud runner image only blocks Cloud (unattended) runs, not this-machine runs
    #[tokio::test]
    async fn a_missing_cloud_runner_image_only_blocks_cloud_runs_not_this_machine_runs() {
        let b = build(BuildOpts::default()).await;
        *b.resolver.lock().unwrap() = Box::new(|q| {
            if q.need[0] == RatchetImageKind::Runner {
                Err(lookup_error(RatchetImageKind::Runner, &[]))
            } else {
                Ok(ResolvedImages {
                    bench: Some("ami-0bench00000000000a".into()),
                    ..Default::default()
                })
            }
        });
        let mut local = input("acme/api");
        local.agents_where = Some(AgentsWhere::Local);
        let id = b.product.create_optimization(&local).await.unwrap();
        assert!(id.as_str().starts_with("opt"), "{id}");
        let mut cloud = input("acme/api");
        cloud.agents_where = Some(AgentsWhere::Cloud);
        let e = err_of(b.product.create_optimization(&cloud).await);
        assert!(
            e.message.starts_with("Not ready: ") && e.message.contains("runner"),
            "{e}"
        );
        b.product.dispose().await;
    }

    /// proposeTemplate goes to the runtime of that repo, with the owner/name
    #[tokio::test]
    async fn propose_template_goes_to_the_runtime_of_that_repo_with_the_owner_name() {
        let b = build(BuildOpts::default()).await;
        let r = b
            .product
            .propose_template(&ProposeTemplateInput {
                repo_url: "https://github.com/acme/api".into(),
                base_ref: "main".into(),
                template_id: "http-latency".into(),
                metric: None,
                direction: None,
                value_usd: None,
                overwrite: None,
            })
            .await
            .unwrap();
        assert_eq!(r.pr_url, "https://github.com/acme/api/pull/9");
        let live = b.live.lock().unwrap();
        assert_eq!(live[0].seen.github_repo.as_deref(), Some("acme/api"));
        let p = live[0].engine.proposals.lock().unwrap();
        assert_eq!(p[0].repo_url, "https://github.com/acme/api.git");
        assert_eq!(p[0].template_id, "http-latency");
        drop(p);
        drop(live);
        b.product.dispose().await;
    }

    /// a changed setting rebuilds an idle runtime on next use, but never one that is running
    #[tokio::test]
    async fn a_changed_setting_rebuilds_an_idle_runtime_on_next_use_but_never_one_that_is_running()
    {
        let b = build(BuildOpts::default()).await;
        b.product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        b.product.invalidate();
        b.product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        settle().await;
        assert_eq!(b.live.lock().unwrap().len(), 2);
        assert!(b.live.lock().unwrap()[0].disposed.load(Ordering::SeqCst));
        b.live.lock().unwrap()[1]
            .engine
            .active
            .store(true, Ordering::SeqCst);
        b.product.invalidate();
        b.product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        assert_eq!(b.live.lock().unwrap().len(), 2);
        assert!(!b.live.lock().unwrap()[1].disposed.load(Ordering::SeqCst));
        b.product.dispose().await;
    }

    /// watches an environment that outlived the last session even with no repo open
    #[tokio::test]
    async fn watches_an_environment_that_outlived_the_last_session_even_with_no_repo_open() {
        let b = build(BuildOpts {
            seed_aws_sandbox: true,
            ..Default::default()
        })
        .await;
        settle().await;
        assert_eq!(b.live.lock().unwrap().len(), 1);
        assert_eq!(b.live.lock().unwrap()[0].seen.github_repo, None);
        let act = b.product.sandbox_activity("ratchet-sandbox").await.unwrap();
        assert_eq!(act.region.as_deref(), Some("us-east-1"));
        b.product.end_sandbox("ratchet-sandbox").await.unwrap();
        assert_eq!(
            b.live.lock().unwrap()[0].engine.calls(),
            ["activity:ratchet-sandbox", "end:ratchet-sandbox"]
        );
        b.product.dispose().await;
    }
}

mod demos {
    use super::*;

    /// starts a demo in a simulation runtime with an automatic environment, created once per project
    #[tokio::test]
    async fn starts_a_demo_in_a_simulation_runtime_with_an_automatic_environment_created_once_per_project()
     {
        let b = build(BuildOpts::default()).await;
        let id = b
            .product
            .create_demo("demo-search", &input("acme/demo-search"))
            .await
            .unwrap();
        {
            let sims = b.sims.lock().unwrap();
            assert_eq!(sims.len(), 1);
            assert_eq!(
                (sims[0].project.as_str(), sims[0].auto),
                ("search", Some(true))
            );
            assert_eq!(
                sims[0].engine.calls()[0],
                "create:/work/search/remote.git:-"
            );
        }
        assert!(b.live.lock().unwrap().is_empty());
        assert_eq!(
            b.ledger.get_optimization(&id).await.unwrap().unwrap().repo,
            "/work/search/remote.git"
        );
        b.product
            .create_demo("demo-search", &input("x"))
            .await
            .unwrap();
        assert_eq!(b.sims.lock().unwrap().len(), 1);
        b.product
            .create_demo("demo-cli", &input("x"))
            .await
            .unwrap();
        b.product
            .create_demo("demo-forecast", &input("x"))
            .await
            .unwrap();
        let projects: Vec<_> = b
            .sims
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.project.clone())
            .collect();
        assert_eq!(projects, ["search", "cli", "forecast"]);
        // each demo project has its own environment id
        let ids: std::collections::HashSet<_> = b
            .sims
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.sandbox_id.clone())
            .collect();
        assert_eq!(ids.len(), 3);
        b.product.dispose().await;
        assert!(
            b.sims
                .lock()
                .unwrap()
                .iter()
                .all(|s| s.disposed.load(Ordering::SeqCst))
        );
    }

    /// routes a demo optimization to its simulation runtime and diffs with its git
    #[tokio::test]
    async fn routes_a_demo_optimization_to_its_simulation_runtime_and_diffs_with_its_git() {
        let b = build(BuildOpts::default()).await;
        let id = b
            .product
            .create_demo("demo-cli", &input("x"))
            .await
            .unwrap();
        b.product.start(id.as_str()).await.unwrap();
        b.product.stop(id.as_str(), None).await.unwrap();
        assert_eq!(
            b.sims.lock().unwrap()[0].engine.calls(),
            [
                "create:/work/cli/remote.git:-".to_owned(),
                format!("start:{id}"),
                format!("stop:{id}")
            ]
        );
        let diff = b
            .product
            .diff("/work/cli/remote.git", "main", "abc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(diff[0].path, "a.ts");
        b.product.dispose().await;
    }

    /// works without any sign-in or AWS: a demo needs nothing
    #[tokio::test]
    async fn works_without_any_sign_in_or_aws_a_demo_needs_nothing() {
        let b = build(BuildOpts {
            token: false,
            aws: false,
            ..Default::default()
        })
        .await;
        let id = b
            .product
            .create_demo("demo-search", &input("x"))
            .await
            .unwrap();
        assert!(id.as_str().starts_with("opt"), "{id}");
        b.product.dispose().await;
    }
}

mod events {
    use super::*;

    fn run_active(active: bool, ids: &[&str]) -> EngineEvent {
        EngineEvent::RunActive {
            active,
            optimization_ids: ids.iter().map(|i| OptimizationId::new(*i)).collect(),
        }
    }

    /// merges run-active from every runtime: active while any runs, off when all are done; forwards the rest
    #[tokio::test]
    async fn merges_run_active_from_every_runtime_and_forwards_the_rest() {
        let b = build(BuildOpts::default()).await;
        let mut rx = b.product.subscribe();
        b.product
            .create_optimization(&input("acme/api"))
            .await
            .unwrap();
        b.product
            .create_demo("demo-search", &input("x"))
            .await
            .unwrap();
        settle().await; // the forwarders attach
        let (live, sim) = (
            b.live.lock().unwrap()[0].engine.clone(),
            b.sims.lock().unwrap()[0].engine.clone(),
        );
        // one at a time, so the forwarding order is the emit order
        for (rt, e) in [
            (&live, run_active(true, &["opt_a"])),
            (&sim, run_active(true, &["opt_b"])),
            (&live, run_active(false, &[])),
            (&sim, run_active(false, &[])),
            (
                &sim,
                EngineEvent::Sandbox {
                    sandbox_id: "ratchet-sim-search".into(),
                    phase: SandboxPhase::Ready,
                    region: Some("us-east-1".into()),
                    text: "environment ready".into(),
                },
            ),
        ] {
            rt.emit(e);
            settle().await;
        }
        let mut seen = vec![];
        while let Ok(e) = rx.try_recv() {
            seen.push(e);
        }
        let active: Vec<_> = seen
            .iter()
            .filter(|e| matches!(e, EngineEvent::RunActive { .. }))
            .cloned()
            .collect();
        assert_eq!(
            active,
            [
                run_active(true, &["opt_a"]),
                run_active(true, &["opt_a", "opt_b"]),
                run_active(true, &["opt_b"]),
                run_active(false, &[]),
            ]
        );
        assert!(matches!(
            seen.last(),
            Some(EngineEvent::Sandbox {
                phase: SandboxPhase::Ready,
                ..
            })
        ));
        assert!(!b.product.run_active());
        b.product.dispose().await;
    }
}
