//! Errors of the environment flow. The kinds mirror the codes of the engine's `EngineError` so a service can map
//! them one to one (`invalid_input` -> 400, `not_found` -> 404, `invalid_state` -> 409, `unavailable` -> 501 ...).

use ratchet_core::ports::PortError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvErrorKind {
    InvalidInput,
    InvalidConfig,
    InvalidState,
    NotFound,
    Unavailable,
    Aborted,
    /// A port (git, agent) failed.
    Port,
}

impl EnvErrorKind {
    /// The `EngineError` code the TypeScript uses for it.
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::InvalidConfig => "invalid_config",
            Self::InvalidState => "invalid_state",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::Aborted => "aborted",
            Self::Port => "port",
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct EnvError {
    pub kind: EnvErrorKind,
    pub message: String,
}

pub type EnvResult<T> = Result<T, EnvError>;

impl EnvError {
    pub fn new(kind: EnvErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn invalid_input(m: impl Into<String>) -> Self {
        Self::new(EnvErrorKind::InvalidInput, m)
    }
    pub fn invalid_config(m: impl Into<String>) -> Self {
        Self::new(EnvErrorKind::InvalidConfig, m)
    }
    pub fn invalid_state(m: impl Into<String>) -> Self {
        Self::new(EnvErrorKind::InvalidState, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(EnvErrorKind::NotFound, m)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new(EnvErrorKind::Unavailable, m)
    }
    pub fn aborted(m: impl Into<String>) -> Self {
        Self::new(EnvErrorKind::Aborted, m)
    }
}

impl From<PortError> for EnvError {
    fn from(e: PortError) -> Self {
        match e {
            PortError::Unsupported(what) => {
                Self::unavailable(format!("{what} is not supported by this git host"))
            }
            PortError::Cancelled => Self::aborted("cancelled"),
            other => Self::new(EnvErrorKind::Port, other.to_string()),
        }
    }
}

impl From<std::io::Error> for EnvError {
    fn from(e: std::io::Error) -> Self {
        Self::new(EnvErrorKind::Port, e.to_string())
    }
}
