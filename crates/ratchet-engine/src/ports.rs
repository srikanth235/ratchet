//! Everything the engine talks to, as the port traits from `ratchet-core`. Port of `packages/engine/src/ports.ts`.

use std::sync::Arc;

use ratchet_core::ports::{
    ArtifactStore, BenchRunner, CloudCompute, CostReconciler, GitHost, PriceBook, RunnerControl,
    SandboxProvider,
};

/// One git host, or a factory when the app drives several repositories.
#[derive(Clone)]
pub enum GitSource {
    One(Arc<dyn GitHost>),
    Factory(Arc<dyn Fn(&str) -> Arc<dyn GitHost> + Send + Sync>),
}

impl GitSource {
    pub fn for_repo(&self, repo_url: &str) -> Arc<dyn GitHost> {
        match self {
            Self::One(g) => g.clone(),
            Self::Factory(f) => f(repo_url),
        }
    }
}

impl From<Arc<dyn GitHost>> for GitSource {
    fn from(g: Arc<dyn GitHost>) -> Self {
        Self::One(g)
    }
}

#[derive(Clone)]
pub struct EnginePorts {
    /// Plan / apply / destroy go through the engine (`plan_sandbox`, `apply_sandbox`, `destroy_sandbox`) so the ledger
    /// records them.
    pub sandbox: Arc<dyn SandboxProvider>,
    pub compute: Arc<dyn CloudCompute>,
    pub prices: Arc<dyn PriceBook>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub git: GitSource,
    /// Cloud agent runners (EC2 + SSM).
    pub runners: Arc<dyn RunnerControl>,
    /// Agents on the user's machine (their own CLI login, a git worktree each). Optional; see
    /// `EngineSettings::agents_where`.
    pub local_runners: Option<Arc<dyn RunnerControl>>,
    pub bench: Arc<dyn BenchRunner>,
    /// Billing truth for the cost estimate (Cost Explorer). Optional: without it `reconcile_costs` reports it is
    /// unavailable.
    pub costs: Option<Arc<dyn CostReconciler>>,
    /// Drafts, revises and approves environments (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"). Optional: without
    /// it `draft_environment` & co. report `unavailable`. Applying an approved environment needs none of this: the engine
    /// reads it from the protected ref at run start.
    pub environment: Option<Arc<dyn ratchet_env::EnvironmentDrafter>>,
}
