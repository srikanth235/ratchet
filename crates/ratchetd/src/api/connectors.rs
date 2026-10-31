//! Connectors: the accounts an optimization can use, several of each kind, stored once and picked per run; and the
//! choice remembered per repo.
//!
//! The wire types live in `ratchet-connectors` (`model`), one to one with `packages/desktop/src/shared/connectors.ts`, so the
//! store and the API cannot drift; they are re-exported here. Only [`ConnectorsVm`] is defined here, because it carries the
//! GitHub device flow of the settings group. Secrets (GitHub tokens, API keys) are written once and live in the keychain; they
//! are never part of any response.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub use ratchet_connectors::model::*;

use super::settings::GithubFlowVm;

/// Everything the Connectors screen shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorsVm {
    pub connectors: Vec<ConnectorVm>,
    /// The GitHub device flow in progress (or just ended), for the code to show.
    #[schema(required = true)]
    pub github_flow: Option<GithubFlowVm>,
    /// False when the keychain is unavailable: tokens and keys cannot be stored (the environment is still honoured).
    pub encryption_available: bool,
    /// True when a real OAuth client id is configured for "Sign in with GitHub".
    pub client_id_configured: bool,
}
