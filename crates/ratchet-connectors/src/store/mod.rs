//! Saved connectors: several GitHub accounts, AWS profiles and agent credentials, one default per kind, a remembered choice per
//! repo. Port of `packages/desktop/src/main/connectors.ts` (+ `connector-records.ts`).
//!
//! Metadata is kept in a small JSON file; tokens and keys go to the [`SecretStore`] (the Keychain on macOS). Secrets only ever
//! leave through [`ConnectorStore::resolve`], [`ConnectorStore::github_token`] and [`ConnectorStore::secret_value`], which the
//! daemon hands to the engine; no view model carries one.

mod records;
mod resolve;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use ratchet_agents::detect::{HarnessAuth, HarnessInfo, HarnessKind};
use ratchet_core::enums::AgentsWhere;
use ratchet_core::ports::Clock;
use ratchet_core::ports::mock::SystemClock;
use ratchet_github::GithubApi;
use regex::Regex;

pub use records::{ConnectorRecord, ConnectorsPersisted, GithubStored, RecordKind};
pub use resolve::{ResolvedAgent, ResolvedAws, ResolvedConnections, ResolvedGithub};

use crate::model::{
    AddAgentConnectorInput, AddAwsConnectorInput, AgentConnectorSource, AgentSelection,
    AwsAuthStyle, ConnectorAgent, ConnectorKind, ConnectorStatus, ConnectorVm,
    EffectiveConnections, GithubConnectorSource, RepoConnections, RepoConnectionsVm,
    UpdateConnectorInput, repo_key,
};
use crate::secrets::{SecretError, SecretStore};
use crate::validate::{
    SecretName, validate_aws_profile, validate_aws_region, validate_secret_format,
};

pub const DEFAULT_AWS_REGION: &str = "us-east-1";
/// The connector made of `GITHUB_TOKEN` / `GH_TOKEN` in the environment (not stored, not removable).
pub const GITHUB_ENV_ID: &str = "github-env";
const AGENTS: [ConnectorAgent; 3] = [
    ConnectorAgent::ClaudeCode,
    ConnectorAgent::Codex,
    ConnectorAgent::Grok,
];
const MAX_CAP_USD: f64 = 10_000_000.0;
/// `Number.MAX_SAFE_INTEGER`: the environment connector sorts after every stored one.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

fn secret_for(agent: ConnectorAgent) -> SecretName {
    match agent {
        ConnectorAgent::ClaudeCode => SecretName::AnthropicApiKey,
        ConnectorAgent::Codex => SecretName::OpenaiApiKey,
        ConnectorAgent::Grok => SecretName::XaiApiKey,
    }
}

fn login_command(agent: ConnectorAgent) -> &'static str {
    match agent {
        ConnectorAgent::ClaudeCode => "`claude`",
        ConnectorAgent::Codex => "`codex login`",
        ConnectorAgent::Grok => "`grok login`",
    }
}

fn agent_of(kind: HarnessKind) -> ConnectorAgent {
    match kind {
        HarnessKind::ClaudeCode => ConnectorAgent::ClaudeCode,
        HarnessKind::Codex => ConnectorAgent::Codex,
        HarnessKind::Grok => ConnectorAgent::Grok,
    }
}

fn login_id(agent: ConnectorAgent) -> String {
    format!(
        "agent-{}-login",
        match agent {
            ConnectorAgent::ClaudeCode => "claude-code",
            ConnectorAgent::Codex => "codex",
            ConnectorAgent::Grok => "grok",
        }
    )
}

fn agent_slug(agent: ConnectorAgent) -> &'static str {
    match agent {
        ConnectorAgent::ClaudeCode => "claude-code",
        ConnectorAgent::Codex => "codex",
        ConnectorAgent::Grok => "grok",
    }
}

/// `prefix-` and four random bytes in hex.
fn new_id(prefix: &str) -> String {
    let b: [u8; 4] = rand::random();
    format!("{prefix}-{:02x}{:02x}{:02x}{:02x}", b[0], b[1], b[2], b[3])
}

fn secret_account(id: &str) -> String {
    format!("connector:{id}")
}

/// A probe that failed because the network or the service was out of reach says nothing about the account.
fn unreachable(detail: &str) -> bool {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)ENOTFOUND|EAI_AGAIN|ECONNREFUSED|ECONNRESET|ETIMEDOUT|getaddrinfo|fetch failed|network|timed out|timeout|could not connect|could not reach|answered 5\d\d|error sending request|dns error|connection refused|connection reset",
        )
        .expect("regex")
    })
    .is_match(detail)
}

fn scrub(text: &str, secrets: &[Option<String>]) -> String {
    secrets
        .iter()
        .flatten()
        .filter(|s| s.len() >= 4)
        .fold(text.to_owned(), |t, s| t.replace(s.as_str(), "***"))
}

/// `1234.5` as `1,234.5` (what `toLocaleString('en-US')` prints; a cap has at most two decimals).
fn usd_en_us(v: f64) -> String {
    let mut s = format!("{v:.2}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(i, f)| (i, f));
    let mut out = String::new();
    for (i, c) in int.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 && c != '-' {
            out.push(',');
        }
        out.push(c);
    }
    let int: String = out.chars().rev().collect();
    if frac.is_empty() {
        int
    } else {
        format!("{int}.{frac}")
    }
}

// ------------------------------------------------------------------ errors

/// Why a connector call failed. The messages are the user-facing wording of the TypeScript `ConnectorError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorError {
    /// "That connector no longer exists."
    #[error("{0}")]
    NotFound(String),
    /// A bad name, region, cap, token or choice.
    #[error("{0}")]
    Invalid(String),
    /// The keychain refused (unavailable, or the write failed).
    #[error("{0}")]
    Keychain(String),
}

pub type Result<T, E = ConnectorError> = std::result::Result<T, E>;

fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(ConnectorError::Invalid(msg.into()))
}

fn gone() -> ConnectorError {
    ConnectorError::NotFound("That connector no longer exists.".into())
}

impl From<SecretError> for ConnectorError {
    fn from(e: SecretError) -> Self {
        Self::Keychain(e.to_string())
    }
}

// ------------------------------------------------------------------ dependencies

/// The detected agent CLIs ("your login" connectors). Implemented by the engine's `HarnessCache`.
#[async_trait]
pub trait HarnessSource: Send + Sync {
    /// The cached result; probes once if nothing was probed yet.
    async fn get(&self) -> Vec<HarnessInfo>;
    /// Probe again.
    async fn refresh(&self) -> Vec<HarnessInfo>;
}

/// "Test connection" probes (the runtime's `ConnectionTester` in production).
#[async_trait]
pub trait ConnectorProbe: Send + Sync {
    async fn aws(&self, region: &str, profile: &str) -> (bool, String);
    async fn anthropic(&self, api_key: &str) -> (bool, String);
    async fn openai(&self, api_key: &str) -> (bool, String);
    async fn xai(&self, api_key: &str) -> (bool, String);
}

