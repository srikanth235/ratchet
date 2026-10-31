//! The ports of this crate: one trait per external capability, each with a real and a mock implementation so the whole
//! product runs in simulation mode (docs/REAL_ENV_TESTING.md lists which mocks must be re-tested for real).
//!
//! TODO(core): replace with ratchet-core. Mapping to the TypeScript ports of `packages/core/src/ports/index.ts`:
//!
//! | here | TypeScript | real | mock |
//! |---|---|---|---|
//! | [`Compute`] | `CloudCompute` | `AwsCloudCompute` | `MockCloudCompute` |
//! | [`ArtifactStore`] | `ArtifactStore` | `S3ArtifactStore` | `FsArtifactStore` |
//! | [`CostSource`] | `CostReconciler` | `AwsCostReconciler` | `MockCostReconciler` |
//! | [`Pricing`] | `PriceBook` | `AwsPriceBook` | `StaticPriceBook` |
//! | [`ImageLookup`] | `findRatchetImages` | `Ec2ImageLookup` | `MockImageLookup` |
//! | [`SandboxProvider`] | `SandboxProvider` | `TofuSandboxProvider` | `MockSandboxProvider` |

use async_trait::async_trait;

use crate::domain::{
    AgentIdentity, BenchLaunchSpec, CostActuals, CostActualsQuery, InstanceInfo, RemoteCommand,
    RemoteCommandResult, RunnerLaunchSpec, SandboxInput, SandboxOutputs, SandboxPlan, SecretRef,
};
use crate::error::Result;
use crate::images::{FoundImages, RatchetArch};

/// Cloud machines, per-agent identities and secrets.
#[async_trait]
pub trait Compute: Send + Sync {
    async fn create_agent_identity(
        &self,
        optimization_id: &str,
        agent_id: &str,
    ) -> Result<AgentIdentity>;
    /// Kill switch: attach deny-all. Must take effect immediately.
    async fn revoke_agent_identity(&self, identity: &AgentIdentity) -> Result<()>;
    async fn delete_agent_identity(&self, identity: &AgentIdentity) -> Result<()>;
    async fn put_secret(
        &self,
        optimization_id: &str,
        agent_id: &str,
        name: &str,
        value: &str,
    ) -> Result<SecretRef>;
    async fn delete_secrets(&self, optimization_id: &str, agent_id: &str) -> Result<()>;
    async fn launch_runner(&self, spec: &RunnerLaunchSpec) -> Result<InstanceInfo>;
    async fn launch_bench_instance(&self, spec: &BenchLaunchSpec) -> Result<InstanceInfo>;
    async fn terminate(&self, instance_ids: &[String]) -> Result<()>;
    /// Every instance carrying the ratchet tags (optionally for one optimization).
    async fn list_tagged(&self, optimization_id: Option<&str>) -> Result<Vec<InstanceInfo>>;
    async fn run_command(
        &self,
        instance_id: &str,
        command: &RemoteCommand,
    ) -> Result<RemoteCommandResult>;
}

/// A flat key/value object store (S3 in live mode, a directory in simulation).
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn put(&self, key: &str, body: &[u8]) -> Result<()>;
    /// `None` when the key does not exist.
    async fn get(&self, key: &str) -> Result<Option<String>>;
    /// Sorted keys starting with `prefix`.
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;

    async fn put_str(&self, key: &str, body: &str) -> Result<()> {
        self.put(key, body.as_bytes()).await
    }
}

/// Billing truth for the cost meter's estimate (docs/V0_ARCHITECTURE.md §7 "Reconcile").
#[async_trait]
pub trait CostSource: Send + Sync {
    async fn actuals(&self, query: &CostActualsQuery) -> Result<CostActuals>;
}

/// On-demand USD per hour of an instance type in the sandbox region.
#[async_trait]
pub trait Pricing: Send + Sync {
    async fn hourly_usd(&self, instance_type: &str) -> Result<f64>;
}

/// Finds the machine images (AMIs) the app launches, by tag (`ratchet:image`, `ratchet:image_version`).
#[async_trait]
pub trait ImageLookup: Send + Sync {
    /// The newest image of each kind whose `ratchet:image_version` matches `app_version`. A missing image is not an
    /// error here (see `resolve_image_refs`).
    async fn find_images(&self, app_version: &str, arch: RatchetArch) -> Result<FoundImages>;
}

/// OpenTofu-driven sandbox lifecycle.
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    async fn plan(&self, input: &SandboxInput) -> Result<SandboxPlan>;
    async fn apply(&self, input: &SandboxInput) -> Result<SandboxOutputs>;
    async fn destroy(&self, input: &SandboxInput) -> Result<()>;
    async fn outputs(&self, sandbox_id: &str) -> Result<Option<SandboxOutputs>>;
}
