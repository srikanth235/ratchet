//! The capabilities of this crate that are not ports of `ratchet-core`.
//!
//! The ports themselves live in [`ratchet_core::ports`] and are implemented here, as the table shows (the Rust names
//! of the TypeScript ports of `packages/core/src/ports/index.ts`):
//!
//! | core trait | TypeScript | real | mock |
//! |---|---|---|---|
//! | [`CloudCompute`](ratchet_core::ports::CloudCompute) | `CloudCompute` | `AwsCloudCompute` | `MockCloudCompute` |
//! | [`ArtifactStore`](ratchet_core::ports::ArtifactStore) | `ArtifactStore` | `S3ArtifactStore` | `FsArtifactStore` |
//! | [`CostReconciler`](ratchet_core::ports::CostReconciler) | `CostReconciler` | `AwsCostReconciler` | `MockCostReconciler` |
//! | [`PriceBook`](ratchet_core::ports::PriceBook) | `PriceBook` | `AwsPriceBook` | `StaticPriceBook` |
//! | [`SandboxProvider`](ratchet_core::ports::SandboxProvider) | `SandboxProvider` | `TofuSandboxProvider` | `MockSandboxProvider` |
//!
//! [`ImageLookup`] (TS: the free function `findRatchetImages`) is the one capability core has no port for.

use async_trait::async_trait;

use crate::error::Result;
use crate::images::{FoundImages, RatchetArch};

/// Finds the machine images (AMIs) the app launches, by tag (`ratchet:image`, `ratchet:image_version`).
#[async_trait]
pub trait ImageLookup: Send + Sync {
    /// The newest image of each kind whose `ratchet:image_version` matches `app_version`. A missing image is not an
    /// error here (see `resolve_image_refs`).
    async fn find_images(&self, app_version: &str, arch: RatchetArch) -> Result<FoundImages>;
}
