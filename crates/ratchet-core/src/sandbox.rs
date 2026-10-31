//! Sandbox lifecycle: drives a sandbox through `planned -> applying -> applied -> destroying -> destroyed /
//! failed`, recording each transition (and the contract outputs on apply) in the ledger. Shared by the engine
//! and the simulation so both enforce the same rules:
//!
//! - apply is allowed from absent / planned / destroyed / failed / applied (a re-apply converges);
//! - destroy is refused while an optimization in the sandbox is baselining, running or stopping, and while the
//!   `blockers` hook reports orphans; a refusal changes nothing;
//! - a provider failure leaves the sandbox `failed` (its real state is unknown) and is returned;
//! - [`SandboxLifecycle::recover`] marks an apply / destroy that a crash interrupted as `failed`.
//!
//! Port of `packages/core/src/sandbox/lifecycle.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;

use crate::enums::{SandboxProviderKind, SandboxStatus};
use crate::ledger::{LedgerError, Patch, RecordSandboxInput, SandboxLedger, SandboxStatusPatch};
use crate::ports::{
    PortError, SandboxInput, SandboxOptimizationRef, SandboxOutputs, SandboxPlan, SandboxProvider,
    SandboxState, SandboxStateStatus,
};
use crate::rows::{Optimization, Sandbox};

/// Why a lifecycle operation was refused or failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SandboxLifecycleError {
    #[error("{0}")]
    InvalidState(String),
    #[error("{0}")]
    NotFound(String),
    /// An optimization in the sandbox is still running. Carries its ids.
    #[error("{message}")]
    SandboxInUse {
        message: String,
        optimization_ids: Vec<String>,
    },
    /// Tagged instances or agent identities are still alive. Carries the reasons.
    #[error("{message}")]
    Orphans {
        message: String,
        blockers: Vec<String>,
    },
    /// The provider (or the blockers hook) failed.
    #[error(transparent)]
    Port(#[from] PortError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
}

/// The TypeScript `SandboxLifecycleErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLifecycleErrorCode {
    InvalidState,
    NotFound,
    SandboxInUse,
    Orphans,
}

impl SandboxLifecycleError {
    /// The code of a lifecycle refusal; `None` for provider and ledger failures.
    pub fn code(&self) -> Option<SandboxLifecycleErrorCode> {
        match self {
            Self::InvalidState(_) => Some(SandboxLifecycleErrorCode::InvalidState),
            Self::NotFound(_) => Some(SandboxLifecycleErrorCode::NotFound),
            Self::SandboxInUse { .. } => Some(SandboxLifecycleErrorCode::SandboxInUse),
            Self::Orphans { .. } => Some(SandboxLifecycleErrorCode::Orphans),
            Self::Port(_) | Self::Ledger(_) => None,
        }
    }
}

/// Remembers the input a sandbox was applied with, so destroy runs with the same variables.
pub trait SandboxInputStore: Send + Sync {
    fn get(&self, sandbox_id: &str) -> Option<SandboxInput>;
    fn set(&self, sandbox_id: &str, input: SandboxInput);
}

/// A [`SandboxInputStore`] in memory.
#[derive(Default)]
pub struct MemorySandboxInputStore {
    inputs: Mutex<HashMap<String, SandboxInput>>,
}

impl SandboxInputStore for MemorySandboxInputStore {
    fn get(&self, sandbox_id: &str) -> Option<SandboxInput> {
        lock(&self.inputs).get(sandbox_id).cloned()
    }

    fn set(&self, sandbox_id: &str, input: SandboxInput) {
        lock(&self.inputs).insert(sandbox_id.to_owned(), input);
    }
}

/// Teardown guard beyond running optimizations: the reasons the sandbox must not be destroyed yet (tagged
/// instances still alive, agent identities not deleted). Fail closed: an error blocks the destroy.
#[async_trait]
pub trait SandboxBlockers: Send + Sync {
    async fn blockers(
        &self,
        sandbox_id: &str,
        optimizations: &[Optimization],
    ) -> Result<Vec<String>, PortError>;
}

pub struct SandboxLifecycleOptions {
    pub ledger: Arc<dyn SandboxLedger>,
    pub provider: Arc<dyn SandboxProvider>,
    /// Ledger `sandboxes.provider`: `aws` for the tofu provider, `mock` for simulation.
    pub kind: SandboxProviderKind,
    pub blockers: Option<Arc<dyn SandboxBlockers>>,
    pub inputs: Option<Arc<dyn SandboxInputStore>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `^arn:aws[^:]*:[^:]*:[^:]*:(\d{12}):`
fn account_in_arn(arn: &str) -> Option<&str> {
    let parts: Vec<&str> = arn.split(':').collect();
    let account = parts.get(4)?;
    (parts.len() >= 6
        && parts[0] == "arn"
        && parts[1].starts_with("aws")
        && account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit()))
    .then_some(*account)
}

fn account_id_of(outputs: &SandboxOutputs) -> Option<String> {
    account_in_arn(&outputs.eval_identity_ref)
        .or_else(|| account_in_arn(&outputs.agent_boundary_ref))
        .map(str::to_owned)
}

