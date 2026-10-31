//! `RealService`: the [`AppService`] of the real engine. Port of `packages/desktop/src/main/service.ts` on top of
//! [`ratchet_engine::desktop::ProductEngine`].
//!
//! Reads come from the ledger through [`LedgerViews`]; commands go to the engine through the product router; settings and
//! secrets live in [`SettingsStore`] / the connector store (secret values stay in the daemon). Connectors are the accounts
//! a run uses; the single-account calls of the first Settings (`setAwsProfile`, `setSecret`, `githubSignIn`, ...) work on top of
//! them. Environment drafts run the user's own setup agent (`crate::drafts`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use ratchet_agents::detect::{HarnessAuth as DetectAuth, HarnessInfo, HarnessKind as DetectKind};
use ratchet_connectors::{
    AwsProfile, AwsProfileLister as ConnectorProfiles, ConnectorAgent, ConnectorDeps,
    ConnectorError, ConnectorKind, ConnectorProbe, ConnectorStore, GITHUB_ENV_ID, GithubStored,
    RecordKind, SecretName as ConnectorSecret,
};
use ratchet_core::enums::{AgentKind, AgentsWhere, SandboxProviderKind, SandboxStatus};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{
    Clock, CostReconcileReport, EngineCreateOptimizationInput, ProposeTemplateInput,
    ProposeTemplateResult, SandboxPhase,
};
use ratchet_core::templates::TemplateFilter;
use ratchet_core::{Calibration, EffectiveProtected};
use ratchet_engine::desktop::{
    HarnessCache, LiveAppConfig, LivePaths, LiveSettingsSource, LiveSnapshot, ProductEngine,
    ProductOptions, effective_paths,
};
use ratchet_github::{GithubApi, slug_of};
use ratchet_ledger::{Ledger, LedgerOptions};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::aws_profiles::{AwsProfileList, ListAwsProfilesOptions, list_aws_profiles};
use super::mappers::{
    derived_phase, first_line, github_error, phase_vm, receipt_vm, service_error,
};
use super::settings::{SettingsStore, env_for};
use crate::api::validate::{validate_profile_name, validate_secret_format};
use crate::api::*;
use crate::connectors_hub::{ConnectorsHub, HubOptions, flow_vm};
use crate::drafts::{DrafterSource, EnvDrafts};
use crate::service::{AppService, ServiceError, ServiceResult};
use crate::views::{LedgerViews, PrUrlLookup};

const DEFAULT_IDLE_MINUTES: u32 = 30;
const SIGN_IN_FIRST: &str = "Sign in with GitHub first (Connectors).";

/// "Test connection" probes (the runtime's `ConnectionTester` in production).
#[async_trait]
pub trait ConnectionProbe: Send + Sync {
    async fn aws(&self, region: &str, profile: &str) -> (bool, String);
    async fn anthropic(&self, api_key: &str) -> (bool, String);
    async fn openai(&self, api_key: &str) -> (bool, String);
}

#[async_trait]
impl ConnectionProbe for ratchet_engine::runtime::ConnectionTester {
    async fn aws(&self, region: &str, profile: &str) -> (bool, String) {
        let r =
            ratchet_engine::runtime::ConnectionTester::aws(self, region, Some(profile), None).await;
        (r.ok, r.detail)
    }
    async fn anthropic(&self, api_key: &str) -> (bool, String) {
        let r = ratchet_engine::runtime::ConnectionTester::anthropic(self, api_key).await;
        (r.ok, r.detail)
    }
    async fn openai(&self, api_key: &str) -> (bool, String) {
        let r = ratchet_engine::runtime::ConnectionTester::openai(self, api_key).await;
        (r.ok, r.detail)
    }
}

pub type AwsProfileLister = Arc<dyn Fn() -> AwsProfileList + Send + Sync>;
pub type OpenExternal = Arc<dyn Fn(&str) + Send + Sync>;

/// The connector store's view of the "test connection" probes.
struct ProbeAdapter(Arc<dyn ConnectionProbe>);

#[async_trait]
impl ConnectorProbe for ProbeAdapter {
    async fn aws(&self, region: &str, profile: &str) -> (bool, String) {
        self.0.aws(region, profile).await
    }
    async fn anthropic(&self, api_key: &str) -> (bool, String) {
        self.0.anthropic(api_key).await
    }
    async fn openai(&self, api_key: &str) -> (bool, String) {
        self.0.openai(api_key).await
    }
}

/// The profiles in `~/.aws`, as the connector store reads them (names and sign-in styles).
fn connector_profiles(lister: &AwsProfileLister) -> ConnectorProfiles {
    let lister = lister.clone();
    Arc::new(move || {
        let l = lister();
        ratchet_connectors::AwsProfileList {
            profiles: l
                .profiles
                .iter()
                .map(|p| AwsProfile {
                    name: p.name.clone(),
                    kind: match p.kind {
                        AwsProfileKind::Sso => AwsAuthStyle::Sso,
                        AwsProfileKind::Role => AwsAuthStyle::Role,
                        AwsProfileKind::Keys => AwsAuthStyle::Keys,
                        AwsProfileKind::Process => AwsAuthStyle::Process,
                        AwsProfileKind::Other => AwsAuthStyle::Other,
                    },
                })
                .collect(),
            files_found: l.files_found,
        }
    })
}

pub struct RealServiceOptions {
    /// The ledger file; `None` = in memory.
    pub ledger_path: Option<PathBuf>,
    pub settings: Arc<SettingsStore>,
    /// What empty tofu settings resolve to.
    pub live_defaults: LivePaths,
    /// Where per-sandbox OpenTofu state, the git cache and local agent worktrees live.
    pub state_root: PathBuf,
    /// The monorepo root (the demos need `examples/` and `node_modules`).
    pub repo_root: PathBuf,
    /// Scratch root of the demo runs.
    pub work_root: PathBuf,
    /// The app version the published AMIs are matched against.
    pub app_version: Option<String>,
    pub clock: Option<Arc<dyn Clock>>,
    /// GitHub REST + device flow. Without it GitHub features report that they are unavailable.
    pub github: Option<Arc<dyn GithubApi>>,
    /// Open a URL in the system browser (the device-flow verification page).
    pub open_external: Option<OpenExternal>,
    pub harnesses: Arc<HarnessCache>,
    /// The AWS profiles on this machine; default: parse `~/.aws`.
    pub aws_profiles: Option<AwsProfileLister>,
    /// Where the connectors' metadata lives; default `connectors.json` next to the settings file (memory only without one).
    pub connectors_file: Option<PathBuf>,
    pub connection_probe: Option<Arc<dyn ConnectionProbe>>,
    /// Idle window of the automatic environment, minutes (shown in Environments).
    pub idle_minutes: u32,
    /// Idle window of the demo environment, ms.
    pub sim_idle_ms: Option<i64>,
    /// Test seam: the product engine to use instead of building one.
    pub product: Option<ProductEngine>,
    /// Test seams of the product engine built here.
    pub create_runtime: Option<ratchet_engine::desktop::LiveFactory>,
    pub resolve_images: Option<ratchet_engine::desktop::ImageResolver>,
    pub create_simulation: Option<ratchet_engine::desktop::SimFactory>,
}

