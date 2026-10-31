//! Error type of the crate. Messages mirror the TypeScript `Error` messages of `packages/aws` so logs and the
//! tests that match on them stay comparable.

use std::fmt;

use aws_smithy_types::error::display::DisplayErrorContext;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;

use crate::sandbox::infra_override::{ContractViolationError, InfraOverrideError};
use crate::sandbox::tofu::{TeardownBlockedError, TofuError};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A caller-supplied value failed validation (ids, secret names, proxy urls, ...). Never reaches AWS.
    #[error("{0}")]
    Invalid(String),
    /// An AWS API call failed. `code` is the service error code (`NoSuchEntity`, `AccessDenied`, ...), empty for
    /// transport-level failures; the TypeScript code reads it as `e.name`.
    #[error("{}", aws_display(code, message))]
    Aws { code: String, message: String },
    #[error(transparent)]
    Tofu(#[from] TofuError),
    #[error(transparent)]
    TeardownBlocked(#[from] TeardownBlockedError),
    #[error(transparent)]
    ContractViolation(#[from] ContractViolationError),
    #[error(transparent)]
    InfraOverride(#[from] InfraOverrideError),
    #[error(transparent)]
    ImageLookup(#[from] crate::images::ImageLookupError),
    /// Anything else the TypeScript code threw as a plain `Error`.
    #[error("{0}")]
    Other(String),
    /// A failure reported by a port implementation (the reaper and the cost meter call ports).
    #[error(transparent)]
    Port(#[from] ratchet_core::ports::PortError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Port implementations return core's [`PortError`](ratchet_core::ports::PortError): the message is kept verbatim, a
/// refused teardown keeps its own variant so callers can tell "nothing changed" from a failure.
impl From<Error> for ratchet_core::ports::PortError {
    fn from(e: Error) -> Self {
        use ratchet_core::ports::PortError;
        match e {
            Error::Port(p) => p,
            Error::TeardownBlocked(t) => PortError::TeardownBlocked(t.to_string()),
            other => PortError::Failed(other.to_string()),
        }
    }
}

fn aws_display(code: &str, message: &str) -> String {
    match (code.is_empty(), message.is_empty() || message == code) {
        (true, _) => message.to_string(),
        (false, true) => code.to_string(),
        (false, false) => format!("{code}: {message}"),
    }
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    pub fn aws(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Aws {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Converts an SDK error, keeping the service error code and message.
    pub fn from_sdk<E, R>(e: aws_sdk_ec2::error::SdkError<E, R>) -> Self
    where
        E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
        R: fmt::Debug + 'static,
    {
        let code = e.code().unwrap_or("").to_string();
        let message = match e.message() {
            Some(m) => m.to_string(),
            None => DisplayErrorContext(&e).to_string(),
        };
        Self::Aws { code, message }
    }

    /// The AWS error code (the TypeScript `errName`), empty when this is not an AWS error.
    pub fn code(&self) -> &str {
        match self {
            Self::Aws { code, .. } => code,
            _ => "",
        }
    }

    /// True when this is an AWS error whose code starts with `prefix` (the TS code tolerates both `EntityAlreadyExists`
    /// and the SDK class name `EntityAlreadyExistsException`).
    pub fn code_starts_with(&self, prefix: &str) -> bool {
        self.code().starts_with(prefix)
    }
}

/// Shorthand for `.map_err(Error::from_sdk)`.
pub(crate) fn sdk<E, R>(e: aws_sdk_ec2::error::SdkError<E, R>) -> Error
where
    E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
    R: fmt::Debug + 'static,
{
    Error::from_sdk(e)
}

#[cfg(test)]
mod tests {
    use ratchet_core::ports::PortError;

    use super::*;

    #[test]
    fn a_refused_teardown_keeps_its_variant_for_the_port() {
        let blocked = Error::TeardownBlocked(TeardownBlockedError {
            roles: vec!["r1".into()],
        });
        let message = blocked.to_string();
        assert_eq!(
            PortError::from(blocked),
            PortError::TeardownBlocked(message)
        );
    }

    #[test]
    fn every_other_error_reaches_the_port_with_its_message_verbatim() {
        let aws = Error::aws("AccessDenied", "nope");
        assert_eq!(
            PortError::from(aws),
            PortError::Failed("AccessDenied: nope".into())
        );
        let invalid = Error::invalid("invalid agent id \"x y\"");
        assert_eq!(
            PortError::from(invalid),
            PortError::Failed("invalid agent id \"x y\"".into())
        );
        // a port error that went through this crate comes out unchanged
        let cancelled = Error::from(PortError::Cancelled);
        assert_eq!(PortError::from(cancelled), PortError::Cancelled);
    }
}
