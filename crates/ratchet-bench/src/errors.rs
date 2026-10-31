//! Errors of the bench crate (port of `errors.ts`).

use ratchet_core::ports::PortError;

/// Why a bench job did not produce an outcome.
///
/// `Infra` is an infrastructure failure (checkout, launch, misconfiguration), as opposed to a candidate failing a
/// gate (that is a failed guardrail inside a normal `BenchOutcome`). `Aborted` and `Cancelled` are the TypeScript
/// `BenchAbortedError` / `BenchCancelledError` (both had `name === "AbortError"`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BenchError {
    /// `BenchError` in TypeScript.
    #[error("{0}")]
    Infra(String),
    /// The job was aborted through its cancellation token.
    #[error("bench job aborted")]
    Aborted,
    /// A queued or running job was cancelled through the `BenchQueue`.
    #[error("bench job cancelled")]
    Cancelled,
}

impl BenchError {
    pub fn infra(msg: impl Into<String>) -> Self {
        BenchError::Infra(msg.into())
    }

    /// True for `Aborted` and `Cancelled` (generic abort handling, the `AbortError` name in TypeScript).
    pub fn is_abort(&self) -> bool {
        matches!(self, BenchError::Aborted | BenchError::Cancelled)
    }
}

impl From<BenchError> for PortError {
    fn from(e: BenchError) -> Self {
        match e {
            BenchError::Aborted | BenchError::Cancelled => PortError::Cancelled,
            BenchError::Infra(m) => PortError::Failed(m),
        }
    }
}

impl From<PortError> for BenchError {
    fn from(e: PortError) -> Self {
        match e {
            PortError::Cancelled => BenchError::Aborted,
            other => BenchError::Infra(other.to_string()),
        }
    }
}

pub type BenchResult<T> = Result<T, BenchError>;