impl RealServiceOptions {
    pub fn new(settings: Arc<SettingsStore>, state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            ledger_path: None,
            settings,
            live_defaults: LivePaths::default(),
            work_root: state_root.join("sim"),
            state_root,
            repo_root: std::env::var_os("RATCHET_REPO_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
            app_version: None,
            clock: None,
            github: None,
            open_external: None,
            harnesses: Arc::new(HarnessCache::default()),
            aws_profiles: None,
            connectors_file: None,
            connection_probe: None,
            idle_minutes: DEFAULT_IDLE_MINUTES,
            sim_idle_ms: None,
            product: None,
            create_runtime: None,
            resolve_images: None,
            create_simulation: None,
        }
    }
}

/// What the product router reads from the daemon: the live settings, and the connectors a run on a repo uses.
struct SettingsSource {
    settings: Arc<SettingsStore>,
    connectors: Arc<ConnectorStore>,
}

#[async_trait]
impl LiveSettingsSource for SettingsSource {
    fn live(&self) -> LiveSnapshot {
        let l = self.settings.live();
        LiveSnapshot {
            aws_profile: l.aws_profile,
            aws_region: l.aws_region,
            runner_image_ref: l.runner_image_ref,
            bench_image_ref: l.bench_image_ref,
            tofu_binary: l.tofu_binary,
            module_dir: l.module_dir,
        }
    }

    fn secret(&self, name: &str) -> Option<String> {
        let name = match name {
            "githubToken" => ConnectorSecret::GithubToken,
            "anthropicApiKey" => ConnectorSecret::AnthropicApiKey,
            "openaiApiKey" => ConnectorSecret::OpenaiApiKey,
            _ => return None,
        };
        self.connectors.secret_value(name)
    }

    fn env_secret(&self, name: &str) -> Option<String> {
        name.parse::<SecretName>()
            .ok()
            .and_then(|n| env_for(n).iter().find_map(|k| self.settings.env().get(*k)))
            .filter(|v| !v.is_empty())
            .cloned()
    }

    async fn connections(&self, repo: Option<&str>) -> ratchet_connectors::ResolvedConnections {
        self.connectors.resolve(repo).await
    }
}

struct Phase {
    phase: SandboxPhase,
    region: Option<String>,
    text: String,
}

struct Inner {
    ledger: Ledger,
    views: LedgerViews,
    product: ProductEngine,
    settings: Arc<SettingsStore>,
    tx: broadcast::Sender<ApiEvent>,
    ledger_path: Option<String>,
    github: Option<Arc<dyn GithubApi>>,
    /// The connectors group (and the GitHub device flow behind both Settings and Connectors).
    hub: ConnectorsHub,
    connectors: Arc<ConnectorStore>,
    /// The login of a token that came from the environment (no stored account): looked up once per token.
    env_login: Mutex<Option<(String, Option<String>)>>,
    /// Parsed ratchet.yaml (as JSON) of repos seen in step 1 (the region).
    configs: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    phases: Mutex<HashMap<String, Phase>>,
    live_defaults: LivePaths,
    harnesses: Arc<HarnessCache>,
    aws_profiles: AwsProfileLister,
    probe: Option<Arc<dyn ConnectionProbe>>,
    idle_minutes: u32,
    pump: Mutex<Option<JoinHandle<()>>>,
    /// Environment drafts: the setup agent (the user's own Claude Code / Codex, run on this machine) per repository.
    drafts: EnvDrafts,
}

/// The setup agent of a repository comes from its live runtime.
struct ProductDrafters(ProductEngine);

#[async_trait]
impl DrafterSource for ProductDrafters {
    async fn drafter_for(
        &self,
        repo: &str,
    ) -> ServiceResult<Arc<dyn ratchet_env::EnvironmentDrafter>> {
        self.0
            .environment_drafter(repo)
            .await
            .map_err(service_error)
    }
}

