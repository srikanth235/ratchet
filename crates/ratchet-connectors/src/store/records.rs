//! What is persisted about connectors (non-secret metadata, in `connectors.json`). Secrets (GitHub tokens, API keys) are
//! never part of a record: they sit in the [`SecretStore`](crate::secrets::SecretStore) under `connector:<id>`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{AgentConnectorSource, ConnectorAgent, ConnectorKind, RepoConnections};

/// How a stored GitHub account got its token (the environment's token is not stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GithubStored {
    Device,
    Token,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum RecordKind {
    Github {
        login: String,
        source: GithubStored,
    },
    Aws {
        profile: String,
        region: String,
        monthly_cap_usd: Option<f64>,
    },
    Agent {
        agent: ConnectorAgent,
        source: AgentConnectorSource,
        /// `····4f2a` for a stored key (non-secret).
        key_hint: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorRecord {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    #[serde(flatten)]
    pub kind: RecordKind,
}

impl ConnectorRecord {
    pub fn connector_kind(&self) -> ConnectorKind {
        match self.kind {
            RecordKind::Github { .. } => ConnectorKind::Github,
            RecordKind::Aws { .. } => ConnectorKind::Aws,
            RecordKind::Agent { .. } => ConnectorKind::Agent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorsPersisted {
    /// The single GitHub token / AWS profile / API keys of the first versions were turned into connectors.
    pub migrated: bool,
    pub items: Vec<ConnectorRecord>,
    /// One default connector id per kind.
    pub defaults: BTreeMap<ConnectorKind, String>,
    /// Detected CLI logins the user removed (not added again automatically).
    pub dismissed: Vec<String>,
    /// Remembered choice per repo (key: lower-cased `owner/name`).
    pub selections: BTreeMap<String, RepoConnections>,
}
