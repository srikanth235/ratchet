//! App settings and the few facts the ledger does not hold (pull-request URLs), in a small JSON file, plus secrets in a
//! [`SecretStore`] (the Keychain on macOS). Port of `packages/desktop/src/main/settings.ts`.
//!
//! Secret values leave only through [`SettingsStore::get_secret`], inside the daemon; no view model carries one.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use ratchet_connectors::{LegacyAccounts, LegacySource};
use ratchet_engine::desktop::SecretStore;
use serde::{Deserialize, Serialize};

use crate::api::validate::validate_live_settings;
use crate::api::{
    AwsAuthKind, DEFAULT_AWS_REGION, GithubSource, LiveSettings, LiveSettingsInput, SecretName,
    SecretsPresent,
};
use crate::service::{ServiceError, ServiceResult};

/// The environment variables that count as a secret being provided.
pub fn env_for(name: SecretName) -> &'static [&'static str] {
    match name {
        SecretName::AnthropicApiKey => &["ANTHROPIC_API_KEY"],
        SecretName::OpenaiApiKey => &["OPENAI_API_KEY"],
        SecretName::GithubToken => &["GITHUB_TOKEN", "GH_TOKEN"],
    }
}

/// Persisted fields (camelCase, like the Electron app's `settings.json`; no secrets).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Persisted {
    aws_profile: String,
    /// The signed-in GitHub account, shown in Settings. Not a secret.
    github_login: String,
    github_source: String,
    /// Live-mode settings other than the profile (that lives in `aws_profile`).
    live: PersistedLive,
    pull_requests: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PersistedLive {
    aws_region: String,
    aws_auth: AwsAuthKind,
    runner_image_ref: String,
    bench_image_ref: String,
    tofu_binary: String,
    module_dir: String,
}

impl Default for PersistedLive {
    fn default() -> Self {
        let d = LiveSettings::default();
        Self {
            aws_region: d.aws_region,
            aws_auth: d.aws_auth,
            runner_image_ref: d.runner_image_ref,
            bench_image_ref: d.bench_image_ref,
            tofu_binary: d.tofu_binary,
            module_dir: d.module_dir,
        }
    }
}

fn connector_secret_name(name: SecretName) -> ratchet_connectors::SecretName {
    match name {
        SecretName::AnthropicApiKey => ratchet_connectors::SecretName::AnthropicApiKey,
        SecretName::OpenaiApiKey => ratchet_connectors::SecretName::OpenaiApiKey,
        SecretName::GithubToken => ratchet_connectors::SecretName::GithubToken,
    }
}

/// The settings file as the connector store's migration source.
impl LegacySource for SettingsStore {
    fn read(&self) -> LegacyAccounts {
        self.legacy_accounts()
    }

    fn clear(&self) {
        self.clear_legacy();
    }
}

pub struct SettingsStore {
    file: Option<PathBuf>,
    secrets: Arc<dyn SecretStore>,
    env: HashMap<String, String>,
    data: Mutex<Persisted>,
}

impl SettingsStore {
    /// `file`: where to persist (`None` = memory only). `env`: the environment credentials are also read from.
    pub fn new(
        file: Option<PathBuf>,
        secrets: Arc<dyn SecretStore>,
        env: HashMap<String, String>,
    ) -> Self {
        let data = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            // a corrupt settings file: start from defaults rather than crash the app
            .and_then(|t| serde_json::from_str::<Persisted>(&t).ok())
            .unwrap_or_default();
        Self {
            file,
            secrets,
            env,
            data: Mutex::new(data),
        }
    }