/// A profile of `~/.aws` (names and kinds only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsProfile {
    pub name: String,
    pub kind: AwsAuthStyle,
}

/// The profiles in `~/.aws` (names only).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AwsProfileList {
    pub profiles: Vec<AwsProfile>,
    /// At least one of the files exists.
    pub files_found: bool,
}

pub type AwsProfileLister = Arc<dyn Fn() -> AwsProfileList + Send + Sync>;

/// The single GitHub token / AWS profile / API keys of the first versions of Settings, read once to become connectors.
#[derive(Debug, Clone, Default)]
pub struct LegacyAccounts {
    pub aws_profile: String,
    pub aws_region: String,
    pub github_login: String,
    /// `Some(true)` = the token came from the device flow, `Some(false)` = pasted.
    pub github_via_device: Option<bool>,
    pub secrets: BTreeMap<SecretName, String>,
    /// A secret is stored but cannot be read right now (keychain locked): migration waits for the next launch instead of
    /// dropping it.
    pub locked: bool,
}

pub trait LegacySource: Send + Sync {
    fn read(&self) -> LegacyAccounts;
    /// The old fields are emptied (and the old secrets deleted) once they became connectors.
    fn clear(&self);
}

#[derive(Default, Clone)]
pub struct ConnectorDeps {
    pub github: Option<Arc<dyn GithubApi>>,
    /// Detected agent CLIs. Without it none are found.
    pub harnesses: Option<Arc<dyn HarnessSource>>,
    /// The profiles in `~/.aws` (names only).
    pub aws_profiles: Option<AwsProfileLister>,
    /// Probes for AWS profiles and API keys. Without it those connectors keep the status they were given.
    pub probe: Option<Arc<dyn ConnectorProbe>>,
    /// The old single-account settings, migrated once.
    pub legacy: Option<Arc<dyn LegacySource>>,
    pub clock: Option<Arc<dyn Clock>>,
}

// ------------------------------------------------------------------ the store

struct State {
    data: ConnectorsPersisted,
    statuses: HashMap<String, ConnectorStatus>,
    harness: Vec<HarnessInfo>,
}

pub struct ConnectorStore {
    file: Option<PathBuf>,
    secrets: Arc<dyn SecretStore>,
    env: HashMap<String, String>,
    deps: ConnectorDeps,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

/// What `test` reports: the TS `{ ok, detail }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestOutcome {
    pub ok: bool,
    pub detail: String,
}

/// A GitHub token to keep (`ConnectorStore::upsert_github`).
#[derive(Debug, Clone)]
pub struct UpsertGithub {
    pub login: String,
    pub token: String,
    pub via_device: bool,
    /// Re-sign-in: store the token on this connector (and follow the login if it changed).
    pub connector_id: Option<String>,
    pub name: Option<String>,
    pub make_default: bool,
}

impl UpsertGithub {
    pub fn new(login: impl Into<String>, token: impl Into<String>, via_device: bool) -> Self {
        Self {
            login: login.into(),
            token: token.into(),
            via_device,
            connector_id: None,
            name: None,
            make_default: false,
        }
    }
}

