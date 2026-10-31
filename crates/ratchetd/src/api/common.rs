//! Shared wire types: the error body, status codes' vocabulary, the daemon's own status.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::vm::EngineInfo;

/// Prefix of every route.
pub const API_PREFIX: &str = "/v1";

/// The contract version. Bumped on a breaking change of the OpenAPI document.
pub const API_VERSION: &str = "v1";

/// One problem with one field of a request (a YAML path, a JSON field, a setting name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldError {
    pub path: String,
    pub message: String,
}

impl FieldError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Machine-readable error class. The HTTP status follows it: `unauthorized` 401, `not_found` 404,
/// `invalid_request` 400, `conflict` 409, `not_ready` 409, `unsupported` 501, `upstream` 502, `internal` 500.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Missing or wrong bearer token.
    Unauthorized,
    NotFound,
    /// The request is malformed or a field is invalid (see `fields`).
    InvalidRequest,
    /// The resource is in a state that does not allow this (end a sandbox while a run is active).
    Conflict,
    /// Something the user has to set up first (sign in with GitHub, connect AWS).
    NotReady,
    /// This engine does not implement the operation.
    Unsupported,
    /// GitHub, AWS or a model API failed.
    Upstream,
    Internal,
}

/// The body of every non-2xx response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: ErrorCode,
    /// One plain sentence for the user. Never contains a secret.
    pub message: String,
    /// Per-field problems, for `invalid_request`.
    #[serde(default)]
    pub fields: Vec<FieldError>,
}

/// `GET /v1/status`: who is answering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    /// Crate version of `ratchetd`.
    pub version: String,
    /// Contract version (`v1`).
    pub api_version: String,
    pub engine: EngineInfo,
    pub pid: u32,
    /// Epoch ms.
    pub started_at: i64,
    pub uptime_s: i64,
    /// The unix socket this daemon listens on.
    #[schema(required = true)]
    pub socket_path: Option<String>,
    /// The ledger file; `None` for an in-memory ledger.
    #[schema(required = true)]
    pub ledger_path: Option<String>,
    /// Id of the newest event on `/v1/events` (resume point for a fresh client).
    pub last_event_id: u64,
}
