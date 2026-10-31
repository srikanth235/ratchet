//! Connectors (PROVISIONAL): saved GitHub / AWS / agent accounts, and the per-repo selection.
//!
//! Shapes come from `design/v02/2-connectors.png`, `3-new-connections.png` and `docs/SIMPLIFIED_MODEL.md`. The
//! TypeScript feature is still in flight on another branch, so every type here may change before it lands; the
//! routes are tagged `connectors (provisional)` in the OpenAPI document. Secrets (tokens, API keys) are written
//! once and live in the keychain; they never come back in a response.

use ratchet_core::enums::{AgentKind, AgentsWhere};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorKind {
    Github,
    Aws,
    Agent,
}

/// `connected` / `signed-in` / `valid` all read as green in the UI; the text says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectorStatus {
    Connected,
    /// SSO session or token ran out; "Sign in" refreshes it.
    Expired,
    SignedOut,
    Invalid,
    /// Not tested yet.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GithubAccountKind {
    Personal,
    Organization,
    App,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubConnectorInfo {
    pub login: String,
    pub account_kind: GithubAccountKind,
    pub repo_count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AwsConnectorAuth {
    Sso,
    /// A named profile in `~/.aws`.
    Profile,
    /// The one-click CloudFormation role.
    Role,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AwsConnectorInfo {
    pub auth: AwsConnectorAuth,
    /// Profile or SSO session name; `null` for a role.
    pub profile: Option<String>,
    pub region: String,
    pub account_id: Option<String>,
    /// Monthly spend cap in USD, when the user set one.
    pub cap_usd_per_month: Option<f64>,
    /// What is left of the cap this month ("$412 left this month").
    pub remaining_usd_this_month: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AgentCredential {
    /// The CLI's own sign-in on this Mac.
    Login,
    /// An API key held in the keychain.
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentConnectorInfo {
    pub agent: AgentKind,
    pub credential: AgentCredential,
    /// CLI version for a login connector.
    pub version: Option<String>,
    /// Last characters of an API key (`····4f2a`).
    pub key_hint: Option<String>,
    /// Where this connector can run agents: a login runs on this Mac, a key also in the cloud.
    pub runs_in: Vec<AgentsWhere>,
}

/// One saved connector. Exactly one of `github` / `aws` / `agent` is set, matching `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorVm {
    pub id: String,
    pub kind: ConnectorKind,
    /// "acme-prod", "Claude Code · your login".
    pub name: String,
    /// The grey line: "Personal · 42 repos", "SSO · us-east-1 · cap $500 / month".
    pub detail: String,
    pub status: ConnectorStatus,
    /// "Connected", "Expired", "Signed out".
    pub status_text: String,
    /// The default of its kind: preselected for new optimizations.
    pub is_default: bool,
    /// Cost line on the New optimization step ("Free while this Mac is awake", "Billed to the key").
    pub cost_note: Option<String>,
    pub github: Option<GithubConnectorInfo>,
    pub aws: Option<AwsConnectorInfo>,
    pub agent: Option<AgentConnectorInfo>,
}

/// `POST /v1/connectors`: what the "Add connector" sheet sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CreateConnectorInput {
    /// Starts the device flow (or stores a pasted token) and saves the account.
    Github {
        name: Option<String>,
        /// A pasted personal access token (Advanced). Without it the device flow starts.
        token: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Aws {
        name: String,
        auth: AwsConnectorAuth,
        profile: Option<String>,
        region: Option<String>,
        cap_usd_per_month: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Agent {
        name: String,
        agent: AgentKind,
        credential: AgentCredential,
        /// The API key, for `credential: api-key`.
        api_key: Option<String>,
        runs_in: Option<Vec<AgentsWhere>>,
    },
}

/// `PATCH /v1/connectors/{id}`: rename, change the cap, make default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConnectorInput {
    pub name: Option<String>,
    pub is_default: Option<bool>,
    pub cap_usd_per_month: Option<f64>,
    pub region: Option<String>,
    /// Replace the stored API key.
    pub api_key: Option<String>,
}

/// One agent choice on the New optimization step: a connector and where it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentSelection {
    pub connector_id: String,
    pub runs_in: AgentsWhere,
}

/// The connectors a run uses (step 2 of New optimization).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSelectionInput {
    pub github_connector_id: Option<String>,
    pub aws_connector_id: Option<String>,
    pub agents: Vec<AgentSelection>,
}

/// `GET /v1/connector-selections?repo=`: the remembered choice for a repo, or the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSelectionVm {
    pub repo: String,
    pub selection: ConnectorSelectionInput,
    /// True when the user chose it before for this repo ("Remembered for this repo"); false = the defaults.
    pub remembered: bool,
}

/// `PUT /v1/connector-selections`: remember the choice for a repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveConnectorSelectionInput {
    pub repo: String,
    pub selection: ConnectorSelectionInput,
}

/// Result of "Test" / "Sign in" on a connector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorTestVm {
    pub connector: ConnectorVm,
    pub ok: bool,
    /// One line; never contains a key or token.
    pub detail: String,
}