#[derive(Clone)]
pub struct RealService(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn invalid_field(path: &str, message: String) -> ServiceError {
    ServiceError::InvalidFields {
        message: message.clone(),
        fields: vec![FieldError::new(path, message)],
    }
}

fn demo_name(id: &str) -> String {
    let title = ratchet_core::get_template(id)
        .map(|t| t.title)
        .unwrap_or_default();
    let t = title.trim_start_matches("Demo:").trim_start();
    let mut c = t.chars();
    let capital = c
        .next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default();
    format!("{capital} (demo)")
}

impl RealService {
    pub async fn new(opts: RealServiceOptions) -> ServiceResult<Self> {
        let clock: Arc<dyn Clock> = opts.clock.clone().unwrap_or_else(|| Arc::new(SystemClock));
        let ledger_opts = LedgerOptions {
            clock: Some(Arc::clone(&clock)),
            max_connections: None,
        };
        let ledger = match &opts.ledger_path {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    tokio::fs::create_dir_all(dir).await.map_err(|e| {
                        ServiceError::Internal(format!("cannot create {}: {e}", dir.display()))
                    })?;
                }
                Ledger::open(p, ledger_opts).await?
            }
            None => Ledger::open_memory(ledger_opts).await?,
        };
        let settings = opts.settings.clone();
        let pr_urls: PrUrlLookup = {
            let s = settings.clone();
            Arc::new(move |id| s.pr_url(id))
        };
        let views = LedgerViews::new(ledger.clone(), Arc::clone(&clock), pr_urls);
        let (tx, _) = broadcast::channel(1024);
        let configs: Arc<Mutex<HashMap<String, serde_json::Value>>> = Arc::default();

        let aws_profiles: AwsProfileLister = opts
            .aws_profiles
            .clone()
            .unwrap_or_else(|| Arc::new(|| list_aws_profiles(&ListAwsProfilesOptions::from_env())));
        let connectors = Arc::new(ConnectorStore::new(
            opts.connectors_file
                .clone()
                .or_else(|| settings.sibling_file("connectors.json")),
            settings.secret_store(),
            settings.env().clone(),
            ConnectorDeps {
                github: opts.github.clone(),
                harnesses: Some(opts.harnesses.clone()),
                aws_profiles: Some(connector_profiles(&aws_profiles)),
                probe: opts
                    .connection_probe
                    .clone()
                    .map(|p| Arc::new(ProbeAdapter(p)) as Arc<dyn ConnectorProbe>),
                legacy: Some(settings.clone()),
                clock: Some(Arc::clone(&clock)),
            },
        ));

        let product = match opts.product.clone() {
            Some(p) => p,
            None => {
                let cfg = configs.clone();
                ProductEngine::new(ProductOptions {
                    ledger: ledger.clone(),
                    repo_root: opts.repo_root.clone(),
                    work_root: opts.work_root.clone(),
                    sim_idle_ms: opts.sim_idle_ms,
                    clock: opts.clock.clone(),
                    create_simulation: opts.create_simulation.clone(),
                    live: Some(LiveAppConfig {
                        settings: Arc::new(SettingsSource {
                            settings: settings.clone(),
                            connectors: connectors.clone(),
                        }),
                        state_root: opts.state_root.clone(),
                        defaults: opts.live_defaults.clone(),
                        harnesses: opts.harnesses.clone(),
                        app_version: opts.app_version.clone(),
                        config_for: Arc::new(move |slug| lock(&cfg).get(slug).cloned()),
                        env: None,
                        create_runtime: opts.create_runtime.clone(),
                        resolve_images: opts.resolve_images.clone(),
                    }),
                })
            }
        };

        let hub = {
            let (p, tx2) = (product.clone(), tx.clone());
            ConnectorsHub::new(HubOptions {
                store: connectors.clone(),
                github: opts.github.clone(),
                aws_profiles: connector_profiles(&aws_profiles),
                open_external: opts.open_external.clone(),
                clock: Arc::clone(&clock),
                invalidate: Arc::new(move || p.invalidate()),
                emit: Arc::new(move |e| {
                    let _ = tx2.send(e);
                }),
            })
        };

        let inner = Arc::new(Inner {
            ledger: ledger.clone(),
            views,
            product: product.clone(),
            settings: settings.clone(),
            tx: tx.clone(),
            ledger_path: opts.ledger_path.as_ref().map(|p| p.display().to_string()),
            github: opts.github.clone(),
            hub,
            connectors,
            env_login: Mutex::new(None),
            configs,
            phases: Mutex::default(),
            live_defaults: opts.live_defaults.clone(),
            harnesses: opts.harnesses.clone(),
            aws_profiles,
            probe: opts.connection_probe.clone(),
            idle_minutes: opts.idle_minutes,
            pump: Mutex::new(None),
            drafts: EnvDrafts::new(Arc::new(ProductDrafters(product.clone())), {
                let tx = tx.clone();
                Arc::new(move |e| {
                    let _ = tx.send(e);
                })
            }),
        });

        // engine events -> API events (and the sandbox phases the Environments screen shows)
        let mut rx = product.subscribe();
        let weak = Arc::downgrade(&inner);
        let pump = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        let Some(i) = weak.upgrade() else { break };
                        if let ratchet_core::ports::EngineEvent::Sandbox {
                            sandbox_id,
                            phase,
                            region,
                            text,
                        } = &e
                        {
                            lock(&i.phases).insert(
                                sandbox_id.clone(),
                                Phase {
                                    phase: *phase,
                                    region: region.clone(),
                                    text: text.clone(),
                                },
                            );
                        }
                        let _ = i.tx.send(ApiEvent::from(e));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        *lock(&inner.pump) = Some(pump);
        Ok(Self(inner))
    }

    pub fn ledger(&self) -> &Ledger {
        &self.0.ledger
    }

    pub fn views(&self) -> &LedgerViews {
        &self.0.views
    }

    pub fn product(&self) -> &ProductEngine {
        &self.0.product
    }

    pub fn settings_store(&self) -> &Arc<SettingsStore> {
        &self.0.settings
    }

    /// The saved connectors (tests read tokens and defaults through it; secrets never leave the daemon otherwise).
    pub fn connector_store(&self) -> &Arc<ConnectorStore> {
        &self.0.connectors
    }

    /// Stop the sign-in poll and the runtimes. Idempotent.
    pub async fn shutdown(&self) {
        self.0.hub.shutdown();
        self.0.product.dispose().await;
        if let Some(p) = lock(&self.0.pump).take() {
            p.abort();
        }
    }
}

impl Inner {
    fn engine(&self) -> EngineInfo {
        EngineInfo {
            kind: EngineKind::Real,
            mode: ratchet_core::enums::Mode::Live,
        }
    }

    fn paths(&self) -> LivePaths {
        let l = self.settings.live();
        effective_paths(&l.tofu_binary, &l.module_dir, &self.live_defaults)
    }

    /// The default AWS connector: what the single-profile Settings used to hold.
    fn default_aws(&self) -> Option<(String, String)> {
        self.connectors
            .default_of(ConnectorKind::Aws)
            .and_then(|r| match r.kind {
                RecordKind::Aws {
                    profile, region, ..
                } => Some((profile, region)),
                _ => None,
            })
    }

    fn aws_profile_name(&self) -> String {
        self.default_aws().map(|a| a.0).unwrap_or_default()
    }

    fn aws_region_name(&self) -> String {
        self.default_aws()
            .map(|a| a.1)
            .unwrap_or_else(|| self.settings.region())
    }

    fn secrets_present(&self) -> SecretsPresent {
        SecretsPresent {
            anthropic_api_key: self.connectors.has_secret(ConnectorSecret::AnthropicApiKey),
            openai_api_key: self.connectors.has_secret(ConnectorSecret::OpenaiApiKey),
            github_token: self.connectors.has_secret(ConnectorSecret::GithubToken),
        }
    }

    fn settings_vm(&self) -> SettingsVm {
        let paths = self.paths();
        let (profile, region) = (self.aws_profile_name(), self.aws_region_name());
        SettingsVm {
            aws_profile: profile.clone(),
            aws_region: region.clone(),
            secrets: self.secrets_present(),
            advanced: AdvancedSettingsVm {
                values: LiveSettings {
                    aws_profile: profile,
                    aws_region: region,
                    ..self.settings.live()
                },
                defaults: AdvancedDefaults {
                    tofu_binary: paths.tofu_binary,
                    module_dir: paths.module_dir,
                },
            },
            encryption_available: self.settings.encryption_available(),
            engine: self.engine(),
        }
    }

    /// The token of a GitHub connector (default: the default one), or the environment's when none was named.
    fn need_token(&self, connector_id: Option<&str>) -> ServiceResult<String> {
        self.connectors
            .github_token(connector_id)
            .or_else(|| {
                connector_id
                    .is_none()
                    .then(|| self.settings.env_secret(SecretName::GithubToken))
                    .flatten()
            })
            .ok_or_else(|| ServiceError::NotReady(SIGN_IN_FIRST.into()))
    }

    fn need_github(&self) -> ServiceResult<&Arc<dyn GithubApi>> {
        self.github.as_ref().ok_or_else(|| {
            ServiceError::Unsupported("GitHub is not available in this build.".into())
        })
    }

    /// The single-account profile call of the first Settings: the profile becomes the default AWS connector (added when it is
    /// new); an empty profile removes the default one.
    fn set_profile(&self, profile: &str) -> ServiceResult<()> {
        if profile.is_empty() {
            if let Some(def) = self.connectors.default_of(ConnectorKind::Aws) {
                self.connectors.remove(&def.id)?;
            }
            return Ok(());
        }
        let existing = self
            .connectors
            .snapshot()
            .into_iter()
            .find_map(|c| match c {
                ConnectorVm::Aws { id, profile: p, .. } if p == profile => Some(id),
                _ => None,
            });
        match existing {
            Some(id) => self.connectors.set_default(&id)?,
            None => {
                self.connectors.add_aws(
                    AddAwsConnectorInput {
                        profile: profile.to_owned(),
                        region: Some(self.settings.region()),
                        monthly_cap_usd: None,
                        name: None,
                    },
                    true,
                )?;
            }
        }
        // The auth kind follows the chosen profile (SSO sessions need `aws sso login`).
        let sso = (self.aws_profiles)()
            .profiles
            .iter()
            .any(|p| p.name == profile && p.kind == AwsProfileKind::Sso);
        self.settings.set_live(&LiveSettingsInput {
            aws_auth: Some(if sso {
                AwsAuthKind::Sso
            } else {
                AwsAuthKind::Profile
            }),
            ..Default::default()
        })
    }

