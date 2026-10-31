//! The invisible sandbox of the simulation: created at the first run, reused while runs are active, idle after the
//! last one ends, removed on "End sandbox now" with a receipt.

use std::collections::HashMap;

use ratchet_core::enums::{OptimizationStatus, SandboxProviderKind, SandboxStatus, SpendCategory};
use ratchet_core::ids::{OptimizationId, SandboxId};
use ratchet_core::ledger::{Patch, RecordSandboxInput, SandboxStatusPatch};
use ratchet_core::ports::SandboxPhase;
use serde_json::json;

use super::Inner;
use crate::api::*;
use crate::service::{ServiceError, ServiceResult};

pub const SIM_SANDBOX: &str = "sbx-sim-1";
const REGION: &str = "us-east-1";

#[derive(Debug, Default)]
pub struct EnvState {
    /// When the last run in a sandbox ended.
    pub idle_since: HashMap<String, i64>,
    /// Teardown receipts by sandbox id.
    pub receipts: HashMap<String, TeardownReceiptVm>,
    /// The last phase message seen per sandbox.
    pub phases: HashMap<String, (SandboxPhase, String)>,
}

fn phase_of(p: SandboxPhase) -> EnvironmentPhase {
    match p {
        SandboxPhase::Creating => EnvironmentPhase::Creating,
        SandboxPhase::Ready => EnvironmentPhase::Ready,
        SandboxPhase::Idle => EnvironmentPhase::Idle,
        SandboxPhase::Destroying => EnvironmentPhase::Destroying,
        SandboxPhase::Destroyed => EnvironmentPhase::Destroyed,
        SandboxPhase::Failed => EnvironmentPhase::Failed,
    }
}

impl Inner {
    fn sandbox_event(&self, id: &str, phase: SandboxPhase, text: &str) {
        self.st()
            .env
            .phases
            .insert(id.to_owned(), (phase, text.to_owned()));
        self.emit(ApiEvent::Sandbox {
            sandbox_id: id.to_owned(),
            phase,
            region: Some(REGION.into()),
            text: text.to_owned(),
        });
    }

    /// Make sure the simulation's sandbox is applied and linked to the optimization.
    pub(crate) async fn ensure_sandbox(&self, optimization_id: &str) -> ServiceResult<()> {
        let sid = SandboxId::new(SIM_SANDBOX);
        let existing = self.ledger.get_sandbox(SIM_SANDBOX).await?;
        let alive = existing
            .as_ref()
            .is_some_and(|s| s.status == SandboxStatus::Applied);
        if !alive {
            self.sandbox_event(
                SIM_SANDBOX,
                SandboxPhase::Creating,
                "creating the environment",
            );
            let mut input = RecordSandboxInput::new(sid.clone(), SandboxProviderKind::Mock, REGION);
            input.account_id = Some("000000000000".into());
            input.status = Some(SandboxStatus::Planned);
            // A destroyed sandbox is reused under the same id: bring it back to applied.
            self.ledger.record_sandbox(input).await?;
            self.ledger
                .set_sandbox_status(
                    SIM_SANDBOX,
                    SandboxStatus::Applied,
                    SandboxStatusPatch {
                        outputs: Patch::Set(json!({
                            "artifactsBucket": "ratchet-sim-artifacts",
                            "sealedBucket": "ratchet-sim-sealed",
                        })),
                        ..Default::default()
                    },
                )
                .await?;
            self.sandbox_event(SIM_SANDBOX, SandboxPhase::Ready, "environment ready");
        } else {
            self.sandbox_event(SIM_SANDBOX, SandboxPhase::Ready, "environment ready");
        }
        self.st().env.idle_since.remove(SIM_SANDBOX);
        self.ledger
            .set_optimization_sandbox(&OptimizationId::new(optimization_id), Some(&sid))
            .await?;
        Ok(())
    }

    /// A run ended: when it was the last one in its sandbox, the idle window starts.
    pub(crate) async fn after_run_ended(&self, optimization_id: &str) {
        let Ok(Some(opt)) = self
            .ledger
            .get_optimization(&OptimizationId::new(optimization_id))
            .await
        else {
            return;
        };
        let Some(sid) = opt.sandbox_id else {
            return;
        };
        let Ok(in_box) = self.ledger.optimizations_in_sandbox(sid.as_str()).await else {
            return;
        };
        if in_box.iter().any(|o| o.status.is_active()) {
            return;
        }
        self.st()
            .env
            .idle_since
            .insert(sid.to_string(), self.clock.now());
        self.sandbox_event(sid.as_str(), SandboxPhase::Idle, "idle: no run is active");
    }

