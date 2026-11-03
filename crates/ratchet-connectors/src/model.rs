//! Connectors: the accounts an optimization can use, several of each kind, stored once and picked per run; and the
//! choice remembered per repo.
//!
//! The wire types: `packages/desktop/src/shared/connectors.ts`, one to one (`ratchetd` publishes them in its OpenAPI
//! document, which is why they derive `ToSchema` behind the `utoipa` feature). Secrets (GitHub tokens, API keys) are
//! written once and live in the keychain; they are never part of any response.

use ratchet_core::enums::AgentsWhere;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ConnectorKind {
    Github,
    Aws,
    Agent,
}

/// The agent CLIs Ratchet drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum ConnectorAgent {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "grok")]
    Grok,
}

impl ConnectorAgent {
    /// What the user reads ("Claude Code").
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
        }
    }

    /// The engine's agent kind.
    pub fn kind(self) -> ratchet_core::enums::AgentKind {
        match self {
            Self::ClaudeCode => ratchet_core::enums::AgentKind::ClaudeCode,
            Self::Codex => ratchet_core::enums::AgentKind::Codex,
            Self::Grok => ratchet_core::enums::AgentKind::Grok,
        }
    }
}

/// - `connected`: a GitHub account / AWS profile / agent login that works
/// - `expired`: it worked once and needs signing in again (GitHub token revoked, `aws sso login` needed)
/// - `signed-out`: nothing to use (an agent CLI that is not signed in or not installed, a GitHub account without a token)
/// - `valid`: an API key the provider accepted
/// - `invalid`: an API key (or static AWS keys) the provider rejected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ConnectorStatus {
    Connected,
    Expired,
    SignedOut,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum GithubConnectorSource {
    Device,
    Token,
    Env,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AgentConnectorSource {
    Login,
    ApiKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AwsAuthStyle {
    Sso,
    Role,
    Keys,
    Process,
    Other,
}

/// One saved connector. `kind` says which of the three it is; the first fields are common.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ConnectorVm {
    #[serde(rename_all = "camelCase")]
    Github {
        id: String,
        /// What the user sees and can rename ("srikanth235").
        name: String,
        /// One muted line under the name. Backticks mark a command to show in monospace.
        detail: String,
        status: ConnectorStatus,
        /// The default of its kind (one per kind).
        is_default: bool,
        /// The account's login, `null` until it is known.
        #[cfg_attr(feature = "utoipa", schema(required = true))]
        login: Option<String>,
        source: GithubConnectorSource,
    },
    #[serde(rename_all = "camelCase")]
    Aws {
        id: String,
        name: String,
        detail: String,
        status: ConnectorStatus,
        is_default: bool,
        /// Profile name in `~/.aws/config` (names only; credentials stay where AWS keeps them).
        profile: String,
        region: String,
        auth: AwsAuthStyle,
        /// Optional monthly spend cap in USD.
        #[cfg_attr(feature = "utoipa", schema(required = true))]
        monthly_cap_usd: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Agent {
        id: String,
        name: String,
        detail: String,
        status: ConnectorStatus,
        /// For agents: the one used when a run does not choose.
        is_default: bool,
        agent: ConnectorAgent,
        source: AgentConnectorSource,
        /// `login` runs on this Mac only; `api-key` can run here or on cloud runners.
        runs_on: Vec<AgentsWhere>,
        /// Detected CLI version (login) or `null`.
        #[cfg_attr(feature = "utoipa", schema(required = true))]
        version: Option<String>,
        /// Last characters of a stored key, e.g. `····4f2a`. Never the key.
        #[cfg_attr(feature = "utoipa", schema(required = true))]
        key_hint: Option<String>,
    },
}

impl ConnectorVm {
    pub fn id(&self) -> &str {
        match self {
            Self::Github { id, .. } | Self::Aws { id, .. } | Self::Agent { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Github { name, .. } | Self::Aws { name, .. } | Self::Agent { name, .. } => name,
        }
    }

    pub fn kind(&self) -> ConnectorKind {
        match self {
            Self::Github { .. } => ConnectorKind::Github,
            Self::Aws { .. } => ConnectorKind::Aws,
            Self::Agent { .. } => ConnectorKind::Agent,
        }
    }

    pub fn status(&self) -> ConnectorStatus {
        match self {
            Self::Github { status, .. } | Self::Aws { status, .. } | Self::Agent { status, .. } => {
                *status
            }
        }
    }

    pub fn is_default(&self) -> bool {
        match self {
            Self::Github { is_default, .. }
            | Self::Aws { is_default, .. }
            | Self::Agent { is_default, .. } => *is_default,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Github { detail, .. } | Self::Aws { detail, .. } | Self::Agent { detail, .. } => {
                detail
            }
        }
    }
}

// ---- inputs -------------------------------------------------------------------------------------------------

/// `POST /v1/connectors/github`: a pasted personal access token or app installation token, checked against GitHub
/// before it is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AddGithubConnectorInput {
    pub token: String,
    pub name: Option<String>,
}

/// `POST /v1/connectors/github/sign-in`: start the device flow for a new account, or sign `connectorId` in again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct StartGithubSignInInput {
    /// Re-sign-in: store the new token on this connector (default: a new one, or the one with the same login).
    pub connector_id: Option<String>,
}

/// `POST /v1/connectors/aws`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AddAwsConnectorInput {
    /// A profile from `~/.aws/config` or `~/.aws/credentials`.
    pub profile: String,
    /// Default `us-east-1`.
    pub region: Option<String>,
    /// USD per month; omit or `null` for no cap.
    pub monthly_cap_usd: Option<f64>,
    pub name: Option<String>,
}