    /// Remove the stored API-key connectors of an agent (the single key of the first Settings is replaced or cleared).
    fn remove_keys_of(&self, agent: ConnectorAgent) -> ServiceResult<()> {
        for c in self.connectors.snapshot() {
            if let ConnectorVm::Agent {
                id,
                agent: a,
                source: AgentConnectorSource::ApiKey,
                ..
            } = c
                && a == agent
            {
                self.connectors.remove(&id)?;
            }
        }
        Ok(())
    }

    /// The default GitHub account, as the old single-account row showed it.
    async fn github_status(&self) -> GithubStatusVm {
        let rec = self.connectors.default_of(ConnectorKind::Github);
        let token = self.connectors.github_token(None);
        let flow = self
            .hub
            .signin()
            .and_then(ratchet_github::signin::GithubSignIn::current)
            .map(flow_vm);
        let client_id_configured = self
            .github
            .as_ref()
            .is_some_and(|g| g.client_id_configured());
        let (Some(token), Some(rec)) = (token, rec) else {
            return GithubStatusVm {
                signed_in: false,
                login: None,
                source: None,
                client_id_configured,
                flow,
            };
        };
        let (mut login, stored) = match &rec.kind {
            RecordKind::Github { login, source } => {
                (Some(login.clone()).filter(|l| !l.is_empty()), Some(*source))
            }
            _ => (None, None),
        };
        let source = if rec.id == GITHUB_ENV_ID {
            GithubSource::Env
        } else {
            match stored {
                Some(GithubStored::Token) => GithubSource::Token,
                _ => GithubSource::Device,
            }
        };
        if login.is_none()
            && let Some(gh) = &self.github
        {
            let known = lock(&self.env_login).clone();
            let looked_up = match known {
                Some((t, l)) if t == token => l,
                _ => {
                    let l = gh.user(&token).await.ok().map(|u| u.login);
                    *lock(&self.env_login) = Some((token.clone(), l.clone()));
                    l
                }
            };
            login = looked_up;
        }
        GithubStatusVm {
            signed_in: true,
            login,
            source: Some(source),
            client_id_configured,
            flow,
        }
    }

    async fn environments_vm(&self) -> ServiceResult<EnvironmentsVm> {
        let rows = self.ledger.list_sandboxes().await?;
        let mut out = Vec::new();
        let mut last = None::<ratchet_core::ports::SandboxTeardownReceipt>;
        for r in &rows {
            let act = self.product.sandbox_activity(r.id.as_str()).await.ok();
            if let Some(a) = &act
                && let Some(rc) = &a.last_receipt
                && last
                    .as_ref()
                    .is_none_or(|l| rc.destroyed_at > l.destroyed_at)
            {
                last = Some(rc.clone());
            }
            if matches!(r.status, SandboxStatus::Destroyed | SandboxStatus::Planned) {
                continue;
            }
            let seen = lock(&self.phases)
                .get(r.id.as_str())
                .map(|p| (p.phase, p.region.clone(), p.text.clone()));
            let has_active = act.as_ref().is_some_and(|a| !a.active_runs.is_empty());
            let derived = derived_phase(r.status, has_active);
            let phase = match &seen {
                Some((
                    p @ (SandboxPhase::Failed | SandboxPhase::Destroying | SandboxPhase::Creating),
                    _,
                    _,
                )) => phase_vm(*p),
                _ => derived,
            };
            out.push(EnvironmentVm {
                sandbox_id: r.id.to_string(),
                region: act
                    .as_ref()
                    .and_then(|a| a.region.clone())
                    .or_else(|| Some(r.region.clone()).filter(|s| !s.is_empty()))
                    .or_else(|| seen.as_ref().and_then(|s| s.1.clone())),
                phase,
                text: seen
                    .as_ref()
                    .map(|s| s.2.clone())
                    .unwrap_or_else(|| match phase {
                        EnvironmentPhase::Idle => "idle: no run is active".into(),
                        EnvironmentPhase::Ready => "environment ready".into(),
                        other => format!("{other:?}").to_lowercase(),
                    }),
                simulated: r.provider == SandboxProviderKind::Mock,
                active_runs: act
                    .as_ref()
                    .map(|a| a.active_runs.iter().map(ToString::to_string).collect())
                    .unwrap_or_default(),
                instances_running: act.as_ref().and_then(|a| a.instances_running),
                spend_usd: act.as_ref().map_or(0.0, |a| a.spend_usd),
                destroy_at: act.as_ref().and_then(|a| a.destroy_at),
                idle_since: act.as_ref().and_then(|a| a.idle_since),
                can_end: r.status == SandboxStatus::Applied && !has_active && act.is_some(),
            });
        }
        Ok(EnvironmentsVm {
            environments: out,
            last_receipt: last.as_ref().map(receipt_vm),
            idle_minutes: self.idle_minutes,
        })
    }

    /// Parse a repo's ratchet.yaml for step 2 and remember its config for the region.
    fn inspect_result(
        &self,
        source: RepoSource,
        repo: Option<String>,
        base_ref: String,
        config_yaml: Option<String>,
        problem: Option<String>,
    ) -> RepoInspectVm {
        let (mut config, mut errors) = (None, Vec::new());
        if let Some(y) = &config_yaml {
            let v = crate::views::validate_config_text(y);
            if v.ok {
                config = v.summary;
                if let (Ok(parsed), Some(repo)) = (ratchet_core::parse_config(y), &repo)
                    && let Ok(json) = serde_json::to_value(&parsed)
                {
                    lock(&self.configs).insert(repo.clone(), json);
                }
            } else {
                errors = v.errors;
            }
        }
        let templates = if config_yaml.is_none() && problem.is_none() {
            ratchet_core::list_templates(TemplateFilter {
                kind: Some(ratchet_core::enums::TemplateKind::Template),
                category: None,
            })
        } else {
            Vec::new()
        };
        RepoInspectVm {
            source,
            repo,
            base_ref,
            config_yaml,
            config,
            errors,
            templates,
            problem,
        }
    }

    async fn harness_vms(&self, refresh: bool) -> Vec<HarnessVm> {
        let d = if refresh {
            let d = self.harnesses.refresh().await;
            self.product.invalidate();
            d
        } else {
            self.harnesses.get().await
        };
        d.harnesses.iter().map(harness_vm).collect()
    }

