//! Shared fakes of the connector tests: no network, no AWS, no real CLIs, no real keychain (the TS `test-support.ts`).
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_agents::detect::{HarnessAuth, HarnessInfo, HarnessKind};
use ratchet_connectors::secrets::{MemorySecretStore, SecretError, SecretStore};
use ratchet_connectors::*;
use ratchet_github::{
    DeviceCode, DevicePoll, Error as GithubError, GithubApi, GithubUser, Repo,
    Result as GithubResult,
};

pub const GH_TOKEN: &str = "ghp_TOPSECRETTOKENVALUE1234567890abcd";
pub const ORG_TOKEN: &str = "github_pat_ORGTOKENORGTOKENORGTOKEN12";
pub const ANTHROPIC_KEY: &str = "sk-ant-api03-TOPSECRETANTHROPICKEY";
pub const OPENAI_KEY: &str = "sk-proj-TOPSECRETOPENAIKEYVALUE";

pub fn last4(key: &str) -> String {
    key.chars().skip(key.chars().count() - 4).collect()
}

// ------------------------------------------------------------------ GitHub

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UserFailure {
    Rejected,
    Offline,
}

/// The TS `FakeGithub`: `user` answers `login`, or fails the way GitHub (or the network) does.
pub struct FakeGithub {
    pub login: Mutex<String>,
    pub fail: Mutex<Option<UserFailure>>,
    pub tokens_seen: Mutex<Vec<String>>,
}

impl FakeGithub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            login: Mutex::new("octocat".into()),
            fail: Mutex::new(None),
            tokens_seen: Mutex::new(Vec::new()),
        })
    }

    pub fn set_fail(&self, f: Option<UserFailure>) {
        *self.fail.lock().unwrap() = f;
    }
}

#[async_trait]
impl GithubApi for FakeGithub {
    fn client_id_configured(&self) -> bool {
        true
    }
    async fn device_start(&self) -> GithubResult<DeviceCode> {
        unimplemented!()
    }
    async fn device_poll(&self, _: &str) -> GithubResult<DevicePoll> {
        unimplemented!()
    }
    async fn user(&self, token: &str) -> GithubResult<GithubUser> {
        self.tokens_seen.lock().unwrap().push(token.to_owned());
        match *self.fail.lock().unwrap() {
            Some(UserFailure::Rejected) => Err(GithubError::http(
                Some(401),
                "GitHub rejected the sign-in (401): sign in again in Settings.",
            )),
            Some(UserFailure::Offline) => {
                Err(GithubError::http(None, "Could not reach GitHub: offline"))
            }
            None => Ok(GithubUser {
                login: self.login.lock().unwrap().clone(),
            }),
        }
    }
    async fn repos(&self, _: &str, _: Option<&str>) -> GithubResult<Vec<Repo>> {
        unimplemented!()
    }
    async fn repo(&self, _: &str, _: &str) -> GithubResult<Repo> {
        unimplemented!()
    }
    async fn file(&self, _: &str, _: &str, _: &str, _: &str) -> GithubResult<Option<String>> {
        unimplemented!()
    }
    async fn branches(&self, _: &str, _: &str) -> GithubResult<Vec<String>> {
        unimplemented!()
    }
}

// ------------------------------------------------------------------ probes

/// The TS `Tester`: every probe answers ok, or `fail` when set; `calls` records what was probed.
#[derive(Default)]
pub struct Tester {
    pub fail: Mutex<Option<String>>,
    pub calls: Mutex<Vec<String>>,
}

impl Tester {
    pub fn set_fail(&self, f: Option<&str>) {
        *self.fail.lock().unwrap() = f.map(str::to_owned);
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn answer(&self, call: String, ok: &str) -> (bool, String) {
        self.calls.lock().unwrap().push(call);
        match self.fail.lock().unwrap().clone() {
            Some(f) => (false, f),
            None => (true, ok.to_owned()),
        }
    }
}

#[async_trait]
impl ConnectorProbe for Tester {
    async fn aws(&self, _region: &str, profile: &str) -> (bool, String) {
        self.answer(
            format!("aws:{profile}"),
            "signed in to account 123456789012",
        )
    }
    async fn anthropic(&self, _: &str) -> (bool, String) {
        self.answer("anthropic".into(), "Anthropic accepted the key")
    }
    async fn openai(&self, _: &str) -> (bool, String) {
        self.answer("openai".into(), "OpenAI accepted the key")
    }
    async fn xai(&self, _: &str) -> (bool, String) {
        self.answer("xai".into(), "xAI accepted the key")
    }
}

// ------------------------------------------------------------------ harnesses

pub fn claude() -> HarnessInfo {
    HarnessInfo {
        kind: HarnessKind::ClaudeCode,
        installed: true,
        version: Some("2.1.4".into()),
        path: Some("/bin/claude".into()),
        auth: HarnessAuth::SignedIn,
        hint: "signed in".into(),
    }
}

pub fn codex() -> HarnessInfo {
    HarnessInfo {
        kind: HarnessKind::Codex,
        installed: false,
        version: None,
        path: None,
        auth: HarnessAuth::SignedOut,
        hint: "Install Codex: npm i -g @openai/codex".into(),
    }
}

pub struct Harnesses(pub Vec<HarnessInfo>);

#[async_trait]
impl HarnessSource for Harnesses {
    async fn get(&self) -> Vec<HarnessInfo> {
        self.0.clone()
    }
    async fn refresh(&self) -> Vec<HarnessInfo> {
        self.0.clone()
    }
}

// ------------------------------------------------------------------ the keychain

/// A keychain that can be locked: reads fail while it is.
pub struct LockableStore {
    inner: MemorySecretStore,
    pub locked: AtomicBool,
}

impl LockableStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemorySecretStore::new(),
            locked: AtomicBool::new(false),
        })
    }

    pub fn lock(&self, on: bool) {
        self.locked.store(on, Ordering::SeqCst);
    }
}