fn fallback_input(row: &Sandbox) -> SandboxInput {
    SandboxInput {
        sandbox_id: row.id.to_string(),
        region: row.region.clone(),
        allowed_instance_types: vec!["c7i.2xlarge".into()],
        max_instances_per_agent: 2,
        max_instance_lifetime_hours: 4.0,
        tags: None,
    }
}

/// Marks a sandbox busy until dropped.
struct Claim<'a> {
    busy: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        lock(self.busy).remove(&self.id);
    }
}

pub struct SandboxLifecycle {
    opts: SandboxLifecycleOptions,
    busy: Mutex<HashSet<String>>,
    errors: Mutex<HashMap<String, String>>,
}

impl SandboxLifecycle {
    pub fn new(opts: SandboxLifecycleOptions) -> Self {
        Self {
            opts,
            busy: Mutex::new(HashSet::new()),
            errors: Mutex::new(HashMap::new()),
        }
    }

    pub async fn plan(&self, input: &SandboxInput) -> Result<SandboxPlan, SandboxLifecycleError> {
        let plan = self.opts.provider.plan(input).await?;
        self.opts
            .ledger
            .record_sandbox(RecordSandboxInput::new(
                input.sandbox_id.as_str(),
                self.opts.kind,
                input.region.as_str(),
            ))
            .await?;
        Ok(plan)
    }

    pub async fn apply(
        &self,
        input: &SandboxInput,
    ) -> Result<SandboxOutputs, SandboxLifecycleError> {
        let ledger = &self.opts.ledger;
        let id = input.sandbox_id.as_str();
        let _claim = self.claim(id, "apply")?;
        ledger
            .record_sandbox(RecordSandboxInput::new(
                id,
                self.opts.kind,
                input.region.as_str(),
            ))
            .await?;
        ledger
            .set_sandbox_status(id, SandboxStatus::Applying, SandboxStatusPatch::default())
            .await?;
        lock(&self.errors).remove(id);
        match self.opts.provider.apply(input).await {
            Ok(outputs) => {
                if let Some(store) = &self.opts.inputs {
                    store.set(id, input.clone());
                }
                let json = serde_json::to_value(&outputs)
                    .map_err(|e| LedgerError::invalid(e.to_string()))?;
                ledger
                    .set_sandbox_status(
                        id,
                        SandboxStatus::Applied,
                        SandboxStatusPatch {
                            outputs: Patch::Set(json),
                            account_id: account_id_of(&outputs).map_or(Patch::Clear, Patch::Set),
                            at: None,
                        },
                    )
                    .await?;
                Ok(outputs)
            }
            Err(err) => {
                lock(&self.errors).insert(id.to_owned(), err.to_string());
                ledger
                    .set_sandbox_status(id, SandboxStatus::Failed, SandboxStatusPatch::default())
                    .await?;
                Err(err.into())
            }
        }
    }

