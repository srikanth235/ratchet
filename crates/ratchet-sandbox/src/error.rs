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
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
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
