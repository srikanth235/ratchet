//! `ratchet-sandbox`: everything Ratchet needs from the cloud sandbox, ported from `packages/aws`.
//!
//! - [`compute`]: EC2/SSM/IAM runner control (`AwsCloudCompute`) with the security-critical user data, and the mock.
//! - [`artifacts`]: S3 and atomic-write directory artifact stores.
//! - [`cost`], [`cost_explorer`], [`pricing`]: live spend estimate, billing truth, instance prices.
//! - [`images`], [`region`], [`identity`]: AMI lookup by tag, per-optimization region, "who am I".
//! - [`reaper`]: lifetime and quota enforcement.
//! - [`sandbox`]: `TofuSandboxProvider` (the `tofu` binary as a subprocess), the `infra:` override, the mock.
//!
//! Every external capability is a [`ratchet_core::ports`] trait with a real and a mock implementation here (see
//! [`ports`]), so simulation mode runs the whole product without AWS. Real AWS is never called by the tests. The data
//! types (`SandboxOutputs`, `InstanceInfo`, ...) are `ratchet-core`'s.

pub mod artifacts;
mod civil;
pub mod compute;
pub mod cost;
pub mod cost_explorer;
pub mod error;
pub mod identity;
pub mod images;
pub mod ports;
pub mod pricing;
pub mod reaper;
pub mod region;
pub mod sandbox;
pub mod util;

pub use artifacts::{FsArtifactStore, S3ArtifactStore};
pub use compute::{
    AwsClients, AwsCloudCompute, AwsCloudComputeOptions, AwsComputeConfig, MockCloudCompute,
    MockCloudComputeOptions,
};
pub use cost::{BudgetGuard, BudgetStatus, CostMeter, CostMeterOptions, CostSnapshot, SpendDelta};
pub use cost_explorer::{AwsCostReconciler, MockCostReconciler, billed, category_for_tags};
pub use error::{Error, Result};
pub use identity::{CallerIdentity, CheckAwsCredentialsOptions, check_aws_credentials};
pub use images::{
    Ec2ImageLookup, FoundImages, ImageLookupError, MockImageLookup, ResolveImageArgs,
    ResolvedImages, resolve_image_refs,
};
pub use ports::ImageLookup;
pub use pricing::{AwsPriceBook, StaticPriceBook};
pub use reaper::{Reaper, ReaperOptions, ReaperPolicy, ReaperReport};
pub use region::{DEFAULT_REGION, environment_region, region_for_optimization};
pub use sandbox::{
    AwsSandboxOutputs, MockSandboxProvider, TofuSandboxProvider, TofuSandboxProviderOptions,
};
