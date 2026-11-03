use std::fmt;

/// Everything this crate can fail with. Messages are user-facing and match the TypeScript wording, so tests (and the
/// app) can match on them. No variant ever carries a token: callers scrub secrets before building the message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A `git` invocation failed (the message is already scrubbed of secrets).
    #[error("{0}")]
    Git(String),
    /// A bad argument or an operation refused before anything was touched.
    #[error("{0}")]
    Invalid(String),
    /// The remote said no; `status` is the HTTP status when there was one.
    #[error("{message}")]
    Http {
        status: Option<u16>,
        message: String,
    },
    /// A port call failed in a collaborator (the clock, say).
    #[error(transparent)]
    Port(#[from] ratchet_core::ports::PortError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The message is kept verbatim: the engine shows it to the user.
impl From<Error> for ratchet_core::ports::PortError {
    fn from(e: Error) -> Self {
        match e {
            Error::Port(p) => p,
            Error::Http {
                status: Some(status),
                message,
            } => ratchet_core::ports::PortError::Http { status, message },
            other => ratchet_core::ports::PortError::Failed(other.to_string()),
        }
    }
}

impl Error {
    pub fn invalid(msg: impl fmt::Display) -> Self {
        Error::Invalid(msg.to_string())
    }

    pub fn http(status: Option<u16>, msg: impl fmt::Display) -> Self {
        Error::Http {
            status,
            message: msg.to_string(),
        }
    }

    /// The HTTP status of a remote failure, if any (the TS code read `err.status`).
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Http { status, .. } => *status,
            _ => None,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Replace every occurrence of `secret` in `text` (ignored when shorter than 4 chars, like the TS `scrub`).
pub fn scrub(text: &str, secret: &str) -> String {
    if secret.len() >= 4 {
        text.replace(secret, "***")
    } else {
        text.to_string()
    }
}