    async fn test_connection(&self, target: ConnectionTarget) -> ConnectionResultVm {
        let aws = self.default_aws();
        let need = |what: &str| (false, what.to_owned());
        let (mut ok, mut detail) = match target {
            ConnectionTarget::Github => match (
                &self.github,
                self.connectors.secret_value(ConnectorSecret::GithubToken),
            ) {
                (None, _) => need("GitHub is not available in this build."),
                (_, None) => need("Sign in with GitHub first."),
                (Some(gh), Some(token)) => match gh.user(&token).await {
                    Ok(u) => (true, format!("signed in as {}", u.login)),
                    Err(e) => (false, first_line(&e.to_string()).to_owned()),
                },
            },
            _ => match &self.probe {
                None => need("Connection tests are not available in this build."),
                Some(p) => match target {
                    ConnectionTarget::Aws => match &aws {
                        None => need("Choose an AWS profile first."),
                        Some((profile, region)) => p.aws(region, profile).await,
                    },
                    ConnectionTarget::Anthropic => {
                        match self
                            .connectors
                            .secret_value(ConnectorSecret::AnthropicApiKey)
                        {
                            Some(k) => p.anthropic(&k).await,
                            None => need("Store an Anthropic API key first."),
                        }
                    }
                    ConnectionTarget::Openai => {
                        match self.connectors.secret_value(ConnectorSecret::OpenaiApiKey) {
                            Some(k) => p.openai(&k).await,
                            None => need("Store an OpenAI API key first."),
                        }
                    }
                    ConnectionTarget::Github => unreachable!("handled above"),
                },
            },
        };
        // Belt and braces: no stored secret value may ever leave in a message, whatever the tester returned.
        for name in [
            ConnectorSecret::AnthropicApiKey,
            ConnectorSecret::OpenaiApiKey,
            ConnectorSecret::GithubToken,
        ] {
            if let Some(v) = self.connectors.secret_value(name)
                && v.chars().count() >= 4
            {
                detail = detail.replace(&v, "***");
            }
        }
        if target == ConnectionTarget::Aws
            && !ok
            && let Some((profile, _)) = &aws
            && (self.aws_profiles)()
                .profiles
                .iter()
                .any(|p| &p.name == profile && p.kind == AwsProfileKind::Sso)
        {
            detail.push_str(&format!(
                " Run \"aws sso login --profile {profile}\" in a terminal, then test again."
            ));
        }
        if detail.is_empty() {
            ok = false;
            detail = "failed".into();
        }
        ConnectionResultVm { target, ok, detail }
    }

    async fn preflight(&self, input: PreflightInput) -> Vec<PreflightItem> {
        let item = |id: &str, label: &str, state: PreflightState, detail: &str| PreflightItem {
            id: id.into(),
            label: label.into(),
            state,
            detail: detail.into(),
        };
        if input.source == PreflightSource::Demo {
            return vec![item(
                "demo",
                "Demo run",
                PreflightState::Ok,
                "simulation: scripted agents, mock AWS, nothing is installed or billed",
            )];
        }
        let mut items = Vec::new();
        // The connectors this repo's runs use: its remembered choice, else the defaults.
        let chosen = self.connectors.resolve(input.repo.as_deref()).await;
        items.push(match &chosen.github {
            Some(g) => item(
                "github",
                "GitHub",
                PreflightState::Ok,
                &g.login
                    .as_ref()
                    .map_or("signed in".into(), |l| format!("signed in as {l}")),
            ),
            None => item(
                "github",
                "Sign in with GitHub",
                PreflightState::Fail,
                "Ratchet pushes candidate branches and opens the pull request as you",
            ),
        });
        items.push(match &chosen.aws {
            Some(a) => item(
                "aws",
                "AWS",
                PreflightState::Ok,
                &format!("{} \u{00b7} {}", a.profile, a.region),
            ),
            None => item(
                "aws",
                "Connect AWS",
                PreflightState::Fail,
                "the evaluator runs in a sandbox in your AWS account",
            ),
        });
        let named = |k: AgentKind| {
            if k == AgentKind::Codex {
                "Codex"
            } else {
                "Claude Code"
            }
        };
        if input.agents_where == AgentsWhere::Local {
            let found = self.harnesses.get().await.harnesses;
            let kinds: Vec<AgentKind> = input
                .agents
                .iter()
                .copied()
                .filter(|a| matches!(a, AgentKind::ClaudeCode | AgentKind::Codex))
                .collect();
            if kinds.is_empty() {
                items.push(item(
                    "agents",
                    "Choose an agent",
                    PreflightState::Fail,
                    "no agent is selected",
                ));
            }
            for k in kinds {
                let label = named(k);
                let id = format!("agent-{}", k.as_str());
                let h = found.iter().find(|h| h.kind.agent_kind() == k);
                match h {
                    Some(h) if h.installed && h.auth != DetectAuth::SignedOut => items.push(item(
                        &id,
                        label,
                        PreflightState::Ok,
                        &format!(
                            "{} \u{00b7} {}",
                            h.version.as_deref().unwrap_or("installed"),
                            if h.auth == DetectAuth::ApiKey {
                                "API key"
                            } else {
                                "signed in"
                            }
                        ),
                    )),
                    other => items.push(item(
                        &id,
                        &format!("{label} is not ready"),
                        PreflightState::Fail,
                        &other.map_or_else(
                            || format!("install {label} on this machine"),
                            |h| h.hint.clone(),
                        ),
                    )),
                }
            }
        } else {
            for a in &input.agents {
                let (name, who) = match a {
                    AgentKind::ClaudeCode => (SecretName::AnthropicApiKey, "Anthropic"),
                    AgentKind::Codex => (SecretName::OpenaiApiKey, "OpenAI"),
                    _ => continue,
                };
                let id = format!("key-{}", a.as_str());
                let label = format!("{who} API key");
                let agent = match a {
                    AgentKind::Codex => ConnectorAgent::Codex,
                    _ => ConnectorAgent::ClaudeCode,
                };
                let has_key = chosen.cloud_keys.contains_key(&agent)
                    || self.settings.env_secret(name).is_some();
                items.push(if has_key {
                    item(&id, &label, PreflightState::Ok, "stored in the keychain")
                } else {
                    item(
                        &id,
                        &label,
                        PreflightState::Fail,
                        "cloud runners need an API key (Connectors)",
                    )
                });
            }
        }
        items
    }
}

fn harness_vm(h: &HarnessInfo) -> HarnessVm {
    HarnessVm {
        kind: match h.kind {
            DetectKind::ClaudeCode => HarnessKind::ClaudeCode,
            DetectKind::Codex => HarnessKind::Codex,
        },
        installed: h.installed,
        version: h.version.clone(),
        path: h.path.clone(),
        auth: match h.auth {
            DetectAuth::SignedIn => HarnessAuth::SignedIn,
            DetectAuth::ApiKey => HarnessAuth::ApiKey,
            DetectAuth::SignedOut => HarnessAuth::SignedOut,
        },
        hint: h.hint.clone(),
    }
}