    /// The seeded history: a finished run that used a sandbox which was removed afterwards, with its receipt.
    pub(crate) async fn seed_environment(&self) -> ServiceResult<()> {
        let id = "sbx-demo-0";
        let Some(ingest) = self
            .ledger
            .get_optimization(&OptimizationId::new("opt_demo_ingest"))
            .await?
        else {
            return Ok(());
        };
        let ended = ingest.ended_at.unwrap_or_else(|| self.clock.now());
        let mut input =
            RecordSandboxInput::new(SandboxId::new(id), SandboxProviderKind::Mock, REGION);
        input.status = Some(SandboxStatus::Applied);
        input.created_at = Some(ingest.created_at);
        self.ledger.record_sandbox(input).await?;
        self.ledger
            .set_optimization_sandbox(&ingest.id, Some(&SandboxId::new(id)))
            .await?;
        self.ledger
            .set_sandbox_status(
                id,
                SandboxStatus::Destroyed,
                SandboxStatusPatch {
                    outputs: Patch::Clear,
                    at: Some(ended + 30 * 60_000),
                    ..Default::default()
                },
            )
            .await?;
        let receipt = self
            .build_receipt(id, ReceiptTrigger::Idle, ended + 30 * 60_000)
            .await?;
        self.st().env.receipts.insert(id.to_owned(), receipt);
        Ok(())
    }

    async fn build_receipt(
        &self,
        sandbox_id: &str,
        trigger: ReceiptTrigger,
        at: i64,
    ) -> ServiceResult<TeardownReceiptVm> {
        let runs = self.ledger.optimizations_in_sandbox(sandbox_id).await?;
        let mut by_cat = [0.0f64; 5];
        let mut receipt_runs = Vec::new();
        let mut agents = 0;
        for o in &runs {
            let b = self.ledger.spend_breakdown(&o.id).await?;
            for (i, c) in SpendCategory::ALL.iter().enumerate() {
                by_cat[i] += b.by_category.get(*c);
            }
            agents += self.ledger.list_agents(&o.id).await?.len() as i64;
            receipt_runs.push(ReceiptRun {
                optimization_id: o.id.to_string(),
                status: o.status.to_string(),
                spend_usd: b.total,
            });
        }
        let total: f64 = by_cat.iter().sum();
        let spend_by_category = SpendCategory::ALL
            .iter()
            .enumerate()
            .filter(|(i, _)| by_cat[*i] > 0.0)
            .map(|(i, c)| LabeledUsd {
                label: match c {
                    SpendCategory::AgentRunner => "Agent runners",
                    SpendCategory::Experiment => "Experiment instances",
                    SpendCategory::BenchRunner => "Bench runner",
                    SpendCategory::StorageEgress => "Storage & egress",
                    SpendCategory::Tokens => "Model tokens",
                }
                .to_owned(),
                usd: by_cat[i],
            })
            .collect();
        Ok(TeardownReceiptVm {
            sandbox_id: sandbox_id.to_owned(),
            region: REGION.to_owned(),
            trigger,
            destroyed_at: at,
            spend_usd: total,
            spend_by_category,
            runs: receipt_runs,
            removed: ReceiptRemoved {
                instances: 2 * agents + 1,
                agent_identities: agents,
                secrets: agents,
                resources: vec![
                    format!("ratchet-{sandbox_id}-artifacts"),
                    format!("ratchet-{sandbox_id}-sealed"),
                    "vpc".into(),
                ],
            },
            nothing_running: true,
            orphans: vec![],
            errors: vec![],
            summary: format!(
                "Environment removed; nothing tagged is left running. {} run(s) cost ${total:.2}.",
                runs.len()
            ),
        })
    }

    pub(crate) async fn receipt_for(
        &self,
        optimization_id: &str,
    ) -> ServiceResult<Option<TeardownReceiptVm>> {
        let Some(opt) = self
            .ledger
            .get_optimization(&OptimizationId::new(optimization_id))
            .await?
        else {
            return Err(ServiceError::not_found("optimization", optimization_id));
        };
        Ok(opt
            .sandbox_id
            .and_then(|s| self.st().env.receipts.get(s.as_str()).cloned()))
    }