/// `POST /v1/connectors/agents`: adopt a detected CLI login (one that was removed earlier) or store an API key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "source", rename_all = "kebab-case")]
pub enum AddAgentConnectorInput {
    Login {
        agent: ConnectorAgent,
    },
    #[serde(rename_all = "camelCase")]
    ApiKey {
        agent: ConnectorAgent,
        api_key: String,
        name: Option<String>,
    },
}

/// `PATCH /v1/connectors/{id}`: rename; AWS: region and monthly cap.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct UpdateConnectorInput {
    pub name: Option<String>,
    /// AWS only.
    pub region: Option<String>,
    /// AWS only; `null` removes the cap, absent leaves it alone.
    #[serde(default, deserialize_with = "double_option")]
    #[cfg_attr(feature = "utoipa", schema(value_type = Option<f64>))]
    pub monthly_cap_usd: Option<Option<f64>>,
}

/// `null` is `Some(None)`, absent is `None`.
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Result of "Test connection" on one connector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectorTestVm {
    pub ok: bool,
    /// One line; never contains a key or token.
    pub detail: String,
    /// The connector with its fresh status (`null` if it no longer exists).
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub connector: Option<ConnectorVm>,
}

// ---- per-repo remembered selection --------------------------------------------------------------------------

/// One agent choice: a connector and where it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentSelection {
    pub connector_id: String,
    #[serde(rename = "where")]
    pub where_: AgentsWhere,
}

/// What one repo's runs use: one GitHub account, one AWS profile, and one connector per agent with where it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RepoConnections {
    pub github: String,
    pub aws: String,
    pub agents: Vec<AgentSelection>,
}

/// What a run would use right now: the saved choice where there is one, else the defaults. `null` = nothing
/// connected of that kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EffectiveConnections {
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub github: Option<String>,
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub aws: Option<String>,
    pub agents: Vec<AgentSelection>,
}

/// `GET /v1/connector-selections?repo=`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RepoConnectionsVm {
    /// `owner/name`, lower-cased: the key the selection is remembered under.
    pub repo: String,
    /// What was saved for this repo last time (only entries that still exist); `null` if nothing usable was saved.
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub saved: Option<RepoConnections>,
    pub effective: EffectiveConnections,
}

/// `PUT /v1/connector-selections`: remember the choice for a repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SaveRepoConnectionsInput {
    pub repo: String,
    pub selection: RepoConnections,
}

/// Normalised key of a repo (`Owner/Name` and `owner/name` are the same repo).
pub fn repo_key(repo: &str) -> String {
    repo.trim().to_lowercase()
}
