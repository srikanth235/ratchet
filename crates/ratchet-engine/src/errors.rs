//! `EngineError` and id validation. Port of `packages/engine/src/errors.ts`.

use ratchet_core::LedgerError;
use ratchet_core::ports::PortError;
use ratchet_core::sandbox::SandboxLifecycleError;
use serde_json::Value;

/// The TypeScript `EngineErrorCode`, plus the failures that pass through from the ledger and the ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineErrorCode {
    InvalidId,
    InvalidInput,
    InvalidConfig,
    InvalidState,
    NotFound,
    MissingSecret,
    NoVerified,
    Orphans,
    TeardownIncomplete,
    Unavailable,
    SandboxUnavailable,
    Aborted,
    SandboxNotApplied,
    SandboxInUse,
    NotConfigured,
    /// The ledger refused (message is the ledger's).
    Ledger,
    /// A port (cloud, git host, runners, bench) failed.
    Port,
    /// Anything else (a bug or an unexpected state).
    Internal,
}

impl EngineErrorCode {
    /// The TypeScript code string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidId => "invalid_id",
            Self::InvalidInput => "invalid_input",
            Self::InvalidConfig => "invalid_config",
            Self::InvalidState => "invalid_state",
            Self::NotFound => "not_found",
            Self::MissingSecret => "missing_secret",
            Self::NoVerified => "no_verified",
            Self::Orphans => "orphans",
            Self::TeardownIncomplete => "teardown_incomplete",
            Self::Unavailable => "unavailable",
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::Aborted => "aborted",
            Self::SandboxNotApplied => "sandbox_not_applied",
            Self::SandboxInUse => "sandbox_in_use",
            Self::NotConfigured => "not_configured",
            Self::Ledger => "ledger",
            Self::Port => "port",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct EngineError {
    pub code: EngineErrorCode,
    pub message: String,
    /// Structured detail for a few codes (`orphans`, `teardown_incomplete`: the teardown report; `invalid_state` of a
    /// template proposal: the existing paths; `invalid_config`: the config errors).
    pub details: Option<Value>,
}

pub type EngineResult<T> = Result<T, EngineError>;

impl EngineError {
    pub fn new(code: EngineErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(EngineErrorCode::InvalidInput, message)
    }

    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::new(EngineErrorCode::InvalidState, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(EngineErrorCode::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(EngineErrorCode::Internal, message)
    }

    pub fn is(&self, code: EngineErrorCode) -> bool {
        self.code == code
    }
}

impl From<LedgerError> for EngineError {
    fn from(e: LedgerError) -> Self {
        Self::new(EngineErrorCode::Ledger, e.message)
    }
}

impl From<ratchet_env::EnvError> for EngineError {
    fn from(e: ratchet_env::EnvError) -> Self {
        use ratchet_env::EnvErrorKind as K;
        let code = match e.kind {
            K::InvalidInput => EngineErrorCode::InvalidInput,
            K::InvalidConfig => EngineErrorCode::InvalidConfig,
            K::InvalidState => EngineErrorCode::InvalidState,
            K::NotFound => EngineErrorCode::NotFound,
            K::Unavailable => EngineErrorCode::Unavailable,
            K::Aborted => EngineErrorCode::Aborted,
            K::Port => EngineErrorCode::Port,
        };
        Self::new(code, e.message)
    }
}

impl From<PortError> for EngineError {
    fn from(e: PortError) -> Self {
        // the live runtime's unready ports fail with `not_configured: <what is missing>`
        if let PortError::Failed(m) = &e
            && let Some(rest) = m.strip_prefix("not_configured: ")
        {
            return Self::new(EngineErrorCode::NotConfigured, rest);
        }
        Self::new(EngineErrorCode::Port, e.to_string())
    }
}

impl From<SandboxLifecycleError> for EngineError {
    fn from(e: SandboxLifecycleError) -> Self {
        match e {
            SandboxLifecycleError::InvalidState(m) => Self::new(EngineErrorCode::InvalidState, m),
            SandboxLifecycleError::NotFound(m) => Self::new(EngineErrorCode::NotFound, m),
            SandboxLifecycleError::SandboxInUse {
                message,
                optimization_ids,
            } => Self::new(EngineErrorCode::SandboxInUse, message)
                .with_details(serde_json::json!({ "optimizationIds": optimization_ids })),
            SandboxLifecycleError::Orphans { message, blockers } => {
                Self::new(EngineErrorCode::Orphans, message)
                    .with_details(serde_json::json!({ "blockers": blockers }))
            }
            SandboxLifecycleError::Port(p) => p.into(),
            SandboxLifecycleError::Ledger(l) => l.into(),
        }
    }
}

impl From<EngineError> for PortError {
    fn from(e: EngineError) -> Self {
        match e.code {
            EngineErrorCode::Aborted => PortError::Cancelled,
            _ => PortError::Failed(e.message),
        }
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(e: serde_json::Error) -> Self {
        Self::internal(e.to_string())
    }
}

const MAX_ID_LEN: usize = 48;

/// Ids end up in branch names, IAM role names and SSM paths: lowercase letters, digits and single dashes only
/// (`^[a-z0-9]+(?:-[a-z0-9]+)*$`, at most 48 characters).
pub fn assert_valid_id(what: &str, id: &str) -> EngineResult<()> {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
    if ok {
        Ok(())
    } else {
        Err(EngineError::new(
            EngineErrorCode::InvalidId,
            format!(
                "invalid {what} {}: use 1-{MAX_ID_LEN} characters from [a-z0-9-], no leading, trailing or doubled dashes",
                serde_json::to_string(id).unwrap_or_default()
            ),
        ))
    }
}