impl SecretStore for LockableStore {
    fn available(&self) -> bool {
        !self.locked.load(Ordering::SeqCst)
    }
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(SecretError::Failed("keychain is locked".into()));
        }
        self.inner.get(account)
    }
    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(SecretError::Unavailable);
        }
        self.inner.set(account, value)
    }
    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.inner.delete(account)
    }
}

// ------------------------------------------------------------------ the old single-account settings

/// What the first versions of Settings held: one GitHub token, one AWS profile, one key per agent. The secrets sit in the
/// same secret store under their old names, like `SettingsStore` keeps them.
pub struct OldSettings {
    pub secrets: Arc<dyn SecretStore>,
    pub state: Mutex<(String, String, String, Option<bool>)>, // aws_profile, aws_region, github_login, via_device
}

impl OldSettings {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Arc<Self> {
        Arc::new(Self {
            secrets,
            state: Mutex::new((String::new(), String::new(), String::new(), None)),
        })
    }

    pub fn set_secret(&self, name: SecretName, value: &str) {
        self.secrets.set(name.as_str(), value).unwrap();
    }

    pub fn set_github_account(&self, login: &str, via_device: bool) {
        let mut s = self.state.lock().unwrap();
        s.2 = login.into();
        s.3 = Some(via_device);
    }

    pub fn set_aws(&self, profile: &str, region: &str) {
        let mut s = self.state.lock().unwrap();
        s.0 = profile.into();
        s.1 = region.into();
    }

    pub fn is_empty(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.0.is_empty()
            && s.2.is_empty()
            && [
                SecretName::GithubToken,
                SecretName::AnthropicApiKey,
                SecretName::OpenaiApiKey,
            ]
            .iter()
            .all(|n| self.secrets.get(n.as_str()).ok().flatten().is_none())
    }
}

impl LegacySource for OldSettings {
    fn read(&self) -> LegacyAccounts {
        let s = self.state.lock().unwrap();
        let mut secrets = BTreeMap::new();
        let mut locked = false;
        for n in [
            SecretName::GithubToken,
            SecretName::AnthropicApiKey,
            SecretName::OpenaiApiKey,
        ] {
            match self.secrets.get(n.as_str()) {
                Ok(Some(v)) => {
                    secrets.insert(n, v);
                }
                Ok(None) => {}
                Err(_) => locked = true,
            }
        }
        LegacyAccounts {
            aws_profile: s.0.clone(),
            aws_region: s.1.clone(),
            github_login: s.2.clone(),
            github_via_device: s.3,
            secrets,
            locked,
        }
    }

    fn clear(&self) {
        *self.state.lock().unwrap() = (String::new(), String::new(), String::new(), None);
        for n in [
            SecretName::GithubToken,
            SecretName::AnthropicApiKey,
            SecretName::OpenaiApiKey,
        ] {
            let _ = self.secrets.delete(n.as_str());
        }
    }
}

// ------------------------------------------------------------------ the store under test

pub fn profiles() -> AwsProfileList {
    AwsProfileList {
        profiles: vec![
            AwsProfile {
                name: "acme-prod".into(),
                kind: AwsAuthStyle::Sso,
            },
            AwsProfile {
                name: "personal-dev".into(),
                kind: AwsAuthStyle::Keys,
            },
            AwsProfile {
                name: "acme-staging".into(),
                kind: AwsAuthStyle::Sso,
            },
        ],
        files_found: true,
    }
}

pub struct Made {
    pub store: ConnectorStore,
    pub github: Arc<FakeGithub>,
    pub tester: Arc<Tester>,
    pub secrets: Arc<dyn SecretStore>,
}

pub struct Opts {
    pub secrets: Arc<dyn SecretStore>,
    pub env: HashMap<String, String>,
    pub file: Option<PathBuf>,
    pub harnesses: Option<Vec<HarnessInfo>>,
    pub aws: AwsProfileList,
    pub legacy: Option<Arc<dyn LegacySource>>,
    pub github: Arc<FakeGithub>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            secrets: Arc::new(MemorySecretStore::new()),
            env: HashMap::new(),
            file: None,
            harnesses: Some(vec![claude(), codex()]),
            aws: profiles(),
            legacy: None,
            github: FakeGithub::new(),
        }
    }
}

pub fn make(o: Opts) -> Made {
    let tester = Arc::new(Tester::default());
    let aws = o.aws;
    let deps = ConnectorDeps {
        github: Some(o.github.clone()),
        harnesses: o
            .harnesses
            .map(|h| Arc::new(Harnesses(h)) as Arc<dyn HarnessSource>),
        aws_profiles: Some(Arc::new(move || aws.clone())),
        probe: Some(tester.clone()),
        legacy: o.legacy,
        clock: None,
    };
    Made {
        store: ConnectorStore::new(o.file, o.secrets.clone(), o.env, deps),
        github: o.github,
        tester,
        secrets: o.secrets,
    }
}

pub fn tmp_file(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("connectors.json")
}

pub fn contents(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

// ------------------------------------------------------------------ reading view models

pub fn github_source(c: &ConnectorVm) -> Option<GithubConnectorSource> {
    match c {
        ConnectorVm::Github { source, .. } => Some(*source),
        _ => None,
    }
}

pub fn agents_of(list: &[ConnectorVm]) -> Vec<&ConnectorVm> {
    list.iter()
        .filter(|c| c.kind() == ConnectorKind::Agent)
        .collect()
}