fn validate_create(input: &CreateOptimizationInput) -> ServiceResult<()> {
    let mut fields = Vec::new();
    if input.agents.is_empty() {
        fields.push(FieldError::new("agents", "choose at least one agent"));
    }
    if !(input.budget_usd.is_finite() && input.budget_usd > 0.0 && input.budget_usd <= 100_000.0) {
        fields.push(FieldError::new("budgetUsd", "must be between 0 and 100000"));
    }
    if input.time_limit_seconds == 0 || input.time_limit_seconds > 14 * 24 * 3600 {
        fields.push(FieldError::new(
            "timeLimitSeconds",
            "must be between 1 s and 14 days",
        ));
    }
    if input.plateau_evals == 0 || input.plateau_evals > 10_000 {
        fields.push(FieldError::new(
            "plateauEvals",
            "must be between 1 and 10000",
        ));
    }
    if input.demo.is_none() && input.repo_url.trim().is_empty() {
        fields.push(FieldError::new("repoUrl", "repoUrl is required"));
    }
    if fields.is_empty() {
        Ok(())
    } else {
        Err(ServiceError::InvalidFields {
            message: "the optimization request is invalid".into(),
            fields,
        })
    }
}

#[async_trait]
impl AppService for RealService {
    fn engine(&self) -> EngineInfo {
        self.0.engine()
    }

    fn ledger_path(&self) -> Option<String> {
        self.0.ledger_path.clone()
    }

    fn subscribe(&self) -> broadcast::Receiver<ApiEvent> {
        self.0.tx.subscribe()
    }

    // ---- optimizations -----------------------------------------------------------------------------------

    async fn list_optimizations(&self, range: KpiRange) -> ServiceResult<OptimizationsSnapshot> {
        self.0.views.snapshot(range, self.0.engine()).await
    }

    async fn create_optimization(
        &self,
        input: CreateOptimizationInput,
    ) -> ServiceResult<CreatedOptimization> {
        validate_create(&input)?;
        if let Some(draft) = &input.environment_draft_id {
            self.0.drafts.require_approved(draft)?;
        }
        let base = EngineCreateOptimizationInput {
            name: input.name.clone(),
            repo_url: String::new(),
            base_ref: input.base_ref.clone(),
            config_path: None,
            agents: input.agents.clone(),
            budget_usd: input.budget_usd,
            time_limit_seconds: input.time_limit_seconds,
            plateau_evals: input.plateau_evals,
            sandbox_id: None,
            agents_where: None,
        };
        let id = if let Some(demo) = input.demo {
            let mut i = base;
            i.name = Some(
                input
                    .name
                    .clone()
                    .unwrap_or_else(|| demo_name(demo.as_str())),
            );
            i.repo_url = format!("acme/{}", demo.as_str());
            i.base_ref = "main".into();
            self.0.product.create_demo(demo.as_str(), &i).await
        } else {
            let slug = slug_of(&input.repo_url).ok_or_else(|| ServiceError::InvalidFields {
                message: format!(
                    "{} is not a GitHub repository (owner/name).",
                    input.repo_url
                ),
                fields: vec![FieldError::new("repoUrl", "use owner/name")],
            })?;
            let mut i = base;
            // The connectors picked for this run become the repo's remembered choice, and the runtime for the repo is built
            // from them; `agents` and `agentsWhere` follow them.
            let mut agents_where = input.agents_where;
            if let Some(c) = &input.connections {
                self.0.connectors.save_repo_connections(&slug, c.clone())?;
                agents_where = self.0.connectors.where_of(&c.agents)?.or(agents_where);
                let mut kinds: Vec<AgentKind> = Vec::new();
                for a in &c.agents {
                    if let Some(ConnectorVm::Agent { agent, .. }) =
                        self.0.connectors.vm_of(&a.connector_id)
                        && !kinds.contains(&agent.kind())
                    {
                        kinds.push(agent.kind());
                    }
                }
                i.agents = kinds;
                self.0.product.invalidate();
            }
            i.repo_url = slug;
            i.agents_where = agents_where;
            self.0.product.create_optimization(&i).await
        }
        .map_err(service_error)?;
        Ok(CreatedOptimization { id: id.to_string() })
    }

    async fn get_optimization(&self, id: &str) -> ServiceResult<OptimizationDetail> {
        let mut d = self.0.views.detail(id).await?;
        d.receipt = self.receipt(id).await.unwrap_or(None);
        d.calibration = self
            .calibration(id)
            .await
            .unwrap_or(None)
            .map(|c| c.explanation);
        Ok(d)
    }

    async fn start_optimization(&self, id: &str) -> ServiceResult<()> {
        self.0.product.start(id).await.map_err(service_error)
    }

    async fn stop_optimization(&self, id: &str, reason: Option<String>) -> ServiceResult<()> {
        self.0
            .product
            .stop(id, reason.as_deref())
            .await
            .map_err(service_error)
    }

    async fn ratchet_history(
        &self,
        id: &str,
        metric: Option<String>,
    ) -> ServiceResult<RatchetSeries> {
        self.0.views.series(id, metric.as_deref()).await
    }

    async fn winning_change(&self, id: &str) -> ServiceResult<WinningChangeVm> {
        let oid = OptimizationId::new(id);
        let opt = self
            .0
            .ledger
            .get_optimization(&oid)
            .await?
            .ok_or_else(|| ServiceError::not_found("optimization", id))?;
        let Some(best) = self.0.ledger.ratchet_history(&oid).await?.last().cloned() else {
            return self.0.views.winning_change(id, None, None).await;
        };
        let short = &best.sha[..best.sha.len().min(7)];
        match self
            .0
            .product
            .diff(&opt.repo, &opt.base_ref, &best.sha)
            .await
        {
            Ok(Some(files)) => self.0.views.winning_change(id, Some(files), None).await,
            Ok(None) => {
                self.0
                    .views
                    .winning_change(id, None, Some(format!("No diff available for {short}.")))
                    .await
            }
            Err(e) => {
                self.0
                    .views
                    .winning_change(
                        id,
                        None,
                        Some(format!(
                            "Could not diff {short} against {}: {}",
                            opt.base_ref,
                            first_line(&e.message)
                        )),
                    )
                    .await
            }
        }
    }

    async fn candidates(&self, id: &str) -> ServiceResult<Vec<CandidateVm>> {
        self.0.views.candidates(id).await
    }

    async fn spend(&self, id: &str) -> ServiceResult<SpendVm> {
        self.0.views.spend(id).await
    }

    async fn reconcile_costs(&self, id: &str) -> ServiceResult<CostReconcileReport> {
        self.0
            .product
            .reconcile_costs(id)
            .await
            .map_err(service_error)
    }

    async fn calibration(&self, id: &str) -> ServiceResult<Option<Calibration>> {
        self.0.product.calibration(id).await.map_err(service_error)
    }

    async fn protected_paths(&self, id: &str) -> ServiceResult<EffectiveProtected> {
        self.0
            .product
            .protected_paths(id)
            .await
            .map_err(service_error)
    }

    async fn receipt(&self, id: &str) -> ServiceResult<Option<TeardownReceiptVm>> {
        if self
            .0
            .ledger
            .get_optimization(&OptimizationId::new(id))
            .await?
            .is_none()
        {
            return Err(ServiceError::not_found("optimization", id));
        }
        Ok(self
            .0
            .product
            .teardown_receipt(id)
            .await
            .map_err(service_error)?
            .as_ref()
            .map(receipt_vm))
    }

