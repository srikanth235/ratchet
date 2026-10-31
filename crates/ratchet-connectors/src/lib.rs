//! Ratchet connectors: the accounts an optimization can use (several GitHub accounts, AWS profiles and agent credentials, one
//! default per kind) and the choice remembered per repo. Port of the desktop main process's `connectors.ts`,
//! `connector-records.ts` and the secret half of `settings.ts` (see `PARITY.md`).
//!
//! - [`model`]: the wire types (what `ratchetd` publishes; `ToSchema` behind the `utoipa` feature). Always available.
//! - [`secrets`]: where tokens and keys live: the macOS Keychain, a 0600 file or memory.
//! - [`ConnectorStore`] (feature `store`, default): the saved connectors, their statuses, probes, migration of the old
//!   single-account settings, the per-repo selection, and [`ResolvedConnections`] for the engine.

pub mod model;
pub mod secrets;
#[cfg(feature = "store")]
mod store;
pub mod validate;

pub use model::*;
pub use validate::{SecretName, validate_aws_profile, validate_aws_region, validate_secret_format};

#[cfg(feature = "store")]
pub use store::{
    AwsProfile, AwsProfileList, AwsProfileLister, ConnectorDeps, ConnectorError, ConnectorProbe,
    ConnectorRecord, ConnectorStore, ConnectorsPersisted, DEFAULT_AWS_REGION, GITHUB_ENV_ID,
    GithubStored, HarnessSource, LegacyAccounts, LegacySource, RecordKind, ResolvedAgent,
    ResolvedAws, ResolvedConnections, ResolvedGithub, Result, TestOutcome, UpsertGithub,
};
