//! Engine types -> API view models (`toReceiptVM`, `toTemplateVM`, error mapping). Port of the matching parts of
//! `packages/desktop/src/main/mappers.ts` that `views::LedgerViews` does not cover.

use ratchet_core::enums::SandboxStatus;
use ratchet_core::ports::{SandboxPhase, SandboxTeardownReceipt, TeardownTrigger};
use ratchet_engine::{EngineError, EngineErrorCode};

use crate::api::*;
use crate::service::ServiceError;

fn category_label(id: &str) -> String {
    match id {
        "agent_runner" => "Agent runners",
        "experiment" => "Experiment instances",
        "bench_runner" => "Bench runner",
        "storage_egress" => "Storage & egress",
        "tokens" => "Model tokens",
        other => other,
    }
    .to_owned()
}

/// The receipt shown on Result and Environments (counts instead of ids; ids stay in the ledger).
pub fn receipt_vm(r: &SandboxTeardownReceipt) -> TeardownReceiptVm {
    TeardownReceiptVm {
        sandbox_id: r.sandbox_id.clone(),
        region: r.region.clone(),
        trigger: match r.trigger {
            TeardownTrigger::Idle => ReceiptTrigger::Idle,
            TeardownTrigger::Manual => ReceiptTrigger::Manual,
        },
        destroyed_at: r.destroyed_at,
        spend_usd: r.spend_usd,
        spend_by_category: r
            .spend_by_category
            .iter()
            .filter(|(_, usd)| **usd > 0.0)
            .map(|(k, usd)| LabeledUsd {
                label: category_label(k),
                usd: *usd,
            })
            .collect(),
        runs: r
            .runs
            .iter()
            .map(|x| ReceiptRun {
                optimization_id: x.optimization_id.to_string(),
                status: x.status.clone(),
                spend_usd: x.spend_usd,
            })
            .collect(),
        removed: ReceiptRemoved {
            instances: r.removed.instances.len() as i64,
            agent_identities: r.removed.agent_identities.len() as i64,
            secrets: r.removed.secrets.len() as i64,
            resources: r.removed.resources.clone(),
        },
        nothing_running: r.nothing_running,
        orphans: r
            .orphans
            .iter()
            .map(|o| format!("{} {} ({})", o.role, o.id, o.state))
            .collect(),
        errors: r.errors.clone(),
        summary: r.summary.clone(),
    }
}

pub fn phase_vm(p: SandboxPhase) -> EnvironmentPhase {
    match p {
        SandboxPhase::Creating => EnvironmentPhase::Creating,
        SandboxPhase::Ready => EnvironmentPhase::Ready,
        SandboxPhase::Idle => EnvironmentPhase::Idle,
        SandboxPhase::Destroying => EnvironmentPhase::Destroying,
        SandboxPhase::Destroyed => EnvironmentPhase::Destroyed,
        SandboxPhase::Failed => EnvironmentPhase::Failed,
    }
}

/// The phase derived from the ledger row (what the TS service shows when no event was seen).
pub fn derived_phase(status: SandboxStatus, has_active_runs: bool) -> EnvironmentPhase {
    match status {
        SandboxStatus::Applying => EnvironmentPhase::Creating,
        SandboxStatus::Destroying => EnvironmentPhase::Destroying,
        SandboxStatus::Failed => EnvironmentPhase::Failed,
        _ if has_active_runs => EnvironmentPhase::Ready,
        _ => EnvironmentPhase::Idle,
    }
}

/// The first line of an error, which is all a toast needs.
pub fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("failed")
}

pub fn service_error(e: EngineError) -> ServiceError {
    use EngineErrorCode as C;
    let m = e.message;
    match e.code {
        C::NotFound => ServiceError::NotFound(m),
        C::InvalidId | C::InvalidInput | C::InvalidConfig => ServiceError::Invalid(m),
        C::InvalidState
        | C::NoVerified
        | C::Orphans
        | C::TeardownIncomplete
        | C::SandboxInUse
        | C::Aborted => ServiceError::Conflict(m),
        C::MissingSecret | C::NotConfigured | C::SandboxNotApplied | C::SandboxUnavailable => {
            ServiceError::NotReady(m)
        }
        C::Unavailable => ServiceError::Unsupported(m),
        C::Port => ServiceError::Upstream(m),
        C::Ledger | C::Internal => ServiceError::Internal(m),
    }
}

pub use crate::connectors_hub::github_error;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ratchet_core::ids::OptimizationId;
    use ratchet_core::ports::{TeardownOrphan, TeardownRemoved, TeardownRun};

    use super::*;

    /// service.test.ts › environments › the receipt carries counts, labelled spend and orphan lines
    #[test]
    fn the_receipt_carries_counts_labelled_spend_and_orphan_lines() {
        let r = SandboxTeardownReceipt {
            sandbox_id: "sbx".into(),
            region: "us-east-1".into(),
            trigger: TeardownTrigger::Manual,
            created_at: None,
            destroyed_at: 5,
            runs: vec![TeardownRun {
                optimization_id: OptimizationId::new("opt-1"),
                status: "completed".into(),
                spend_usd: 2.0,
            }],
            spend_usd: 2.0,
            spend_by_category: BTreeMap::from([
                ("experiment".to_owned(), 1.5),
                ("tokens".to_owned(), 0.5),
                ("agent_runner".to_owned(), 0.0),
            ]),
            removed: TeardownRemoved {
                instances: vec!["i-1".into(), "i-2".into()],
                agent_identities: vec!["a".into()],
                secrets: vec![],
                resources: vec!["vpc".into()],
            },
            nothing_running: false,
            orphans: vec![TeardownOrphan {
                id: "i-9".into(),
                role: "experiment".into(),
                state: "running".into(),
            }],
            errors: vec!["x".into()],
            summary: "s".into(),
        };
        let v = receipt_vm(&r);
        assert_eq!(v.trigger, ReceiptTrigger::Manual);
        assert_eq!(v.removed.instances, 2);
        assert_eq!(v.removed.agent_identities, 1);
        assert_eq!(v.removed.secrets, 0);
        let labels: Vec<_> = v
            .spend_by_category
            .iter()
            .map(|l| l.label.as_str())
            .collect();
        assert_eq!(labels, ["Experiment instances", "Model tokens"]);
        assert_eq!(v.orphans, ["experiment i-9 (running)"]);
    }

    #[test]
    fn engine_errors_map_to_service_errors() {
        let e = |c, m: &str| service_error(EngineError::new(c, m));
        assert!(matches!(
            e(EngineErrorCode::NotFound, "x"),
            ServiceError::NotFound(_)
        ));
        assert!(matches!(
            e(EngineErrorCode::NotConfigured, "x"),
            ServiceError::NotReady(_)
        ));
        assert!(matches!(
            e(EngineErrorCode::NoVerified, "x"),
            ServiceError::Conflict(_)
        ));
        assert!(matches!(
            e(EngineErrorCode::Unavailable, "x"),
            ServiceError::Unsupported(_)
        ));
        assert!(matches!(
            e(EngineErrorCode::Port, "x"),
            ServiceError::Upstream(_)
        ));
    }
}