    pub(crate) async fn environments(&self) -> ServiceResult<EnvironmentsVm> {
        let now = self.clock.now();
        let idle_ms = i64::from(self.opts.idle_minutes) * 60_000;
        let mut out = Vec::new();
        for s in self.ledger.list_sandboxes().await? {
            if matches!(s.status, SandboxStatus::Destroyed | SandboxStatus::Planned) {
                continue;
            }
            let runs = self.ledger.optimizations_in_sandbox(s.id.as_str()).await?;
            let active: Vec<_> = runs.iter().filter(|o| o.status.is_active()).collect();
            let mut spend = 0.0;
            for o in &active {
                spend += self.ledger.total_spend(&o.id).await?;
            }
            let (seen, seen_text) = self
                .st()
                .env
                .phases
                .get(s.id.as_str())
                .map_or((None, None), |(p, t)| (Some(*p), Some(t.clone())));
            let derived = match s.status {
                SandboxStatus::Applying => EnvironmentPhase::Creating,
                SandboxStatus::Destroying => EnvironmentPhase::Destroying,
                SandboxStatus::Failed => EnvironmentPhase::Failed,
                _ if !active.is_empty() => EnvironmentPhase::Ready,
                _ => EnvironmentPhase::Idle,
            };
            let phase = match seen {
                Some(
                    p @ (SandboxPhase::Failed | SandboxPhase::Destroying | SandboxPhase::Creating),
                ) => phase_of(p),
                _ => derived,
            };
            let idle_since = if active.is_empty() {
                self.st().env.idle_since.get(s.id.as_str()).copied()
            } else {
                None
            };
            out.push(EnvironmentVm {
                sandbox_id: s.id.to_string(),
                region: Some(s.region.clone()),
                phase,
                text: seen_text.unwrap_or_else(|| match phase {
                    EnvironmentPhase::Idle => "idle: no run is active".into(),
                    EnvironmentPhase::Ready => "environment ready".into(),
                    other => format!("{other:?}").to_lowercase(),
                }),
                simulated: s.provider == SandboxProviderKind::Mock,
                active_runs: active.iter().map(|o| o.id.to_string()).collect(),
                instances_running: Some(active.len() as u64 * 3),
                spend_usd: spend,
                destroy_at: idle_since.map(|t| t + idle_ms),
                idle_since,
                can_end: s.status == SandboxStatus::Applied && active.is_empty(),
            });
        }
        let last_receipt = self
            .st()
            .env
            .receipts
            .values()
            .max_by_key(|r| r.destroyed_at)
            .cloned();
        let _ = now;
        Ok(EnvironmentsVm {
            environments: out,
            last_receipt,
            idle_minutes: self.opts.idle_minutes,
        })
    }

    pub(crate) async fn end_environment(&self, sandbox_id: &str) -> ServiceResult<()> {
        let Some(s) = self.ledger.get_sandbox(sandbox_id).await? else {
            return Err(ServiceError::not_found("environment", sandbox_id));
        };
        if s.status != SandboxStatus::Applied {
            return Err(ServiceError::Conflict(format!(
                "environment {sandbox_id} is {} and cannot be ended",
                s.status
            )));
        }
        let runs = self.ledger.optimizations_in_sandbox(sandbox_id).await?;
        if runs.iter().any(|o| {
            matches!(
                o.status,
                OptimizationStatus::Baselining
                    | OptimizationStatus::Running
                    | OptimizationStatus::Stopping
            )
        }) {
            return Err(ServiceError::Conflict(
                "a run is still active in this environment; stop it first".into(),
            ));
        }
        self.sandbox_event(
            sandbox_id,
            SandboxPhase::Destroying,
            "removing the environment",
        );
        let at = self.clock.now();
        self.ledger
            .set_sandbox_status(
                sandbox_id,
                SandboxStatus::Destroyed,
                SandboxStatusPatch {
                    outputs: Patch::Clear,
                    at: Some(at),
                    ..Default::default()
                },
            )
            .await?;
        let receipt = self
            .build_receipt(sandbox_id, ReceiptTrigger::Manual, at)
            .await?;
        {
            let mut st = self.st();
            st.env.receipts.insert(sandbox_id.to_owned(), receipt);
            st.env.idle_since.remove(sandbox_id);
        }
        self.sandbox_event(sandbox_id, SandboxPhase::Destroyed, "environment removed");
        Ok(())
    }
}