impl ConnectorStore {
    /// `file`: where the metadata lives (`None` = memory only). `env`: the environment credentials are also read from.
    /// Migrates the old single-account settings once.
    pub fn new(
        file: Option<PathBuf>,
        secrets: Arc<dyn SecretStore>,
        env: HashMap<String, String>,
        deps: ConnectorDeps,
    ) -> Self {
        let data = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            // a corrupt file: start from nothing rather than crash the app
            .and_then(|t| serde_json::from_str::<ConnectorsPersisted>(&t).ok())
            .unwrap_or_default();
        let clock = deps.clock.clone().unwrap_or_else(|| Arc::new(SystemClock));
        let store = Self {
            file,
            secrets,
            env,
            deps,
            clock,
            state: Mutex::new(State {
                data,
                statuses: HashMap::new(),
                harness: Vec::new(),
            }),
        };
        store.migrate();
        store
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> i64 {
        self.clock.now()
    }

    /// False when the keychain is unavailable: tokens and keys cannot be stored (the environment is still honoured).
    pub fn encryption_available(&self) -> bool {
        self.secrets.available()
    }

    fn save(&self, st: &State) {
        let Some(file) = &self.file else { return };
        let text = serde_json::to_string_pretty(&st.data).unwrap_or_default();
        let write = || -> std::io::Result<()> {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = file.with_extension("json.tmp");
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            std::io::Write::write_all(&mut opts.open(&tmp)?, text.as_bytes())?;
            std::fs::rename(&tmp, file)
        };
        if let Err(e) = write() {
            tracing::warn!("cannot save the connectors to {}: {e}", file.display());
        }
    }

    // ------------------------------------------------------------------ environment and secrets

    fn env_secret(&self, name: SecretName) -> Option<String> {
        name.env_vars()
            .iter()
            .find_map(|k| self.env.get(*k).filter(|v| !v.is_empty()).cloned())
    }

    fn set_secret(&self, id: &str, value: &str) -> Result<()> {
        if !self.secrets.available() {
            return Err(ConnectorError::Keychain(
                SecretError::Unavailable.to_string(),
            ));
        }
        Ok(self.secrets.set(&secret_account(id), value)?)
    }

    fn get_secret(&self, id: &str) -> Option<String> {
        self.secrets.get(&secret_account(id)).ok().flatten()
    }

    fn clear_secret(&self, id: &str) {
        if let Err(e) = self.secrets.delete(&secret_account(id)) {
            tracing::warn!("cannot delete the secret of {id}: {e}");
        }
    }

    fn is_env(r: &ConnectorRecord) -> bool {
        r.id == GITHUB_ENV_ID
    }

    /// Secret of a stored connector, or the environment's token for the environment connector.
    fn secret_of(&self, r: &ConnectorRecord) -> Option<String> {
        if Self::is_env(r) {
            self.env_secret(SecretName::GithubToken)
        } else {
            self.get_secret(&r.id)
        }
    }

    // ------------------------------------------------------------------ migration

    /// The single GitHub token, AWS profile and API keys of the first versions become connectors (once).
    fn migrate(&self) {
        let mut st = self.st();
        if st.data.migrated {
            return;
        }
        let old = self
            .deps
            .legacy
            .as_ref()
            .map(|l| l.read())
            .unwrap_or_default();
        if old.locked {
            return; // a secret we cannot read right now: try again on the next launch rather than lose it
        }
        let t = self.now();
        let mut items: Vec<ConnectorRecord> = Vec::new();
        let mut defaults: BTreeMap<ConnectorKind, String> = BTreeMap::new();
        let mut written: Vec<String> = Vec::new();
        let fail = |id: &str, e: &ConnectorError| {
            tracing::warn!("cannot migrate {id} to the keychain: {e}");
        };
        if let Some(token) = old.secrets.get(&SecretName::GithubToken) {
            let id = new_id("github");
            match self.set_secret(&id, token) {
                Ok(()) => {
                    written.push(id.clone());
                    let login = old.github_login.clone();
                    items.push(ConnectorRecord {
                        id: id.clone(),
                        name: if login.is_empty() {
                            "GitHub".into()
                        } else {
                            login.clone()
                        },
                        created_at: t,
                        kind: RecordKind::Github {
                            login,
                            source: if old.github_via_device == Some(true) {
                                GithubStored::Device
                            } else {
                                GithubStored::Token
                            },
                        },
                    });
                    defaults.insert(ConnectorKind::Github, id);
                }
                Err(e) => return fail(&id, &e),
            }
        }
        if !old.aws_profile.is_empty() {
            let id = new_id("aws");
            items.push(ConnectorRecord {
                id: id.clone(),
                name: old.aws_profile.clone(),
                created_at: t,
                kind: RecordKind::Aws {
                    profile: old.aws_profile.clone(),
                    region: if old.aws_region.is_empty() {
                        DEFAULT_AWS_REGION.into()
                    } else {
                        old.aws_region.clone()
                    },
                    monthly_cap_usd: None,
                },
            });
            defaults.insert(ConnectorKind::Aws, id);
        }
        for agent in AGENTS {
            let Some(key) = old.secrets.get(&secret_for(agent)) else {
                continue;
            };
            let id = new_id(&format!("agent-{}", agent_slug(agent)));
            if let Err(e) = self.set_secret(&id, key) {
                for w in &written {
                    self.clear_secret(w);
                }
                return fail(&id, &e);
            }
            items.push(ConnectorRecord {
                id,
                name: format!("{} · API key", agent.label()),
                created_at: t,
                kind: RecordKind::Agent {
                    agent,
                    source: AgentConnectorSource::ApiKey,
                    key_hint: Some(key_hint(key)),
                },
            });
        }
        st.data.items.extend(items);
        st.data.defaults.extend(defaults);
        st.data.migrated = true;
        if let Some(l) = &self.deps.legacy {
            l.clear();
        }
        self.save(&st);
    }

    // ------------------------------------------------------------------ records

    /// Stored connectors, plus the GitHub token from the environment when there is one (not persisted, not removable).
    fn records(&self, st: &State) -> Vec<ConnectorRecord> {
        let mut out = st.data.items.clone();
        if self.env_secret(SecretName::GithubToken).is_some() {
            out.push(ConnectorRecord {
                id: GITHUB_ENV_ID.into(),
                name: "GitHub (environment)".into(),
                created_at: MAX_SAFE_INTEGER,
                kind: RecordKind::Github {
                    login: String::new(),
                    source: GithubStored::Device,
                },
            });
        }
        out
    }

    fn find(&self, st: &State, id: &str) -> Option<ConnectorRecord> {
        self.records(st).into_iter().find(|r| r.id == id)
    }

    fn need(&self, st: &State, id: &str) -> Result<ConnectorRecord> {
        self.find(st, id).ok_or_else(gone)
    }

    /// Detected CLI logins become connectors the first time they are seen (unless the user removed them).
    async fn sync(&self) {
        let Some(h) = &self.deps.harnesses else {
            return;
        };
        let found = h.get().await;
        let mut st = self.st();
        st.harness = found;
        self.add_detected_logins(&mut st);
    }

    fn add_detected_logins(&self, st: &mut State) {
        let mut changed = false;
        for h in st.harness.clone() {
            if !h.installed {
                continue;
            }
            let agent = agent_of(h.kind);
            let id = login_id(agent);
            if st.data.items.iter().any(|r| r.id == id) || st.data.dismissed.contains(&id) {
                continue;
            }
            st.data.items.push(ConnectorRecord {
                id,
                name: format!("{} · your login", agent.label()),
                created_at: self.now(),
                kind: RecordKind::Agent {
                    agent,
                    source: AgentConnectorSource::Login,
                    key_hint: None,
                },
            });
            changed = true;
        }
        if changed {
            self.save(st);
        }
    }

    fn harness_of(st: &State, agent: ConnectorAgent) -> Option<HarnessInfo> {
        st.harness
            .iter()
            .find(|h| agent_of(h.kind) == agent)
            .cloned()
    }

    fn aws_list(&self) -> Option<AwsProfileList> {
        self.deps.aws_profiles.as_ref().map(|f| f())
    }

    // ------------------------------------------------------------------ status

    fn status_of(&self, st: &State, r: &ConnectorRecord) -> ConnectorStatus {
        let probed = st.statuses.get(&r.id).copied();
        match &r.kind {
            RecordKind::Github { .. } => {
                if self.secret_of(r).is_some() {
                    probed.unwrap_or(ConnectorStatus::Connected)
                } else {
                    ConnectorStatus::SignedOut
                }
            }
            RecordKind::Aws { profile, .. } => {
                if let Some(p) = probed {
                    return p;
                }
                match self.aws_list() {
                    Some(l) if l.files_found && !l.profiles.iter().any(|p| &p.name == profile) => {
                        ConnectorStatus::Invalid
                    }
                    _ => ConnectorStatus::Connected,
                }
            }
            RecordKind::Agent { agent, source, .. } => match source {
                AgentConnectorSource::ApiKey => {
                    if self.secret_of(r).is_some() {
                        probed.unwrap_or(ConnectorStatus::Valid)
                    } else {
                        ConnectorStatus::Invalid
                    }
                }
                AgentConnectorSource::Login => match Self::harness_of(st, *agent) {
                    Some(h) if h.installed && h.auth != HarnessAuth::SignedOut => {
                        ConnectorStatus::Connected
                    }
                    _ => ConnectorStatus::SignedOut,
                },
            },
        }
    }

    // ------------------------------------------------------------------ defaults

    /// The stored default if it still exists; else the first connector (for agents: the first usable one, logins first).
    fn default_id(&self, st: &State, kind: ConnectorKind) -> Option<String> {
        let recs: Vec<ConnectorRecord> = self
            .records(st)
            .into_iter()
            .filter(|r| r.connector_kind() == kind)
            .collect();
        if let Some(stored) = st.data.defaults.get(&kind)
            && recs.iter().any(|r| &r.id == stored)
        {
            return Some(stored.clone());
        }
        if kind != ConnectorKind::Agent {
            return recs.first().map(|r| r.id.clone());
        }
        let usable: Vec<ConnectorRecord> = recs
            .iter()
            .filter(|r| {
                matches!(
                    self.status_of(st, r),
                    ConnectorStatus::Connected | ConnectorStatus::Valid
                )
            })
            .cloned()
            .collect();
        sort_agents(usable)
            .into_iter()
            .next()
            .or_else(|| sort_agents(recs).into_iter().next())
            .map(|r| r.id)
    }

    pub fn set_default(&self, id: &str) -> Result<()> {
        let mut st = self.st();
        let r = self.need(&st, id)?;
        st.data.defaults.insert(r.connector_kind(), id.to_owned());
        self.save(&st);
        Ok(())
    }

    /// The default connector of a kind, or `None`.
    pub fn default_of(&self, kind: ConnectorKind) -> Option<ConnectorRecord> {
        let st = self.st();
        let id = self.default_id(&st, kind)?;
        self.find(&st, &id)
    }

    // ------------------------------------------------------------------ view models

    fn vm(
        &self,
        st: &State,
        r: &ConnectorRecord,
        defaults: &BTreeMap<ConnectorKind, Option<String>>,
    ) -> ConnectorVm {
        let status = self.status_of(st, r);
        let is_default = defaults
            .get(&r.connector_kind())
            .is_some_and(|d| d.as_deref() == Some(r.id.as_str()));
        let (id, name) = (r.id.clone(), r.name.clone());
        match &r.kind {
            RecordKind::Github { login, source } => {
                let source = if Self::is_env(r) {
                    GithubConnectorSource::Env
                } else {
                    match source {
                        GithubStored::Device => GithubConnectorSource::Device,
                        GithubStored::Token => GithubConnectorSource::Token,
                    }
                };
                let detail = match source {
                    GithubConnectorSource::Env => "From GITHUB_TOKEN in the environment",
                    GithubConnectorSource::Device => "Signed in with GitHub",
                    GithubConnectorSource::Token => "Access token",
                };
                let detail = match status {
                    ConnectorStatus::Expired => format!("{detail} · sign in again"),
                    ConnectorStatus::SignedOut => "No token stored · sign in".to_owned(),
                    _ => detail.to_owned(),
                };
                ConnectorVm::Github {
                    id,
                    name,
                    detail,
                    status,
                    is_default,
                    login: Some(login.clone()).filter(|l| !l.is_empty()),
                    source,
                }
            }
            RecordKind::Aws {
                profile,
                region,
                monthly_cap_usd,
            } => {
                let auth = self.aws_auth(profile);
                let label = match auth {
                    AwsAuthStyle::Sso => "SSO",
                    AwsAuthStyle::Role => "Assume role",
                    AwsAuthStyle::Process => "Credential process",
                    _ => "~/.aws profile",
                };
                let cap = monthly_cap_usd
                    .map(|c| format!(" · cap ${} / month", usd_en_us(c)))
                    .unwrap_or_default();
                let missing = if status == ConnectorStatus::Invalid {
                    " · profile not found in ~/.aws"
                } else {
                    ""
                };
                ConnectorVm::Aws {
                    id,
                    name,
                    detail: format!("{label} · {region}{cap}{missing}"),
                    status,
                    is_default,
                    profile: profile.clone(),
                    region: region.clone(),
                    auth,
                    monthly_cap_usd: *monthly_cap_usd,
                }
            }
            RecordKind::Agent {
                agent,
                source,
                key_hint,
            } => {
                let h = Self::harness_of(st, *agent);
                let mut version = None;
                let detail = match source {
                    AgentConnectorSource::ApiKey => {
                        let hint = key_hint.clone().unwrap_or_default();
                        let tail = if status == ConnectorStatus::Invalid {
                            "the provider rejected it"
                        } else {
                            "also runs in the cloud"
                        };
                        format!("API key {hint} · {tail}").replace("  ", " ")
                    }
                    AgentConnectorSource::Login => match h {
                        Some(h) if h.installed => {
                            version = h.version.clone();
                            format!(
                                "Found on this Mac{}{}",
                                h.version
                                    .as_ref()
                                    .map(|v| format!(" · v{v}"))
                                    .unwrap_or_default(),
                                if h.auth == HarnessAuth::SignedOut {
                                    format!(" · run {}", login_command(*agent))
                                } else {
                                    String::new()
                                }
                            )
                        }
                        _ => "Not found on this Mac".to_owned(),
                    },
                };
                ConnectorVm::Agent {
                    id,
                    name,
                    detail,
                    status,
                    is_default,
                    agent: *agent,
                    source: *source,
                    runs_on: match source {
                        AgentConnectorSource::Login => vec![AgentsWhere::Local],
                        AgentConnectorSource::ApiKey => {
                            vec![AgentsWhere::Local, AgentsWhere::Cloud]
                        }
                    },
                    version,
                    key_hint: key_hint.clone(),
                }
            }
        }
    }

    fn aws_auth(&self, profile: &str) -> AwsAuthStyle {
        self.aws_list()
            .and_then(|l| l.profiles.into_iter().find(|p| p.name == profile))
            .map_or(AwsAuthStyle::Other, |p| p.kind)
    }

    fn defaults_now(&self, st: &State) -> BTreeMap<ConnectorKind, Option<String>> {
        [
            ConnectorKind::Github,
            ConnectorKind::Aws,
            ConnectorKind::Agent,
        ]
        .into_iter()
        .map(|k| (k, self.default_id(st, k)))
        .collect()
    }

    /// Every connector as the Connectors screen shows it (detected CLI logins included).
    pub async fn list(&self) -> Vec<ConnectorVm> {
        self.sync().await;
        self.snapshot()
    }

    /// Same without probing the machine again.
    pub fn snapshot(&self) -> Vec<ConnectorVm> {
        let st = self.st();
        let defaults = self.defaults_now(&st);
        let all = self.records(&st);
        let rank = |k: ConnectorKind| match k {
            ConnectorKind::Github => 0,
            ConnectorKind::Aws => 1,
            ConnectorKind::Agent => 2,
        };
        let mut rest: Vec<ConnectorRecord> = all
            .iter()
            .filter(|r| r.connector_kind() != ConnectorKind::Agent)
            .cloned()
            .collect();
        rest.sort_by_key(|r| rank(r.connector_kind()));
        let agents = sort_agents(
            all.into_iter()
                .filter(|r| r.connector_kind() == ConnectorKind::Agent)
                .collect(),
        );
        rest.into_iter()
            .chain(agents)
            .map(|r| self.vm(&st, &r, &defaults))
            .collect()
    }

    pub fn vm_of(&self, id: &str) -> Option<ConnectorVm> {
        let st = self.st();
        let r = self.find(&st, id)?;
        Some(self.vm(&st, &r, &self.defaults_now(&st)))
    }

    // ------------------------------------------------------------------ add / update / remove

    fn name_taken(
        &self,
        st: &State,
        kind: ConnectorKind,
        name: &str,
        except: Option<&str>,
    ) -> bool {
        self.records(st).iter().any(|r| {
            r.connector_kind() == kind
                && Some(r.id.as_str()) != except
                && r.name.to_lowercase() == name.to_lowercase()
        })
    }

    fn unique_name(&self, st: &State, kind: ConnectorKind, wanted: &str) -> String {
        let mut name = wanted.to_owned();
        let mut n = 2;
        while self.name_taken(st, kind, &name, None) {
            name = format!("{wanted} ({n})");
            n += 1;
        }
        name
    }

    fn clean_name(name: &str) -> Result<String> {
        let n = name.trim();
        if n.is_empty() || n.chars().count() > 80 {
            return invalid("Use a name of 1 to 80 characters.");
        }
        Ok(n.to_owned())
    }

    /// The first connector of a kind becomes its default.
    fn default_if_first(st: &mut State, kind: ConnectorKind, id: &str) {
        let stored_exists = st
            .data
            .defaults
            .get(&kind)
            .is_some_and(|d| st.data.items.iter().any(|r| &r.id == d));
        if !stored_exists {
            st.data.defaults.insert(kind, id.to_owned());
        }
    }

    /// Store a GitHub token as a connector. The same account (login) signing in again replaces its token; `connector_id`
    /// re-signs-in a specific connector (and follows the login if it changed).
    pub fn upsert_github(&self, input: UpsertGithub) -> Result<ConnectorRecord> {
        let mut st = self.st();
        let UpsertGithub {
            login,
            token,
            via_device,
            connector_id,
            name,
            make_default,
        } = input;
        let is_github = |r: &ConnectorRecord| r.connector_kind() == ConnectorKind::Github;
        let mut target = connector_id.as_ref().and_then(|cid| {
            st.data
                .items
                .iter()
                .position(|r| &r.id == cid && is_github(r))
        });
        if target.is_none() {
            target = st.data.items.iter().position(|r| match &r.kind {
                RecordKind::Github { login: l, .. } => {
                    !l.is_empty() && l.to_lowercase() == login.to_lowercase()
                }
                _ => false,
            });
        }
        if connector_id.is_some() && target.is_none() {
            return Err(gone());
        }
        let id = target
            .map(|i| st.data.items[i].id.clone())
            .unwrap_or_else(|| new_id("github"));
        self.set_secret(&id, &token)?; // fails (changing nothing) when the keychain is unavailable
        let source = if via_device {
            GithubStored::Device
        } else {
            GithubStored::Token
        };
        let rec = if let Some(i) = target {
            let new_name = match &name {
                Some(n) => Some(Self::clean_name(n)?),
                None => {
                    let r = &st.data.items[i];
                    let old_login = match &r.kind {
                        RecordKind::Github { login, .. } => login.as_str(),
                        _ => "",
                    };
                    (r.name == old_login || r.name == "GitHub").then(|| login.clone())
                }
            };
            let r = &mut st.data.items[i];
            if let Some(n) = new_name {
                r.name = n;
            }
            r.kind = RecordKind::Github {
                login: login.clone(),
                source,
            };
            r.clone()
        } else {
            let wanted = match &name {
                Some(n) => Self::clean_name(n)?,
                None => login.clone(),
            };
            let rec = ConnectorRecord {
                id: id.clone(),
                name: self.unique_name(&st, ConnectorKind::Github, &wanted),
                created_at: self.now(),
                kind: RecordKind::Github {
                    login: login.clone(),
                    source,
                },
            };
            st.data.items.push(rec.clone());
            rec
        };
        st.statuses.insert(id.clone(), ConnectorStatus::Connected);
        Self::default_if_first(&mut st, ConnectorKind::Github, &id);
        if make_default {
            st.data.defaults.insert(ConnectorKind::Github, id);
        }
        self.save(&st);
        Ok(rec)
    }

    /// A pasted token (personal access token or app token): proven against GitHub before it is kept.
    pub async fn add_github_token(
        &self,
        token: &str,
        name: Option<String>,
        make_default: bool,
    ) -> Result<ConnectorRecord> {
        if let Some(problem) = validate_secret_format(SecretName::GithubToken, token) {
            return invalid(problem);
        }
        let Some(github) = &self.deps.github else {
            return invalid("GitHub is not available in this build.");
        };
        let login = match github.user(token).await {
            Ok(u) => u.login,
            Err(e) => return invalid(scrub(&e.to_string(), &[Some(token.to_owned())])),
        };
        self.upsert_github(UpsertGithub {
            login,
            token: token.to_owned(),
            via_device: false,
            connector_id: None,
            name,
            make_default,
        })
    }

    fn valid_region(region: &str) -> Result<String> {
        let problem = validate_aws_region(region);
        if problem.is_some() || region.trim().is_empty() {
            return invalid(problem.unwrap_or_else(|| "Choose a region such as us-east-1.".into()));
        }
        Ok(region.trim().to_owned())
    }

    fn valid_cap(cap: Option<f64>) -> Result<Option<f64>> {
        let Some(cap) = cap else { return Ok(None) };
        if !cap.is_finite() || cap <= 0.0 || cap > MAX_CAP_USD {
            return invalid("The monthly cap must be a positive number of US dollars.");
        }
        Ok(Some((cap * 100.0).round() / 100.0))
    }

    pub fn add_aws(
        &self,
        input: AddAwsConnectorInput,
        make_default: bool,
    ) -> Result<ConnectorRecord> {
        let profile = input.profile.trim().to_owned();
        let problem = validate_aws_profile(&profile);
        if problem.is_some() || profile.is_empty() {
            return invalid(problem.unwrap_or_else(|| "Choose an AWS profile.".into()));
        }
        let mut st = self.st();
        if st
            .data
            .items
            .iter()
            .any(|r| matches!(&r.kind, RecordKind::Aws { profile: p, .. } if p == &profile))
        {
            return invalid(format!("{profile} is already connected."));
        }
        let wanted = match &input.name {
            Some(n) => Self::clean_name(n)?,
            None => profile.clone(),
        };
        let region = Self::valid_region(
            input
                .region
                .as_deref()
                .filter(|r| !r.is_empty())
                .unwrap_or(DEFAULT_AWS_REGION),
        )?;
        let monthly_cap_usd = Self::valid_cap(input.monthly_cap_usd)?;
        let rec = ConnectorRecord {
            id: new_id("aws"),
            name: self.unique_name(&st, ConnectorKind::Aws, &wanted),
            created_at: self.now(),
            kind: RecordKind::Aws {
                profile,
                region,
                monthly_cap_usd,
            },
        };
        st.data.items.push(rec.clone());
        Self::default_if_first(&mut st, ConnectorKind::Aws, &rec.id);
        if make_default {
            st.data.defaults.insert(ConnectorKind::Aws, rec.id.clone());
        }
        self.save(&st);
        Ok(rec)
    }

    /// An API key for Claude Code, Codex or Grok, kept in the keychain and checked with the provider (a rejected key is kept as
    /// `invalid`); or adopt a detected CLI login that was removed earlier.
    pub async fn add_agent(
        &self,
        input: AddAgentConnectorInput,
        wait_for_check: bool,
    ) -> Result<ConnectorRecord> {
        match input {
            AddAgentConnectorInput::Login { agent } => {
                let id = login_id(agent);
                self.sync().await;
                let mut st = self.st();
                if let Some(existing) = st.data.items.iter().find(|r| r.id == id) {
                    return Ok(existing.clone());
                }
                if !Self::harness_of(&st, agent).is_some_and(|h| h.installed) {
                    return invalid(format!("{} was not found on this Mac.", agent.label()));
                }
                st.data.dismissed.retain(|x| x != &id);
                self.add_detected_logins(&mut st);
                st.data
                    .items
                    .iter()
                    .find(|r| r.id == id)
                    .cloned()
                    .ok_or_else(gone)
            }
            AddAgentConnectorInput::ApiKey {
                agent,
                api_key,
                name,
            } => {
                if let Some(problem) = validate_secret_format(secret_for(agent), &api_key) {
                    return invalid(problem);
                }
                let id = new_id(&format!("agent-{}", agent_slug(agent)));
                let rec = {
                    let mut st = self.st();
                    let wanted = match &name {
                        Some(n) => Self::clean_name(n)?,
                        None => format!("{} · API key", agent.label()),
                    };
                    self.set_secret(&id, &api_key)?;
                    let rec = ConnectorRecord {
                        id: id.clone(),
                        name: self.unique_name(&st, ConnectorKind::Agent, &wanted),
                        created_at: self.now(),
                        kind: RecordKind::Agent {
                            agent,
                            source: AgentConnectorSource::ApiKey,
                            key_hint: Some(key_hint(&api_key)),
                        },
                    };
                    st.data.items.push(rec.clone());
                    self.save(&st);
                    rec
                };
                if wait_for_check {
                    let _ = self.test(&id).await;
                }
                Ok(rec)
            }
        }
    }

    pub fn update(&self, id: &str, patch: UpdateConnectorInput) -> Result<()> {
        let mut st = self.st();
        let r = self.need(&st, id)?;
        if Self::is_env(&r) {
            return invalid("This account comes from the environment; change it there.");
        }
        let i = st
            .data
            .items
            .iter()
            .position(|x| x.id == id)
            .ok_or_else(gone)?;
        if let Some(n) = &patch.name {
            let name = Self::clean_name(n)?;
            if self.name_taken(&st, r.connector_kind(), &name, Some(id)) {
                return invalid(format!("There is already a connector named {name}."));
            }
            st.data.items[i].name = name;
        }
        if patch.region.is_some() || patch.monthly_cap_usd.is_some() {
            let RecordKind::Aws {
                region,
                monthly_cap_usd,
                ..
            } = &mut st.data.items[i].kind
            else {
                return invalid("Only AWS connectors have a region and a spend cap.");
            };
            if let Some(r) = &patch.region {
                *region = Self::valid_region(r)?;
            }
            if let Some(cap) = patch.monthly_cap_usd {
                *monthly_cap_usd = Self::valid_cap(cap)?;
            }
        }
        self.save(&st);
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut st = self.st();
        let r = self.need(&st, id)?;
        if Self::is_env(&r) {
            return invalid(
                "This account comes from GITHUB_TOKEN in the environment; unset it there.",
            );
        }
        let kind = r.connector_kind();
        st.data.items.retain(|x| x.id != id);
        if st.data.defaults.get(&kind).is_some_and(|d| d == id) {
            st.data.defaults.remove(&kind);
        }
        if matches!(
            r.kind,
            RecordKind::Agent {
                source: AgentConnectorSource::Login,
                ..
            }
        ) && !st.data.dismissed.iter().any(|d| d == id)
        {
            st.data.dismissed.push(id.to_owned());
        }
        self.clear_secret(id);
        st.statuses.remove(id);
        self.save(&st);
        Ok(())
    }

    // ------------------------------------------------------------------ probes

    /// Check one connector against its provider and remember the status. A network failure leaves the status alone.
    pub async fn test(&self, id: &str) -> Result<TestOutcome> {
        let (r, known) = {
            let st = self.st();
            let r = self.need(&st, id)?;
            let mut known: Vec<Option<String>> = st
                .data
                .items
                .iter()
                .map(|x| self.get_secret(&x.id))
                .collect();
            known.extend([
                self.env_secret(SecretName::GithubToken),
                self.env_secret(SecretName::AnthropicApiKey),
                self.env_secret(SecretName::OpenaiApiKey),
                self.env_secret(SecretName::XaiApiKey),
            ]);
            (r, known)
        };
        let (ok, detail, status) = self.probe(&r).await;
        if let Some(s) = status {
            self.st().statuses.insert(id.to_owned(), s);
        }
        Ok(TestOutcome {
            ok,
            detail: scrub(&detail, &known),
        })
    }

    async fn probe(&self, r: &ConnectorRecord) -> (bool, String, Option<ConnectorStatus>) {
        let unavailable = (
            false,
            "Connection tests are not available in this build.".to_owned(),
            None,
        );
        match &r.kind {
            RecordKind::Github { .. } => {
                let token = self.secret_of(r);
                let Some(github) = &self.deps.github else {
                    return (false, "GitHub is not available in this build.".into(), None);
                };
                let Some(token) = token else {
                    return (
                        false,
                        "Sign in with GitHub first.".into(),
                        Some(ConnectorStatus::SignedOut),
                    );
                };
                match github.user(&token).await {
                    Ok(u) => {
                        if !Self::is_env(r) && !u.login.is_empty() {
                            let mut st = self.st();
                            if let Some(rec) = st.data.items.iter_mut().find(|x| x.id == r.id)
                                && let RecordKind::Github { login, .. } = &mut rec.kind
                                && *login != u.login
                            {
                                login.clone_from(&u.login);
                                self.save(&st);
                            }
                        }
                        (
                            true,
                            format!("signed in as {}", u.login),
                            Some(ConnectorStatus::Connected),
                        )
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        let expired = msg.contains("(401)") || msg.contains("rejected the sign-in");
                        (false, msg, expired.then_some(ConnectorStatus::Expired))
                    }
                }
            }
            RecordKind::Aws {
                profile, region, ..
            } => {
                let Some(probe) = &self.deps.probe else {
                    return unavailable;
                };
                let (ok, detail) = probe.aws(region, profile).await;
                if ok {
                    return (true, detail, Some(ConnectorStatus::Connected));
                }
                if unreachable(&detail) {
                    return (false, detail, None);
                }
                let auth = self.aws_auth(profile);
                let expired = matches!(
                    auth,
                    AwsAuthStyle::Sso | AwsAuthStyle::Role | AwsAuthStyle::Process
                );
                let detail = if auth == AwsAuthStyle::Sso {
                    format!(
                        "{detail} Run \"aws sso login --profile {profile}\" in a terminal, then test again."
                    )
                } else {
                    detail
                };
                (
                    false,
                    detail,
                    Some(if expired {
                        ConnectorStatus::Expired
                    } else {
                        ConnectorStatus::Invalid
                    }),
                )
            }
            RecordKind::Agent { agent, source, .. } => match source {
                AgentConnectorSource::Login => {
                    if let Some(h) = &self.deps.harnesses {
                        let found = h.refresh().await;
                        self.st().harness = found;
                    }
                    let h = Self::harness_of(&self.st(), *agent);
                    let ok = h
                        .as_ref()
                        .is_some_and(|h| h.installed && h.auth != HarnessAuth::SignedOut);
                    let detail = h.map_or_else(
                        || format!("{} was not found on this Mac.", agent.label()),
                        |h| h.hint,
                    );
                    (
                        ok,
                        detail,
                        Some(if ok {
                            ConnectorStatus::Connected
                        } else {
                            ConnectorStatus::SignedOut
                        }),
                    )
                }
                AgentConnectorSource::ApiKey => {
                    let Some(probe) = &self.deps.probe else {
                        return unavailable;
                    };
                    let Some(api_key) = self.secret_of(r) else {
                        return (
                            false,
                            "The key is not in the keychain any more; add it again.".into(),
                            Some(ConnectorStatus::Invalid),
                        );
                    };
                    let (ok, detail) = match agent {
                        ConnectorAgent::ClaudeCode => probe.anthropic(&api_key).await,
                        ConnectorAgent::Codex => probe.openai(&api_key).await,
                        ConnectorAgent::Grok => probe.xai(&api_key).await,
                    };
                    if ok {
                        (true, detail, Some(ConnectorStatus::Valid))
                    } else {
                        let status = (!unreachable(&detail)).then_some(ConnectorStatus::Invalid);
                        (false, detail, status)
                    }
                }
            },
        }
    }

    /// Probe the machine for agent CLIs again, then check every connector (failures never throw).
    pub async fn refresh(&self) {
        if let Some(h) = &self.deps.harnesses {
            let found = h.refresh().await;
            let mut st = self.st();
            st.harness = found;
            self.add_detected_logins(&mut st);
        }
        let ids: Vec<String> = {
            let st = self.st();
            self.records(&st)
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r.kind,
                        RecordKind::Agent {
                            source: AgentConnectorSource::Login,
                            ..
                        }
                    )
                })
                .map(|r| r.id)
                .collect()
        };
        futures::future::join_all(ids.iter().map(|id| self.test(id))).await;
    }

    // ------------------------------------------------------------------ secrets for the engine (daemon only)

    /// GitHub token of a connector (default: the default GitHub connector).
    pub fn github_token(&self, id: Option<&str>) -> Option<String> {
        let st = self.st();
        let r = match id {
            Some(id) => self.find(&st, id),
            None => self
                .default_id(&st, ConnectorKind::Github)
                .and_then(|d| self.find(&st, &d)),
        }?;
        matches!(r.kind, RecordKind::Github { .. })
            .then(|| self.secret_of(&r))
            .flatten()
    }

    /// The value behind the old single-secret names: the default GitHub token, the Anthropic / OpenAI key of the default key
    /// connector of that agent (or any), then the environment.
    pub fn secret_value(&self, name: SecretName) -> Option<String> {
        match name {
            SecretName::GithubToken => self
                .github_token(None)
                .or_else(|| self.env_secret(SecretName::GithubToken)),
            SecretName::AnthropicApiKey => self
                .key_of(ConnectorAgent::ClaudeCode)
                .or_else(|| self.env_secret(name)),
            SecretName::OpenaiApiKey => self
                .key_of(ConnectorAgent::Codex)
                .or_else(|| self.env_secret(name)),
            SecretName::XaiApiKey => self
                .key_of(ConnectorAgent::Grok)
                .or_else(|| self.env_secret(name)),
        }
    }

    pub fn has_secret(&self, name: SecretName) -> bool {
        self.secret_value(name).is_some()
    }

    /// The stored key of the best `api-key` connector for an agent: the default one if it is a key, else the first.
    fn key_of(&self, agent: ConnectorAgent) -> Option<String> {
        let st = self.st();
        self.key_of_locked(&st, agent)
    }

    fn key_of_locked(&self, st: &State, agent: ConnectorAgent) -> Option<String> {
        let keys: Vec<ConnectorRecord> = self
            .records(st)
            .into_iter()
            .filter(|r| {
                matches!(&r.kind, RecordKind::Agent { agent: a, source: AgentConnectorSource::ApiKey, .. } if *a == agent)
            })
            .collect();
        let def = self.default_id(st, ConnectorKind::Agent);
        let pick = keys
            .iter()
            .find(|r| Some(&r.id) == def.as_ref())
            .or_else(|| {
                keys.iter()
                    .find(|r| self.status_of(st, r) == ConnectorStatus::Valid)
            })
            .or_else(|| keys.first())?;
        self.secret_of(pick)
    }

    // ------------------------------------------------------------------ per-repo selection

    /// The saved selection filtered to what still exists.
    fn valid_selection(
        &self,
        st: &State,
        sel: &RepoConnections,
    ) -> (bool, bool, Vec<AgentSelection>) {
        let ok = |id: &str, kind: ConnectorKind| {
            self.find(st, id)
                .is_some_and(|r| r.connector_kind() == kind)
        };
        let agents = sel
            .agents
            .iter()
            .filter(|a| {
                self.find(st, &a.connector_id)
                    .is_some_and(|r| match r.kind {
                        RecordKind::Agent { source, .. } => {
                            a.where_ == AgentsWhere::Local || source == AgentConnectorSource::ApiKey
                        }
                        _ => false,
                    })
            })
            .cloned()
            .collect();
        (
            ok(&sel.github, ConnectorKind::Github),
            ok(&sel.aws, ConnectorKind::Aws),
            agents,
        )
    }

    /// The default connector of each kind as a selection (`None` = nothing connected of that kind).
    fn default_selection(&self, st: &State) -> EffectiveConnections {
        let agent = self.default_id(st, ConnectorKind::Agent);
        EffectiveConnections {
            github: self.default_id(st, ConnectorKind::Github),
            aws: self.default_id(st, ConnectorKind::Aws),
            agents: agent
                .map(|a| {
                    vec![AgentSelection {
                        connector_id: a,
                        where_: AgentsWhere::Local,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    /// What a run on this repo would use: the remembered choice where it is still valid, else the defaults.
    pub fn repo_connections(&self, repo: &str) -> RepoConnectionsVm {
        let st = self.st();
        self.repo_connections_locked(&st, repo)
    }

    fn repo_connections_locked(&self, st: &State, repo: &str) -> RepoConnectionsVm {
        let key = repo_key(repo);
        let defaults = self.default_selection(st);
        let Some(saved) = st.data.selections.get(&key) else {
            return RepoConnectionsVm {
                repo: key,
                saved: None,
                effective: defaults,
            };
        };
        let (gh, aws, agents) = self.valid_selection(st, saved);
        let complete = gh && aws && !agents.is_empty();
        RepoConnectionsVm {
            repo: key,
            saved: complete.then(|| RepoConnections {
                github: saved.github.clone(),
                aws: saved.aws.clone(),
                agents: agents.clone(),
            }),
            effective: EffectiveConnections {
                github: if gh {
                    Some(saved.github.clone())
                } else {
                    defaults.github
                },
                aws: if aws {
                    Some(saved.aws.clone())
                } else {
                    defaults.aws
                },
                agents: if agents.is_empty() {
                    defaults.agents
                } else {
                    agents
                },
            },
        }
    }

    /// Check and remember the choice for a repo.
    pub fn save_repo_connections(
        &self,
        repo: &str,
        sel: RepoConnections,
    ) -> Result<RepoConnectionsVm> {
        let mut st = self.st();
        if self
            .find(&st, &sel.github)
            .is_none_or(|r| r.connector_kind() != ConnectorKind::Github)
        {
            return invalid("Choose a GitHub account.");
        }
        if self
            .find(&st, &sel.aws)
            .is_none_or(|r| r.connector_kind() != ConnectorKind::Aws)
        {
            return invalid("Choose an AWS account.");
        }
        if sel.agents.is_empty() {
            return invalid("Choose at least one agent.");
        }
        let mut seen: Vec<ConnectorAgent> = Vec::new();
        for a in &sel.agents {
            let Some(ConnectorRecord {
                kind: RecordKind::Agent { agent, source, .. },
                name,
                ..
            }) = self.find(&st, &a.connector_id)
            else {
                return invalid("Choose an agent connector.");
            };
            if a.where_ == AgentsWhere::Cloud && source != AgentConnectorSource::ApiKey {
                return invalid(format!(
                    "{name} is a login on this Mac: it cannot run in the cloud. Use an API key."
                ));
            }
            if seen.contains(&agent) {
                return invalid(format!("Choose one {} connector per run.", agent.label()));
            }
            seen.push(agent);
        }
        st.data.selections.insert(
            repo_key(repo),
            RepoConnections {
                github: sel.github,
                aws: sel.aws,
                agents: sel
                    .agents
                    .into_iter()
                    .map(|a| AgentSelection {
                        connector_id: a.connector_id,
                        where_: a.where_,
                    })
                    .collect(),
            },
        );
        self.save(&st);
        Ok(self.repo_connections_locked(&st, repo))
    }

    /// Where every chosen agent runs, or why the choice cannot be run (the engine runs all agents of one optimization in one
    /// place).
    pub fn where_of(&self, agents: &[AgentSelection]) -> Result<Option<AgentsWhere>> {
        let places: Vec<AgentsWhere> =
            agents
                .iter()
                .map(|a| a.where_)
                .fold(Vec::new(), |mut v, w| {
                    if !v.contains(&w) {
                        v.push(w);
                    }
                    v
                });
        if places.len() > 1 {
            return invalid(
                "Run all agents on this Mac or all of them in the cloud: mixing is not supported yet.",
            );
        }
        Ok(agents.first().map(|a| a.where_))
    }

    /// The connectors a run on `repo` uses, with their secrets (`None` repo: the defaults). Backwards compatible: with nothing
    /// remembered for the repo, the defaults are used, as the single-account Settings did.
    pub async fn resolve(&self, repo: Option<&str>) -> ResolvedConnections {
        self.sync().await;
        let st = self.st();
        let (eff, saved) = match repo {
            Some(r) => {
                let v = self.repo_connections_locked(&st, r);
                (v.effective, v.saved)
            }
            None => (self.default_selection(&st), None),
        };
        let gh = eff.github.as_ref().and_then(|id| self.find(&st, id));
        let aws = eff.aws.as_ref().and_then(|id| self.find(&st, id));
        let token = gh.as_ref().and_then(|g| match g.kind {
            RecordKind::Github { .. } => self.secret_of(g),
            _ => None,
        });
        let mut agents: Vec<ResolvedAgent> = Vec::new();
        let mut cloud_keys: BTreeMap<ConnectorAgent, String> = BTreeMap::new();
        if let Some(saved) = saved {
            for a in &saved.agents {
                let Some(ConnectorRecord {
                    id,
                    name,
                    kind: RecordKind::Agent { agent, source, .. },
                    ..
                }) = self.find(&st, &a.connector_id)
                else {
                    continue;
                };
                let api_key = if source == AgentConnectorSource::ApiKey {
                    self.get_secret(&id)
                } else {
                    None
                };
                if let Some(k) = &api_key {
                    cloud_keys.insert(agent, k.clone());
                }
                agents.push(ResolvedAgent {
                    connector_id: id,
                    name,
                    agent,
                    source,
                    where_: a.where_,
                    api_key,
                });
            }
        } else {
            // No choice for this repo: the best connector of each agent (the default one first), and any stored key for cloud
            // runners.
            let def = self.default_id(&st, ConnectorKind::Agent);
            for agent in AGENTS {
                let mine = sort_agents(
                    self.records(&st)
                        .into_iter()
                        .filter(|r| matches!(&r.kind, RecordKind::Agent { agent: a, .. } if *a == agent))
                        .collect(),
                );
                let usable: Vec<&ConnectorRecord> = mine
                    .iter()
                    .filter(|r| {
                        matches!(
                            self.status_of(&st, r),
                            ConnectorStatus::Connected | ConnectorStatus::Valid
                        )
                    })
                    .collect();
                let pick = mine
                    .iter()
                    .find(|r| Some(&r.id) == def.as_ref())
                    .or_else(|| usable.first().copied())
                    .or_else(|| mine.first());
                if let Some(p) = pick
                    && let RecordKind::Agent { source, .. } = &p.kind
                {
                    agents.push(ResolvedAgent {
                        connector_id: p.id.clone(),
                        name: p.name.clone(),
                        agent,
                        source: *source,
                        where_: AgentsWhere::Local,
                        api_key: if *source == AgentConnectorSource::ApiKey {
                            self.secret_of(p)
                        } else {
                            None
                        },
                    });
                }
                if let Some(k) = self.key_of_locked(&st, agent) {
                    cloud_keys.insert(agent, k);
                }
            }
        }
        ResolvedConnections {
            github: match (&gh, token) {
                (
                    Some(ConnectorRecord {
                        id,
                        name,
                        kind: RecordKind::Github { login, .. },
                        ..
                    }),
                    Some(token),
                ) => Some(ResolvedGithub {
                    id: id.clone(),
                    name: name.clone(),
                    login: Some(login.clone()).filter(|l| !l.is_empty()),
                    token,
                }),
                _ => None,
            },
            aws: match aws {
                Some(ConnectorRecord {
                    id,
                    name,
                    kind:
                        RecordKind::Aws {
                            profile,
                            region,
                            monthly_cap_usd,
                        },
                    ..
                }) => Some(ResolvedAws {
                    id,
                    name,
                    profile,
                    region,
                    monthly_cap_usd,
                }),
                _ => None,
            },
            agents,
            cloud_keys,
        }
    }
}

/// `····4f2a`: the last characters of a key (never the key).
fn key_hint(key: &str) -> String {
    let tail: String = {
        let n = key.chars().count();
        key.chars().skip(n.saturating_sub(4)).collect()
    };
    format!("····{tail}")
}

/// Logins before keys, Claude Code before Codex before Grok (`sortAgents`).
fn sort_agents(mut list: Vec<ConnectorRecord>) -> Vec<ConnectorRecord> {
    let rank = |r: &ConnectorRecord| match &r.kind {
        RecordKind::Agent { agent, source, .. } => {
            AGENTS.iter().position(|a| a == agent).unwrap_or(0) * 2
                + usize::from(*source == AgentConnectorSource::ApiKey)
        }
        _ => 0,
    };
    list.sort_by_key(rank);
    list
}