    pub async fn destroy(&self, sandbox_id: &str) -> Result<(), SandboxLifecycleError> {
        let ledger = &self.opts.ledger;
        let Some(row) = ledger.get_sandbox(sandbox_id).await? else {
            return Err(SandboxLifecycleError::NotFound(format!(
                "sandbox {sandbox_id} not found"
            )));
        };
        match row.status {
            SandboxStatus::Destroyed => return Ok(()),
            SandboxStatus::Planned => {
                return Err(SandboxLifecycleError::InvalidState(format!(
                    "sandbox {sandbox_id} was never applied; there is nothing to destroy"
                )));
            }
            SandboxStatus::Applying | SandboxStatus::Destroying => {
                return Err(SandboxLifecycleError::InvalidState(format!(
                    "sandbox {sandbox_id} is {}",
                    row.status
                )));
            }
            SandboxStatus::Applied | SandboxStatus::Failed => {}
        }
        let _claim = self.claim(sandbox_id, "destroy")?;
        let optimizations = ledger.optimizations_in_sandbox(sandbox_id).await?;
        let active: Vec<&Optimization> = optimizations
            .iter()
            .filter(|o| o.status.is_active())
            .collect();
        if !active.is_empty() {
            let ids: Vec<String> = active.iter().map(|o| o.id.to_string()).collect();
            return Err(SandboxLifecycleError::SandboxInUse {
                message: format!(
                    "cannot destroy sandbox {sandbox_id}: {} optimization(s) still running ({}). Stop them first.",
                    active.len(),
                    ids.join(", ")
                ),
                optimization_ids: ids,
            });
        }
        if let Some(hook) = &self.opts.blockers {
            let blockers = hook.blockers(sandbox_id, &optimizations).await?;
            if !blockers.is_empty() {
                return Err(SandboxLifecycleError::Orphans {
                    message: format!(
                        "cannot destroy sandbox {sandbox_id}: {}",
                        blockers.join("; ")
                    ),
                    blockers,
                });
            }
        }
        let input = self
            .opts
            .inputs
            .as_ref()
            .and_then(|s| s.get(sandbox_id))
            .unwrap_or_else(|| fallback_input(&row));
        let before = row.status;
        ledger
            .set_sandbox_status(
                sandbox_id,
                SandboxStatus::Destroying,
                SandboxStatusPatch::default(),
            )
            .await?;
        lock(&self.errors).remove(sandbox_id);
        match self.opts.provider.destroy(&input).await {
            Ok(()) => {}
            Err(PortError::TeardownBlocked(message)) => {
                // The provider refused before running `tofu destroy`: nothing changed, so the sandbox is as it was.
                ledger
                    .set_sandbox_status(sandbox_id, before, SandboxStatusPatch::default())
                    .await?;
                return Err(SandboxLifecycleError::Orphans {
                    message,
                    blockers: Vec::new(),
                });
            }
            Err(err) => {
                lock(&self.errors).insert(sandbox_id.to_owned(), err.to_string());
                ledger
                    .set_sandbox_status(
                        sandbox_id,
                        SandboxStatus::Failed,
                        SandboxStatusPatch::default(),
                    )
                    .await?;
                return Err(err.into());
            }
        }
        ledger
            .set_sandbox_status(
                sandbox_id,
                SandboxStatus::Destroyed,
                SandboxStatusPatch {
                    outputs: Patch::Clear,
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }

    /// What the ledger knows about a sandbox, plus the optimizations linked to it.
    pub async fn state(&self, sandbox_id: &str) -> Result<SandboxState, SandboxLifecycleError> {
        let row = self.opts.ledger.get_sandbox(sandbox_id).await?;
        let optimizations: Vec<SandboxOptimizationRef> = self
            .opts
            .ledger
            .optimizations_in_sandbox(sandbox_id)
            .await?
            .into_iter()
            .map(|o| SandboxOptimizationRef {
                id: o.id,
                status: o.status,
            })
            .collect();
        let Some(row) = row else {
            return Ok(SandboxState {
                sandbox_id: sandbox_id.to_owned(),
                status: SandboxStateStatus::Absent,
                provider: None,
                region: None,
                account_id: None,
                outputs: None,
                applied_at: None,
                destroyed_at: None,
                last_error: None,
                optimizations,
            });
        };
        Ok(SandboxState {
            sandbox_id: sandbox_id.to_owned(),
            status: row.status.into(),
            provider: Some(row.provider),
            region: Some(row.region),
            account_id: row.account_id,
            outputs: row.outputs.and_then(|v| serde_json::from_value(v).ok()),
            applied_at: row.applied_at,
            destroyed_at: row.destroyed_at,
            last_error: lock(&self.errors).get(sandbox_id).cloned(),
            optimizations,
        })
    }

    /// An apply / destroy cut short by a crash leaves unknown infrastructure behind: mark it failed so it can be
    /// retried. Returns the ids it fixed.
    pub async fn recover(&self) -> Result<Vec<String>, SandboxLifecycleError> {
        let mut fixed = Vec::new();
        for row in self.opts.ledger.list_sandboxes().await? {
            if matches!(
                row.status,
                SandboxStatus::Applying | SandboxStatus::Destroying
            ) {
                self.opts
                    .ledger
                    .set_sandbox_status(
                        row.id.as_str(),
                        SandboxStatus::Failed,
                        SandboxStatusPatch::default(),
                    )
                    .await?;
                let what = if row.status == SandboxStatus::Applying {
                    "apply"
                } else {
                    "destroy"
                };
                lock(&self.errors).insert(
                    row.id.to_string(),
                    format!("{what} was interrupted; run it again to converge"),
                );
                fixed.push(row.id.to_string());
            }
        }
        Ok(fixed)
    }

    fn claim(&self, sandbox_id: &str, what: &str) -> Result<Claim<'_>, SandboxLifecycleError> {
        let mut busy = lock(&self.busy);
        if !busy.insert(sandbox_id.to_owned()) {
            return Err(SandboxLifecycleError::InvalidState(format!(
                "sandbox {sandbox_id} is busy (another {what} is running)"
            )));
        }
        Ok(Claim {
            busy: &self.busy,
            id: sandbox_id.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_account_from_an_arn() {
        assert_eq!(
            account_in_arn("arn:aws:iam::123456789012:policy/b"),
            Some("123456789012")
        );
        assert_eq!(
            account_in_arn("arn:aws-cn:iam::123456789012:role/x"),
            Some("123456789012")
        );
        assert_eq!(account_in_arn("arn:aws:s3:::bucket"), None);
        assert_eq!(account_in_arn("arn:aws:iam::12345:policy/b"), None);
        assert_eq!(account_in_arn("arn:aws:iam::123456789012"), None); // no colon after the account
        assert_eq!(account_in_arn("nope"), None);
    }

    #[test]
    fn error_codes_map_to_the_typescript_ones() {
        let e = SandboxLifecycleError::Orphans {
            message: "m".into(),
            blockers: vec![],
        };
        assert_eq!(e.code(), Some(SandboxLifecycleErrorCode::Orphans));
        assert_eq!(
            SandboxLifecycleError::Port(PortError::failed("x")).code(),
            None
        );
    }
}