    async fn open_pull_request(&self, id: &str) -> ServiceResult<PullRequestVm> {
        let pr = self
            .0
            .product
            .open_pull_request(id)
            .await
            .map_err(service_error)?;
        self.0.settings.set_pr_url(id, &pr.url);
        Ok(PullRequestVm { url: pr.url })
    }

    async fn report(&self, id: &str) -> ServiceResult<ReportVm> {
        self.0.views.report(id).await
    }

    async fn recent_activity(
        &self,
        optimization_id: Option<String>,
        limit: u32,
    ) -> ServiceResult<Vec<ActivityVm>> {
        self.0
            .views
            .activity(limit.clamp(1, 200) as usize, optimization_id.as_deref())
            .await
    }

    // ---- agents ------------------------------------------------------------------------------------------

    async fn agent_events(
        &self,
        agent_id: &str,
        cursor: Option<i64>,
        limit: u32,
    ) -> ServiceResult<AgentEventsPage> {
        self.0.views.agent_events(agent_id, cursor, limit).await
    }

    async fn pause_agent(&self, agent_id: &str) -> ServiceResult<()> {
        self.0
            .product
            .pause_agent(agent_id)
            .await
            .map_err(service_error)
    }

    async fn resume_agent(&self, agent_id: &str) -> ServiceResult<()> {
        self.0
            .product
            .resume_agent(agent_id)
            .await
            .map_err(service_error)
    }

