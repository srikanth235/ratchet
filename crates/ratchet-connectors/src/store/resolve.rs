//! What a run uses, with the secrets in it: daemon only, never serialized to a client.

use std::collections::BTreeMap;
use std::fmt;

use ratchet_core::enums::AgentsWhere;

use crate::model::{AgentConnectorSource, ConnectorAgent};

pub struct ResolvedGithub {
    pub id: String,
    pub name: String,
    pub login: Option<String>,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAws {
    pub id: String,
    pub name: String,
    pub profile: String,
    pub region: String,
    pub monthly_cap_usd: Option<f64>,
}

pub struct ResolvedAgent {
    pub connector_id: String,
    pub name: String,
    pub agent: ConnectorAgent,
    pub source: AgentConnectorSource,
    pub where_: AgentsWhere,
    /// The stored key of an `api-key` connector (`None` for a CLI login).
    pub api_key: Option<String>,
}

pub struct ResolvedConnections {
    pub github: Option<ResolvedGithub>,
    pub aws: Option<ResolvedAws>,
    /// The chosen agents (the remembered selection for the repo, else the default connector of each agent).
    pub agents: Vec<ResolvedAgent>,
    /// The key a cloud runner of each agent gets (only from a chosen or default `api-key` connector; never from a login).
    pub cloud_keys: BTreeMap<ConnectorAgent, String>,
}

impl ResolvedConnections {
    /// The agents that run in `where_`, if any were chosen.
    pub fn agent_kinds(&self) -> Vec<ConnectorAgent> {
        let mut out: Vec<ConnectorAgent> = Vec::new();
        for a in &self.agents {
            if !out.contains(&a.agent) {
                out.push(a.agent);
            }
        }
        out
    }
}

// Debug output never carries a secret.
impl fmt::Debug for ResolvedGithub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedGithub")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("login", &self.login)
            .field("token", &"***")
            .finish()
    }
}

impl fmt::Debug for ResolvedAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedAgent")
            .field("connector_id", &self.connector_id)
            .field("name", &self.name)
            .field("agent", &self.agent)
            .field("source", &self.source)
            .field("where", &self.where_)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .finish()
    }
}

impl fmt::Debug for ResolvedConnections {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedConnections")
            .field("github", &self.github)
            .field("aws", &self.aws)
            .field("agents", &self.agents)
            .field("cloud_keys", &self.cloud_keys.keys().collect::<Vec<_>>())
            .finish()
    }
}