    fn data(&self) -> std::sync::MutexGuard<'_, Persisted> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn aws_profile(&self) -> String {
        self.data().aws_profile.clone()
    }

    /// Every live-mode setting that is not a secret.
    pub fn live(&self) -> LiveSettings {
        let d = self.data();
        LiveSettings {
            aws_profile: d.aws_profile.clone(),
            aws_region: d.live.aws_region.clone(),
            aws_auth: d.live.aws_auth,
            runner_image_ref: d.live.runner_image_ref.clone(),
            bench_image_ref: d.live.bench_image_ref.clone(),
            tofu_binary: d.live.tofu_binary.clone(),
            module_dir: d.live.module_dir.clone(),
        }
    }

    /// `aws_region`, or the default when empty.
    pub fn region(&self) -> String {
        let r = self.live().aws_region;
        if r.is_empty() {
            DEFAULT_AWS_REGION.into()
        } else {
            r
        }
    }

    pub fn encryption_available(&self) -> bool {
        self.secrets.available()
    }

    /// The secret store (the connector store keeps its tokens and keys in the same one).
    pub fn secret_store(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }

    /// The environment credentials are also read from.
    pub fn env(&self) -> &HashMap<String, String> {
        &self.env
    }

    /// A file next to the settings file (`connectors.json`), `None` when the settings are memory only.
    pub fn sibling_file(&self, name: &str) -> Option<PathBuf> {
        self.file.as_ref().map(|f| f.with_file_name(name))
    }

    /// The single GitHub token / AWS profile / API keys of the first versions, read once to become connectors. `locked` = a
    /// secret is stored but cannot be read right now (keychain unavailable): migration waits for the next launch instead of
    /// dropping it.
    pub fn legacy_accounts(&self) -> LegacyAccounts {
        let mut secrets = BTreeMap::new();
        let mut locked = false;
        for name in SecretName::ALL {
            match self.secrets.get(name.as_str()) {
                Ok(Some(v)) => {
                    secrets.insert(connector_secret_name(name), v);
                }
                Ok(None) => {}
                Err(_) => locked = true,
            }
        }
        let d = self.data();
        LegacyAccounts {
            aws_profile: d.aws_profile.clone(),
            aws_region: d.live.aws_region.clone(),
            github_login: d.github_login.clone(),
            github_via_device: match d.github_source.as_str() {
                "device" => Some(true),
                "token" => Some(false),
                _ => None,
            },
            secrets,
            locked,
        }
    }

    /// Forget the old single-account fields (they became connectors).
    pub fn clear_legacy(&self) {
        for name in SecretName::ALL {
            let _ = self.secrets.delete(name.as_str());
        }
        {
            let mut d = self.data();
            d.aws_profile.clear();
            d.github_login.clear();
            d.github_source.clear();
        }
        self.save();
    }

    pub fn set_aws_profile(&self, profile: &str) {
        self.data().aws_profile = profile.to_owned();
        self.save();
    }

    /// Validate and store a batch of live settings; nothing is saved when any field is invalid.
    pub fn set_live(&self, patch: &LiveSettingsInput) -> ServiceResult<()> {
        let errors = validate_live_settings(patch);
        if let Some(first) = errors.first() {
            return Err(ServiceError::InvalidFields {
                message: format!("{}: {}", first.path, first.message),
                fields: errors,
            });
        }
        {
            let mut d = self.data();
            let trim = |v: &Option<String>| v.as_ref().map(|s| s.trim().to_owned());
            if let Some(v) = trim(&patch.aws_profile) {
                d.aws_profile = v;
            }
            if let Some(v) = trim(&patch.aws_region) {
                d.live.aws_region = v;
            }
            if let Some(v) = patch.aws_auth {
                d.live.aws_auth = v;
            }
            if let Some(v) = trim(&patch.runner_image_ref) {
                d.live.runner_image_ref = v;
            }
            if let Some(v) = trim(&patch.bench_image_ref) {
                d.live.bench_image_ref = v;
            }
            if let Some(v) = trim(&patch.tofu_binary) {
                d.live.tofu_binary = v;
            }
            if let Some(v) = trim(&patch.module_dir) {
                d.live.module_dir = v;
            }
        }
        self.save();
        Ok(())
    }

    pub fn pr_url(&self, id: &str) -> Option<String> {
        self.data().pull_requests.get(id).cloned()
    }

    pub fn set_pr_url(&self, id: &str, url: &str) {
        self.data()
            .pull_requests
            .insert(id.to_owned(), url.to_owned());
        self.save();
    }

    /// The signed-in GitHub account (non-secret), empty when unknown.
    pub fn github_login(&self) -> String {
        self.data().github_login.clone()
    }

    pub fn github_source(&self) -> Option<GithubSource> {
        match self.data().github_source.as_str() {
            "device" => Some(GithubSource::Device),
            "token" => Some(GithubSource::Token),
            "env" => Some(GithubSource::Env),
            _ => None,
        }
    }

    pub fn set_github_account(&self, login: &str, source: GithubSource) {
        {
            let mut d = self.data();
            d.github_login = login.to_owned();
            d.github_source = match source {
                GithubSource::Device => "device",
                GithubSource::Token => "token",
                GithubSource::Env => "env",
            }
            .into();
        }
        self.save();
    }

    pub fn clear_github_account(&self) {
        {
            let mut d = self.data();
            d.github_login.clear();
            d.github_source.clear();
        }
        self.save();
    }

    pub fn set_secret(&self, name: SecretName, value: &str) -> ServiceResult<()> {
        if !self.secrets.available() {
            return Err(ServiceError::Conflict(
                "OS keychain encryption is unavailable; refusing to store the secret in plaintext"
                    .into(),
            ));
        }
        self.secrets
            .set(name.as_str(), value)
            .map_err(|e| ServiceError::Internal(e.to_string()))
    }

    pub fn clear_secret(&self, name: SecretName) -> ServiceResult<()> {
        self.secrets
            .delete(name.as_str())
            .map_err(|e| ServiceError::Internal(e.to_string()))
    }

    fn stored(&self, name: SecretName) -> Option<String> {
        self.secrets.get(name.as_str()).ok().flatten()
    }

    /// The value of a secret from the process environment only (never the store).
    pub fn env_secret(&self, name: SecretName) -> Option<String> {
        self.env_value(name)
    }

    fn env_value(&self, name: SecretName) -> Option<String> {
        env_for(name)
            .iter()
            .find_map(|k| self.env.get(*k).filter(|v| !v.is_empty()).cloned())
    }

    /// Configured = stored in the keychain, or provided through the environment.
    pub fn has_secret(&self, name: SecretName) -> bool {
        self.stored(name).is_some() || self.env_value(name).is_some()
    }

    /// Daemon only: hand to the engine, never to a client.
    pub fn get_secret(&self, name: SecretName) -> Option<String> {
        self.stored(name).or_else(|| self.env_value(name))
    }

    /// Whether the value comes from the store (rather than only the environment).
    pub fn secret_is_stored(&self, name: SecretName) -> bool {
        self.stored(name).is_some()
    }

    pub fn secrets_present(&self) -> SecretsPresent {
        SecretsPresent {
            anthropic_api_key: self.has_secret(SecretName::AnthropicApiKey),
            openai_api_key: self.has_secret(SecretName::OpenaiApiKey),
            github_token: self.has_secret(SecretName::GithubToken),
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let text = {
            let d = self.data();
            serde_json::to_string_pretty(&*d).unwrap_or_default()
        };
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
            tracing::warn!("cannot save settings to {}: {e}", file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use ratchet_engine::desktop::{FileSecretStore, MemorySecretStore};

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect()
    }

    /// settings.test.ts › SettingsStore secrets › stores secrets encrypted, round-trips them in main, and persists
    /// across instances (the secret goes to the SecretStore and never into settings.json)
    #[test]
    fn secrets_go_to_the_secret_store_never_the_settings_file_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let secrets: Arc<dyn SecretStore> =
            Arc::new(FileSecretStore::new(dir.path().join("secrets.json")));
        let s = SettingsStore::new(Some(file.clone()), secrets.clone(), HashMap::new());
        s.set_aws_profile("p");
        s.set_secret(SecretName::AnthropicApiKey, "sk-ant-SUPERSECRET")
            .unwrap();
        assert!(
            !std::fs::read_to_string(&file)
                .unwrap()
                .contains("SUPERSECRET")
        );
        assert_eq!(
            s.get_secret(SecretName::AnthropicApiKey).as_deref(),
            Some("sk-ant-SUPERSECRET")
        );
        let again = SettingsStore::new(Some(file), secrets, HashMap::new());
        assert_eq!(again.aws_profile(), "p");
        assert!(again.has_secret(SecretName::AnthropicApiKey));
        assert_eq!(
            again.get_secret(SecretName::AnthropicApiKey).as_deref(),
            Some("sk-ant-SUPERSECRET")
        );
        again.clear_secret(SecretName::AnthropicApiKey).unwrap();
        assert!(!again.has_secret(SecretName::AnthropicApiKey));
    }

    /// settings.test.ts › SettingsStore secrets › refuses to store secrets when the keychain is unavailable
    #[test]
    fn refuses_to_store_secrets_when_the_keychain_is_unavailable() {
        let s = SettingsStore::new(
            None,
            Arc::new(MemorySecretStore::unavailable()),
            HashMap::new(),
        );
        let err = s
            .set_secret(SecretName::GithubToken, "ghp_123456789")
            .unwrap_err();
        assert!(err.to_string().contains("unavailable"), "{err}");
        assert!(!s.encryption_available());
    }

    /// settings.test.ts › SettingsStore secrets › treats environment-provided credentials as configured
    #[test]
    fn environment_credentials_count_as_configured() {
        let s = SettingsStore::new(
            None,
            Arc::new(MemorySecretStore::new()),
            env(&[("GH_TOKEN", "ghp_x")]),
        );
        assert!(s.has_secret(SecretName::GithubToken));
        assert!(!s.has_secret(SecretName::OpenaiApiKey));
        assert_eq!(
            s.get_secret(SecretName::GithubToken).as_deref(),
            Some("ghp_x")
        );
        assert!(!s.secret_is_stored(SecretName::GithubToken));
    }

    /// settings.test.ts › SettingsStore secrets › survives a corrupt settings file
    #[test]
    fn survives_a_corrupt_settings_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, "{not json").unwrap();
        let s = SettingsStore::new(
            Some(file),
            Arc::new(MemorySecretStore::new()),
            HashMap::new(),
        );
        assert_eq!(s.aws_profile(), "");
        assert_eq!(s.region(), DEFAULT_AWS_REGION);
    }

    #[test]
    fn a_batch_with_an_invalid_field_saves_nothing() {
        let s = SettingsStore::new(None, Arc::new(MemorySecretStore::new()), HashMap::new());
        let bad = LiveSettingsInput {
            aws_region: Some("eu-west-2".into()),
            bench_image_ref: Some("not-an-ami".into()),
            ..Default::default()
        };
        assert!(matches!(
            s.set_live(&bad),
            Err(ServiceError::InvalidFields { .. })
        ));
        assert_eq!(s.region(), DEFAULT_AWS_REGION);
        let ok = LiveSettingsInput {
            aws_region: Some(" eu-west-2 ".into()),
            aws_profile: Some("dev".into()),
            ..Default::default()
        };
        s.set_live(&ok).unwrap();
        assert_eq!(
            (s.region().as_str(), s.aws_profile().as_str()),
            ("eu-west-2", "dev")
        );
    }

    #[test]
    fn the_github_account_and_pr_urls_persist() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let mk = || {
            SettingsStore::new(
                Some(file.clone()),
                Arc::new(MemorySecretStore::new()),
                HashMap::new(),
            )
        };
        let s = mk();
        s.set_github_account("octo", GithubSource::Device);
        s.set_pr_url("opt-1", "https://github.com/a/b/pull/1");
        let again = mk();
        assert_eq!(again.github_login(), "octo");
        assert_eq!(again.github_source(), Some(GithubSource::Device));
        assert_eq!(
            again.pr_url("opt-1").as_deref(),
            Some("https://github.com/a/b/pull/1")
        );
        again.clear_github_account();
        assert_eq!(mk().github_source(), None);
    }
}