    async fn message_agent(&self, agent_id: &str, text: &str) -> ServiceResult<()> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 4000 {
            return Err(invalid_field(
                "text",
                "the message must be 1 to 4000 characters".into(),
            ));
        }
        self.0
            .product
            .message_agent(agent_id, text)
            .await
            .map_err(service_error)
    }

    // ---- config and preflight ----------------------------------------------------------------------------

    async fn preflight(&self, input: PreflightInput) -> ServiceResult<Vec<PreflightItem>> {
        Ok(self.0.preflight(input).await)
    }

    // ---- settings ----------------------------------------------------------------------------------------

    async fn settings(&self) -> ServiceResult<SettingsVm> {
        Ok(self.0.settings_vm())
    }

    async fn set_aws_profile(&self, profile: &str) -> ServiceResult<SettingsVm> {
        if let Some(problem) = validate_profile_name(profile) {
            return Err(invalid_field("profile", problem));
        }
        self.0.set_profile(profile.trim())?;
        self.0.product.invalidate();
        Ok(self.0.settings_vm())
    }

    async fn save_advanced_settings(&self, input: LiveSettingsInput) -> ServiceResult<SettingsVm> {
        let LiveSettingsInput { aws_profile, .. } = input.clone();
        if let Some(p) = &aws_profile
            && let Some(problem) = validate_profile_name(p)
        {
            return Err(invalid_field("awsProfile", problem));
        }
        let region = input.aws_region.clone();
        // validates the whole batch first; the profile lives in the connectors
        self.0.settings.set_live(&LiveSettingsInput {
            aws_profile: None,
            ..input
        })?;
        if let Some(p) = &aws_profile {
            self.0.set_profile(p.trim())?;
        }
        if let Some(r) = region.as_deref().map(str::trim).filter(|r| !r.is_empty())
            && let Some(def) = self.0.connectors.default_of(ConnectorKind::Aws)
        {
            self.0.connectors.update(
                &def.id,
                UpdateConnectorInput {
                    region: Some(r.to_owned()),
                    ..Default::default()
                },
            )?;
        }
        self.0.product.invalidate();
        Ok(self.0.settings_vm())
    }

    async fn set_secret(&self, name: SecretName, value: &str) -> ServiceResult<SettingsVm> {
        if let Some(problem) = validate_secret_format(name, value) {
            return Err(invalid_field("value", problem));
        }
        if name == SecretName::GithubToken {
            // A pasted token (Advanced): prove it works and learn the account before keeping it; it becomes the default
            // GitHub account.
            self.0
                .connectors
                .add_github_token(value, None, true)
                .await
                .map_err(|e| match e {
                    ConnectorError::Invalid(m) => ServiceError::Upstream(m),
                    other => other.into(),
                })?;
            *lock(&self.0.env_login) = None;
        } else {
            // The single key of the first Settings: replaces the agent's stored key connector (or adds one).
            let agent = if name == SecretName::AnthropicApiKey {
                ConnectorAgent::ClaudeCode
            } else {
                ConnectorAgent::Codex
            };
            self.0.remove_keys_of(agent)?;
            self.0
                .connectors
                .add_agent(
                    AddAgentConnectorInput::ApiKey {
                        agent,
                        api_key: value.to_owned(),
                        name: None,
                    },
                    true,
                )
                .await?;
        }
        self.0.product.invalidate();
        if name == SecretName::GithubToken {
            let _ = self.0.tx.send(ApiEvent::Github { signed_in: true });
        }
        Ok(self.0.settings_vm())
    }

    async fn clear_secret(&self, name: SecretName) -> ServiceResult<SettingsVm> {
        if name == SecretName::GithubToken {
            if let Some(rec) = self.0.connectors.default_of(ConnectorKind::Github)
                && rec.id != GITHUB_ENV_ID
            {
                self.0.connectors.remove(&rec.id)?;
            }
        } else {
            let agent = if name == SecretName::AnthropicApiKey {
                ConnectorAgent::ClaudeCode
            } else {
                ConnectorAgent::Codex
            };
            self.0.remove_keys_of(agent)?;
        }
        self.0.product.invalidate();
        if name == SecretName::GithubToken {
            let _ = self.0.tx.send(ApiEvent::Github { signed_in: false });
        }
        Ok(self.0.settings_vm())
    }

    async fn test_connection(&self, target: ConnectionTarget) -> ServiceResult<ConnectionResultVm> {
        Ok(self.0.test_connection(target).await)
    }

    // ---- GitHub, repos -----------------------------------------------------------------------------------

    async fn github_status(&self) -> ServiceResult<GithubStatusVm> {
        Ok(self.0.github_status().await)
    }

    async fn github_sign_in(&self) -> ServiceResult<GithubStatusVm> {
        let signin = self.0.hub.need_signin()?;
        signin.start().await.map_err(github_error)?;
        Ok(self.0.github_status().await)
    }

    async fn github_sign_out(&self) -> ServiceResult<GithubStatusVm> {
        self.0.hub.shutdown();
        if let Some(rec) = self.0.connectors.default_of(ConnectorKind::Github)
            && rec.id != GITHUB_ENV_ID
        {
            self.0.connectors.remove(&rec.id)?;
        }
        *lock(&self.0.env_login) = None;
        self.0.product.invalidate();
        let _ = self.0.tx.send(ApiEvent::Github { signed_in: false });
        Ok(self.0.github_status().await)
    }

    async fn list_repos(
        &self,
        query: Option<String>,
        connector_id: Option<String>,
    ) -> ServiceResult<Vec<RepoVm>> {
        let token = self.0.need_token(connector_id.as_deref())?;
        let gh = self.0.need_github()?;
        Ok(gh
            .repos(&token, query.as_deref())
            .await
            .map_err(github_error)?
            .into_iter()
            .map(|r| RepoVm {
                full_name: r.full_name,
                default_branch: r.default_branch,
                private: r.private,
                description: r.description,
                pushed_at: r.pushed_at,
            })
            .collect())
    }

    async fn inspect_repo(
        &self,
        source: RepoSource,
        connector_id: Option<String>,
    ) -> ServiceResult<RepoInspectVm> {
        match &source {
            RepoSource::Github { repo } => {
                let token = self.0.need_token(connector_id.as_deref())?;
                let gh = self.0.need_github()?;
                let i = ratchet_github::api::inspect_github_repo(gh.as_ref(), &token, repo)
                    .await
                    .map_err(github_error)?;
                Ok(self.0.inspect_result(
                    source.clone(),
                    Some(i.repo.full_name),
                    i.repo.default_branch,
                    i.config_yaml,
                    None,
                ))
            }
            RepoSource::Local { path } => {
                let info = ratchet_github::inspect_local_folder(Path::new(path)).await;
                Ok(self.0.inspect_result(
                    source.clone(),
                    info.repo,
                    info.base_ref,
                    info.config_yaml,
                    info.problem,
                ))
            }
        }
    }

    // ---- AWS, agent CLIs ---------------------------------------------------------------------------------

    async fn aws_profiles(&self) -> ServiceResult<AwsProfilesVm> {
        let found = (self.0.aws_profiles)();
        Ok(AwsProfilesVm {
            profiles: found.profiles,
            sso_sessions: found.sso_sessions,
            selected: self.0.aws_profile_name(),
            region: self.0.aws_region_name(),
            default_region: DEFAULT_AWS_REGION.into(),
            files_found: found.files_found,
        })
    }

    async fn harnesses(&self, refresh: bool) -> ServiceResult<Vec<HarnessVm>> {
        Ok(self.0.harness_vms(refresh).await)
    }

    // ---- templates ---------------------------------------------------------------------------------------

    async fn propose_template(
        &self,
        input: ProposeTemplateInput,
    ) -> ServiceResult<ProposeTemplateResult> {
        self.0.need_token(None)?;
        let slug = slug_of(&input.repo_url).ok_or_else(|| ServiceError::InvalidFields {
            message: format!(
                "{} is not a GitHub repository (owner/name).",
                input.repo_url
            ),
            fields: vec![FieldError::new("repoUrl", "use owner/name")],
        })?;
        let mut i = input;
        i.repo_url = slug;
        self.0
            .product
            .propose_template(&i)
            .await
            .map_err(service_error)
    }

    // ---- overview ----------------------------------------------------------------------------------------

    async fn overview(&self, range: OverviewRange) -> ServiceResult<OverviewVm> {
        self.0.views.overview(range).await
    }

    // ---- connectors --------------------------------------------------------------------------------------

    async fn list_connectors(&self) -> ServiceResult<ConnectorsVm> {
        Ok(self.0.hub.view().await)
    }

    async fn refresh_connectors(&self) -> ServiceResult<ConnectorsVm> {
        self.0.hub.refresh().await
    }

    async fn add_github_connector(
        &self,
        input: AddGithubConnectorInput,
    ) -> ServiceResult<ConnectorsVm> {
        let vm = self.0.hub.add_github(input).await?;
        *lock(&self.0.env_login) = None;
        Ok(vm)
    }

    async fn start_github_connector_sign_in(
        &self,
        input: StartGithubSignInInput,
    ) -> ServiceResult<ConnectorsVm> {
        self.0.hub.start_sign_in(input).await
    }

    async fn cancel_github_connector_sign_in(&self) -> ServiceResult<ConnectorsVm> {
        self.0.hub.cancel_sign_in().await
    }

    async fn add_aws_connector(&self, input: AddAwsConnectorInput) -> ServiceResult<ConnectorsVm> {
        self.0.hub.add_aws(input).await
    }

    async fn add_agent_connector(
        &self,
        input: AddAgentConnectorInput,
    ) -> ServiceResult<ConnectorsVm> {
        self.0.hub.add_agent(input).await
    }

    async fn update_connector(
        &self,
        id: &str,
        input: UpdateConnectorInput,
    ) -> ServiceResult<ConnectorsVm> {
        self.0.hub.update(id, input).await
    }

    async fn remove_connector(&self, id: &str) -> ServiceResult<ConnectorsVm> {
        self.0.hub.remove(id).await
    }

    async fn set_default_connector(&self, id: &str) -> ServiceResult<ConnectorsVm> {
        self.0.hub.set_default(id).await
    }

    async fn test_connector(&self, id: &str) -> ServiceResult<ConnectorTestVm> {
        self.0.hub.test(id).await
    }

    async fn repo_connections(&self, repo: &str) -> ServiceResult<RepoConnectionsVm> {
        self.0.hub.repo_connections(repo).await
    }

    async fn save_repo_connections(
        &self,
        repo: &str,
        selection: RepoConnections,
    ) -> ServiceResult<RepoConnectionsVm> {
        self.0.hub.save_repo_connections(repo, selection).await
    }

    // ---- environment drafts ------------------------------------------------------------------------------

    async fn create_environment_draft(
        &self,
        mut input: CreateEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        self.0.need_token(None)?;
        input.repo = slug_of(&input.repo).ok_or_else(|| ServiceError::InvalidFields {
            message: format!("{} is not a GitHub repository (owner/name).", input.repo),
            fields: vec![FieldError::new("repo", "use owner/name")],
        })?;
        if input.aws_profile.is_none() {
            input.aws_profile = Some(self.0.aws_profile_name()).filter(|p| !p.is_empty());
        }
        self.0.drafts.create(input).await
    }

    async fn list_environment_drafts(&self) -> ServiceResult<EnvironmentDraftsVm> {
        Ok(EnvironmentDraftsVm {
            drafts: self.0.drafts.list(),
        })
    }

    async fn get_environment_draft(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        self.0.drafts.get(id)
    }

    async fn discard_environment_draft(&self, id: &str) -> ServiceResult<()> {
        self.0.drafts.discard(id)
    }

    async fn revise_environment_draft(
        &self,
        id: &str,
        input: ReviseEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        self.0.drafts.revise(id, input).await
    }

    async fn approve_environment_draft(
        &self,
        id: &str,
        input: ApproveEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        self.0.drafts.approve(id, input).await
    }

    // ---- environments ------------------------------------------------------------------------------------

    async fn environments(&self) -> ServiceResult<EnvironmentsVm> {
        self.0.environments_vm().await
    }

    async fn end_environment(&self, sandbox_id: &str) -> ServiceResult<EnvironmentsVm> {
        if self.0.ledger.get_sandbox(sandbox_id).await?.is_none() {
            return Err(ServiceError::not_found("environment", sandbox_id));
        }
        self.0
            .product
            .end_sandbox(sandbox_id)
            .await
            .map_err(service_error)?;
        self.0.environments_vm().await
    }
}
