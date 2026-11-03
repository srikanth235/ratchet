//! The Ratchet MCP server's side of the daemon.
//!
//! * [`Sessions`]: capability tokens for harness sessions (issue at start, revoke at close). A token opens `/v1/mcp/*` and one
//!   tool set, nothing else.
//! * [`routes`]: `POST /v1/mcp/call` and `GET /v1/mcp/session`, authenticated by a session token instead of the owner's.
//!   `ratchetd mcp --session <token>` is a stdio server that forwards to them (see `ratchet_mcp::socket`).
//! * [`tools`]: what each chat tool does, in terms of [`AppService`](crate::service::AppService).
//! * [`config_branch`]: `draft_optimization` writes the config to the `ratchet/config` branch only.

pub mod config_branch;
pub mod routes;
pub mod sessions;
pub mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use async_trait::async_trait;
use ratchet_core::ports::GitHost;
use serde::Serialize;

use crate::service::{ServiceError, ServiceResult};

pub use sessions::{IssuedSession, Scope, SessionInfo, Sessions};

/// A read-only question to one of the user's connectors (`connector_query`). The real daemon answers from the connectors the
/// user signed in; a connector that is signed out is the one thing a person has to fix, and the error says which.
#[async_trait]
pub trait ConnectorQuery: Send + Sync {
    async fn query(&self, connector: &str, query: &str) -> ServiceResult<serde_json::Value>;
}

/// Git hosts by repo, for committing a draft's config. The real daemon resolves them from the GitHub sign-in.
#[async_trait]
pub trait ConfigHosts: Send + Sync {
    async fn host_for(&self, repo: &str) -> ServiceResult<Arc<dyn GitHost>>;

    /// The session store the daemon's own agent launcher issues tokens from; the hub shares it, so a token the launcher
    /// issues opens `/v1/mcp/*`. `None`: the hub keeps its own.
    fn sessions(&self) -> Option<Arc<Sessions>> {
        None
    }

    /// Who answers `connector_query`. `None`: no connector can be asked.
    fn connector_query(&self) -> Option<Arc<dyn ConnectorQuery>> {
        None
    }
}

/// An in-memory git host per daemon, for the simulation and tests: nothing leaves the process.
pub struct MockHosts(Arc<ratchet_github::MockGitHost>);

impl Default for MockHosts {
    fn default() -> Self {
        Self::new()
    }
}

impl MockHosts {
    pub fn new() -> Self {
        Self(Arc::new(ratchet_github::MockGitHost::new(
            std::collections::BTreeMap::new(),
        )))
    }
}

#[async_trait]
impl ConfigHosts for MockHosts {
    async fn host_for(&self, _repo: &str) -> ServiceResult<Arc<dyn GitHost>> {
        Ok(self.0.clone())
    }
}

/// A drafted optimization: the card the chat shows, and what `create_optimization` needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationDraft {
    pub draft_id: String,
    pub repo: String,
    /// The branch the optimization starts from once created: `ratchet/config` (the base ref plus the config commit).
    pub base_ref: String,
    pub config_branch: String,
    /// The commit that carries the config.
    pub commit: String,
    pub goal: String,
    pub template: String,
    pub metric: String,
    pub direction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    pub budget_usd: f64,
    pub agents: Vec<String>,
    /// Paths written to the config branch.
    pub files: Vec<String>,
    /// Set once `create_optimization` ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_optimization: Option<String>,
}

/// Everything the MCP side of the daemon keeps.
#[derive(Default)]
pub struct McpHub {
    pub sessions: Arc<Sessions>,
    drafts: Mutex<HashMap<String, OptimizationDraft>>,
    hosts: RwLock<Option<Arc<dyn ConfigHosts>>>,
    connector_query: RwLock<Option<Arc<dyn ConnectorQuery>>>,
}

impl McpHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// With the git hosts drafts are committed through.
    pub fn with_hosts(hosts: Arc<dyn ConfigHosts>) -> Self {
        let hub = Self {
            sessions: hosts.sessions().unwrap_or_default(),
            ..Self::default()
        };
        hub.set_hosts(hosts);
        hub
    }

    pub fn set_hosts(&self, hosts: Arc<dyn ConfigHosts>) {
        if let Some(q) = hosts.connector_query() {
            self.set_connector_query(q);
        }
        *self.hosts.write().unwrap_or_else(PoisonError::into_inner) = Some(hosts);
    }

    pub fn set_connector_query(&self, q: Arc<dyn ConnectorQuery>) {
        *self
            .connector_query
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(q);
    }

    pub(crate) fn connector_querier(&self) -> Option<Arc<dyn ConnectorQuery>> {
        self.connector_query
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) async fn host_for(&self, repo: &str) -> ServiceResult<Arc<dyn GitHost>> {
        let hosts = self
            .hosts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match hosts {
            Some(h) => h.host_for(repo).await,
            None => Err(ServiceError::NotReady(
                "no git host is configured for drafts; sign in to GitHub in Settings".into(),
            )),
        }
    }

    fn drafts(&self) -> MutexGuard<'_, HashMap<String, OptimizationDraft>> {
        self.drafts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn put_draft(&self, d: OptimizationDraft) {
        self.drafts().insert(d.draft_id.clone(), d);
    }

    pub fn draft(&self, id: &str) -> Option<OptimizationDraft> {
        self.drafts().get(id).cloned()
    }

    pub(crate) fn mark_created(&self, id: &str, optimization: &str) {
        if let Some(d) = self.drafts().get_mut(id) {
            d.created_optimization = Some(optimization.to_owned());
        }
    }
}
