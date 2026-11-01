//! Settings, GitHub sign-in, AWS profiles, detected agent CLIs, connection tests.
//!
//! Secrets never appear in a response: `SettingsVm.secrets` only says whether one is stored.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::vm::EngineInfo;

/// `profile` = a named profile with static keys or an assume-role; `sso` = a profile created by `aws configure sso`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AwsAuthKind {
    Profile,
    Sso,
}

/// Everything a client may read and write for live mode (no secrets). Empty string = not set / use the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LiveSettings {
    pub aws_profile: String,
    pub aws_region: String,
    pub aws_auth: AwsAuthKind,
    /// AMI that agent runners boot from.
    pub runner_image_ref: String,
    /// AMI that bench / confirm instances boot from.
    pub bench_image_ref: String,
    /// Path to the `tofu` binary; empty = the bundled one, else `tofu` on PATH.
    pub tofu_binary: String,
    /// Directory of the OpenTofu module; empty = the bundled `infra/aws`.
    pub module_dir: String,
}

pub const DEFAULT_AWS_REGION: &str = "us-east-1";

impl Default for LiveSettings {
    fn default() -> Self {
        Self {
            aws_profile: String::new(),
            aws_region: DEFAULT_AWS_REGION.into(),
            aws_auth: AwsAuthKind::Profile,
            runner_image_ref: String::new(),
            bench_image_ref: String::new(),
            tofu_binary: String::new(),
            module_dir: String::new(),
        }
    }
}

/// `PUT /v1/settings/advanced`: any subset of the live settings; validated as a whole, nothing is saved if any
/// field is invalid.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LiveSettingsInput {
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
    pub aws_auth: Option<AwsAuthKind>,
    pub runner_image_ref: Option<String>,
    pub bench_image_ref: Option<String>,
    pub tofu_binary: Option<String>,
    pub module_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub enum SecretName {
    #[serde(rename = "anthropicApiKey")]
    AnthropicApiKey,
    #[serde(rename = "openaiApiKey")]
    OpenaiApiKey,
    #[serde(rename = "githubToken")]
    GithubToken,
}

impl SecretName {
    pub const ALL: [SecretName; 3] = [Self::AnthropicApiKey, Self::OpenaiApiKey, Self::GithubToken];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicApiKey => "anthropicApiKey",
            Self::OpenaiApiKey => "openaiApiKey",
            Self::GithubToken => "githubToken",
        }
    }
}

impl std::str::FromStr for SecretName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|n| n.as_str() == s)
            .ok_or_else(|| format!("unknown secret {s:?}"))
    }
}

/// Which secrets are stored (never their values).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecretsPresent {
    pub anthropic_api_key: bool,
    pub openai_api_key: bool,
    pub github_token: bool,
}

impl SecretsPresent {
    pub fn get(&self, n: SecretName) -> bool {
        match n {
            SecretName::AnthropicApiKey => self.anthropic_api_key,
            SecretName::OpenaiApiKey => self.openai_api_key,
            SecretName::GithubToken => self.github_token,
        }
    }

    pub fn set(&mut self, n: SecretName, present: bool) {
        match n {
            SecretName::AnthropicApiKey => self.anthropic_api_key = present,
            SecretName::OpenaiApiKey => self.openai_api_key = present,
            SecretName::GithubToken => self.github_token = present,
        }
    }
}

/// What empty `tofuBinary` / `moduleDir` resolve to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedDefaults {
    pub tofu_binary: String,
    pub module_dir: String,
}

/// The "Advanced (development)" disclosure of Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSettingsVm {
    pub values: LiveSettings,
    pub defaults: AdvancedDefaults,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsVm {
    pub aws_profile: String,
    pub aws_region: String,
    pub secrets: SecretsPresent,
    pub advanced: AdvancedSettingsVm,
    /// False when the OS keychain is unavailable, so secrets cannot be stored.
    pub encryption_available: bool,
    pub engine: EngineInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetAwsProfileInput {
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetSecretInput {
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionTarget {
    Aws,
    Github,
    Anthropic,
    Openai,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionInput {
    pub target: ConnectionTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionResultVm {
    pub target: ConnectionTarget,
    pub ok: bool,
    /// One line; never contains a key or token.
    pub detail: String,
}

// ---- GitHub -------------------------------------------------------------------------------------------------

/// `device`: "Sign in with GitHub"; `token`: pasted (Advanced); `env`: GITHUB_TOKEN / GH_TOKEN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GithubSource {
    Device,
    Token,
    Env,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GithubFlowState {
    Pending,
    Expired,
    Denied,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubFlowVm {
    pub state: GithubFlowState,
    /// Shown to the user, who types it at `verificationUri`.
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: i64,
    #[schema(required = true)]
    pub message: Option<String>,
}

/// The GitHub row of Settings. The token itself never leaves the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubStatusVm {
    pub signed_in: bool,
    #[schema(required = true)]
    pub login: Option<String>,
    #[schema(required = true)]
    pub source: Option<GithubSource>,
    /// False while the built-in placeholder OAuth client id is in use.
    pub client_id_configured: bool,
    /// The device-flow sign-in in progress (or the reason the last one ended).
    #[schema(required = true)]
    pub flow: Option<GithubFlowVm>,
}

// ---- AWS ----------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AwsProfileKind {
    Sso,
    Role,
    Keys,
    Process,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AwsProfileSource {
    Config,
    Credentials,
}

/// A profile found in `~/.aws/config` or `~/.aws/credentials` (names only, never a key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AwsProfileVm {
    pub name: String,
    pub kind: AwsProfileKind,
    pub source: AwsProfileSource,
    /// The `sso-session` a profile uses.
    #[schema(required = true)]
    pub sso_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AwsProfilesVm {
    pub profiles: Vec<AwsProfileVm>,
    /// `[sso-session x]` names in `~/.aws/config`.
    pub sso_sessions: Vec<String>,
    /// The profile Ratchet uses (empty = none chosen).
    pub selected: String,
    pub region: String,
    pub default_region: String,
    /// False when neither file exists.
    pub files_found: bool,
}

// ---- agent CLIs ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum HarnessKind {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "grok")]
    Grok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum HarnessAuth {
    #[serde(rename = "signed-in")]
    SignedIn,
    #[serde(rename = "api-key")]
    ApiKey,
    #[serde(rename = "signed-out")]
    SignedOut,
}

/// One detected agent CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessVm {
    pub kind: HarnessKind,
    pub installed: bool,
    #[schema(required = true)]
    pub version: Option<String>,
    #[schema(required = true)]
    pub path: Option<String>,
    pub auth: HarnessAuth,
    /// What to do next, or what was found. Never contains a credential.
    pub hint: String,
}
