//! The invisible sandbox (docs/SIMPLIFIED_MODEL.md, "Sandbox lifecycle: invisible"). Port of
//! `packages/engine/src/sandbox-auto.ts`.
//!
//!  - Created lazily at the first run start in a region (`ensure_sandbox_for`), from `ratchet.yaml`
//!    (`environment.region`) or the default.
//!  - Reused while runs are active; every `tick()` looks at it.
//!  - Destroyed automatically once no run is active for the idle window (default 30 min): ended runs are torn down first
//!    (identities, secrets, leftover instances), the destroy goes through the same orphan check as an explicit one, and a
//!    `SandboxTeardownReceipt` is recorded: what was removed, what it cost, proof that nothing is left running.
//!
//! Only sandboxes the engine created are destroyed automatically; one applied explicitly (`apply_sandbox`) stays until
//! `destroy_sandbox`. The idle clock is persisted, so a sandbox that outlived the app is destroyed on the first tick
//! after launch.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use ratchet_core::config::RatchetConfig;
use ratchet_core::enums::{Mode, OptimizationStatus, SandboxStatus};
use ratchet_core::ports::{
    EngineEvent, InstanceState, SandboxActivity, SandboxInput, SandboxOutputs, SandboxPhase,
    SandboxStateStatus, SandboxTeardownReceipt, TeardownOrphan, TeardownRemoved, TeardownRun,
    TeardownTrigger,
};
use ratchet_core::rows::Optimization;

use crate::engine::{Engine, is_not_configured, lock, oid};
use crate::errors::{EngineError, EngineErrorCode, EngineResult};
use crate::kv::k;
use crate::misc::TeardownReport;

fn active(s: OptimizationStatus) -> bool {
    matches!(
        s,
        OptimizationStatus::Baselining | OptimizationStatus::Running | OptimizationStatus::Stopping
    )
}

fn ended(s: OptimizationStatus) -> bool {
    matches!(s, OptimizationStatus::Stopped | OptimizationStatus::Failed)
}

#[derive(Default)]
pub struct AutoState {
    /// Serializes ensure / destroy per sandbox (one apply or destroy at a time; the lifecycle refuses concurrent ones).
    chains: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// After a failed automatic destroy, do not retry before this time.
    retry_after: Mutex<HashMap<String, i64>>,
}

/// `environment.region` of ratchet.yaml, else `fallback`.
pub fn region_of(config: &RatchetConfig, fallback: &str) -> String {
    config
        .environment
        .as_ref()
        .and_then(|e| e.region.as_deref())
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

impl Engine {
    fn queue_for(&self, sandbox_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.auto.chains)
            .entry(sandbox_id.to_owned())
            .or_default()
            .clone()
    }

    pub fn auto_sandbox_id_for(&self, region: &str) -> String {
        let s = &self.settings.sandbox;
        if let Some(f) = &s.sandbox_id {
            return f(region);
        }
        let base = if self.mode == Mode::Live {
            "ratchet-sandbox"
        } else {
            "ratchet-sim"
        };
        if region == s.default_region {
            base.to_owned()
        } else {
            format!("{base}-{region}")
        }
    }

    fn phase(&self, sandbox_id: &str, phase: SandboxPhase, region: Option<&str>, text: String) {
        self.emit(EngineEvent::Sandbox {
            sandbox_id: sandbox_id.to_owned(),
            phase,
            region: region.map(str::to_owned),
            text,
        });
    }

    // ------------------------------------------------------------------ create (first run start in a region)

    /// Make sure the optimization's sandbox is applied, creating it if needed, and link the optimization to it. The
    /// idle clock stops. Fails with `sandbox_unavailable` when the apply fails (the optimization stays a draft).
    pub(crate) async fn ensure_sandbox_for(&self, optimization_id: &str) -> EngineResult<String> {
        let opt = self.require_opt(optimization_id).await?;
        let config = self.config(optimization_id).await?;
        let existing = match &opt.sandbox_id {
            Some(s) => self.ledger.get_sandbox(s.as_str()).await?,
            None => None,
        };
        let region = existing
            .map(|e| e.region)
            .unwrap_or_else(|| region_of(&config, &self.settings.sandbox.default_region));
        let sandbox_id = opt
            .sandbox_id
            .as_ref()
            .map_or_else(|| self.auto_sandbox_id_for(&region), |s| s.to_string());

        {
            let q = self.queue_for(&sandbox_id);
            let _turn = q.lock().await;
            let row = self.ledger.get_sandbox(&sandbox_id).await?;
            if row.is_none_or(|r| r.status != SandboxStatus::Applied) {
                let input = self.sandbox_input_for(&sandbox_id, &region, &config);
                self.phase(
                    &sandbox_id,
                    SandboxPhase::Creating,
                    Some(&region),
                    format!("creating the environment in {region}"),
                );
                if let Err(err) = self.apply_lifecycle(&input).await {
                    self.phase(
                        &sandbox_id,
                        SandboxPhase::Failed,
                        Some(&region),
                        format!("could not create the environment: {}", err.message),
                    );
                    return Err(EngineError::new(
                        EngineErrorCode::SandboxUnavailable,
                        format!(
                            "could not create the environment in {region}: {}",
                            err.message
                        ),
                    ));
                }
                // ours: destroyed automatically after the idle window
                self.kv.set(&k::sandbox_auto(&sandbox_id), "1").await?;
                self.phase(
                    &sandbox_id,
                    SandboxPhase::Ready,
                    Some(&region),
                    "environment ready".into(),
                );
            }
            self.kv.delete(&k::sandbox_idle(&sandbox_id)).await?;
        }
        if opt.sandbox_id.as_ref().map(|s| s.as_str()) != Some(sandbox_id.as_str()) {
            self.ledger
                .set_optimization_sandbox(&oid(optimization_id), Some(&sandbox_id.clone().into()))
                .await?;
        }
        Ok(sandbox_id)
    }

    fn sandbox_input_for(
        &self,
        sandbox_id: &str,
        region: &str,
        config: &RatchetConfig,
    ) -> SandboxInput {
        if let Some(custom) = self.settings.sandbox.input.as_ref().and_then(|f| {
            f(&crate::settings::SandboxInputContext {
                sandbox_id,
                region,
                config,
            })
        }) {
            return custom;
        }
        let mut types: Vec<String> = Vec::new();
        for t in config
            .agent
            .allowed_instance_types
            .iter()
            .chain(std::iter::once(&config.evaluator.hardware))
        {
            if !types.contains(t) {
                types.push(t.clone());
            }
        }
        SandboxInput {
            sandbox_id: sandbox_id.to_owned(),
            region: region.to_owned(),
            allowed_instance_types: types,
            max_instances_per_agent: 2,
            max_instance_lifetime_hours: 4.0,
            tags: Some(BTreeMap::from([(
                "ratchet:managed".to_owned(),
                "true".to_owned(),
            )])),
        }
    }

    // ------------------------------------------------------------------ idle clock + destroy

    /// Called by the engine's `tick()`. Never fails.
    pub(crate) async fn auto_tick(&self) {
        if !self.settings.sandbox.auto {
            return;
        }
        let rows = match self.ledger.list_sandboxes().await {
            Ok(r) => r,
            Err(e) => {
                self.report_error(e, "auto sandbox list");
                return;
            }
        };
        for row in rows {
            // `failed` + ours: a destroy that left infrastructure of unknown state; keep retrying it (with back-off).
            if !matches!(row.status, SandboxStatus::Applied | SandboxStatus::Failed) {
                continue;
            }
            match self.kv.has(&k::sandbox_auto(row.id.as_str())).await {
                Ok(true) => {}
                _ => continue,
            }
            if let Err(err) = self.auto_tick_one(row.id.as_str()).await {
                self.report_error(err, &format!("auto sandbox {}", row.id));
            }
        }
    }

    async fn active_runs(&self, sandbox_id: &str) -> EngineResult<Vec<Optimization>> {
        Ok(self
            .ledger
            .optimizations_in_sandbox(sandbox_id)
            .await?
            .into_iter()
            .filter(|o| active(o.status))
            .collect())
    }

    /// When the idle window started: persisted at the first observation, derived from the last run's end (or the apply)
    /// until then.
    async fn idle_since(&self, sandbox_id: &str) -> EngineResult<i64> {
        if let Some(known) = self.kv.get_i64(&k::sandbox_idle(sandbox_id)).await? {
            return Ok(known);
        }
        let row = self.ledger.get_sandbox(sandbox_id).await?;
        let mut latest = row.and_then(|r| r.applied_at).unwrap_or(0);
        for o in self.ledger.optimizations_in_sandbox(sandbox_id).await? {
            latest = latest.max(o.ended_at.unwrap_or(o.created_at));
        }
        let since = self.clock.now().min(latest);
        self.kv
            .set(&k::sandbox_idle(sandbox_id), &since.to_string())
            .await?;
        Ok(since)
    }

    async fn auto_tick_one(&self, sandbox_id: &str) -> EngineResult<()> {
        if !self.active_runs(sandbox_id).await?.is_empty() || !lock(&self.pending_starts).is_empty()
        {
            self.kv.delete(&k::sandbox_idle(sandbox_id)).await?;
            return Ok(());
        }
        let first = !self.kv.has(&k::sandbox_idle(sandbox_id)).await?;
        let since = self.idle_since(sandbox_id).await?;
        if first {
            let region = self.ledger.get_sandbox(sandbox_id).await?.map(|r| r.region);
            self.phase(
                sandbox_id,
                SandboxPhase::Idle,
                region.as_deref(),
                format!(
                    "no run is active; the environment is removed in {} min unless a run starts",
                    (self.settings.sandbox.idle_ms as f64 / 60_000.0).round()
                ),
            );
        }
        if self.clock.now() - since < self.settings.sandbox.idle_ms {
            return Ok(());
        }
        if self.clock.now()
            < lock(&self.auto.retry_after)
                .get(sandbox_id)
                .copied()
                .unwrap_or(0)
        {
            return Ok(());
        }
        if let Err(err) = self.destroy_auto(sandbox_id, TeardownTrigger::Idle).await {
            lock(&self.auto.retry_after).insert(
                sandbox_id.to_owned(),
                self.clock.now() + self.settings.sandbox.retry_ms,
            );
            self.report_error(err, &format!("auto destroy {sandbox_id}"));
        }
        Ok(())
    }

    /// Tear the sandbox down now: end every finished run's cloud resources, destroy, verify, and record the receipt.
    /// Refuses while a run is active. `Idle` is the automatic path; `Manual` is [`Engine::end_sandbox`].
    async fn destroy_auto(
        &self,
        sandbox_id: &str,
        trigger: TeardownTrigger,
    ) -> EngineResult<SandboxTeardownReceipt> {
        let q = self.queue_for(sandbox_id);
        let _turn = q.lock().await;
        self.destroy_now(sandbox_id, trigger).await
    }

    async fn destroy_now(
        &self,
        sandbox_id: &str,
        trigger: TeardownTrigger,
    ) -> EngineResult<SandboxTeardownReceipt> {
        let Some(row) = self.ledger.get_sandbox(sandbox_id).await? else {
            return Err(EngineError::not_found(format!(
                "sandbox {sandbox_id} not found"
            )));
        };
        if row.status != SandboxStatus::Applied && row.status != SandboxStatus::Failed {
            return Err(EngineError::invalid_state(format!(
                "sandbox {sandbox_id} is {}; there is nothing to end",
                row.status
            )));
        }
        let active_runs = self.active_runs(sandbox_id).await?;
        let pending = !lock(&self.pending_starts).is_empty();
        if !active_runs.is_empty() || pending {
            return Err(EngineError::new(
                EngineErrorCode::SandboxInUse,
                format!(
                    "sandbox {sandbox_id} is in use by {}",
                    if active_runs.is_empty() {
                        "a run that is starting".to_owned()
                    } else {
                        active_runs
                            .iter()
                            .map(|o| o.id.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ),
            ));
        }
        let region = row.region.clone();
        let outputs: Option<SandboxOutputs> = match &row.outputs {
            Some(v) => serde_json::from_value(v.clone()).ok(),
            None => self.ports.sandbox.outputs(sandbox_id).await.ok().flatten(),
        };
        let mut errors: Vec<String> = Vec::new();
        self.phase(
            sandbox_id,
            SandboxPhase::Destroying,
            Some(&region),
            "removing the environment".into(),
        );

        // 1. finish the cloud side of every ended run (the destroy refuses while their identities or instances remain)
        let runs = self.ledger.optimizations_in_sandbox(sandbox_id).await?;
        for o in &runs {
            if !ended(o.status) {
                continue;
            }
            if let Err(err) = self.teardown(o.id.as_str()).await {
                errors.push(format!("teardown {}: {}", o.id, err.message));
            }
        }
        let mut reports: Vec<TeardownReport> = Vec::new();
        for o in &runs {
            if let Some(r) = self.teardown_report(o.id.as_str()).await? {
                reports.push(r);
            }
        }
        for r in &reports {
            for e in &r.errors {
                if !errors.contains(e) {
                    errors.push(e.clone());
                }
            }
        }

        // 2. destroy through the lifecycle (same orphan check as the explicit path)
        if let Err(err) = self.sandboxes.destroy(sandbox_id).await {
            self.phase(
                sandbox_id,
                SandboxPhase::Failed,
                Some(&region),
                format!("could not remove the environment: {err}"),
            );
            return Err(err.into());
        }

        // 3. verify: nothing tagged is left running
        let mut orphans: Vec<TeardownOrphan> = Vec::new();
        match self.ports.compute.list_tagged(None).await {
            Ok(all) => {
                for i in all
                    .into_iter()
                    .filter(|i| i.state != InstanceState::Terminated)
                {
                    orphans.push(TeardownOrphan {
                        id: i.id,
                        role: json_str(&i.role),
                        state: json_str(&i.state),
                    });
                }
            }
            // No compute without sandbox outputs: nothing can be running. Anything else is reported, not hidden.
            Err(e) if is_not_configured(&e) => {}
            Err(e) => errors.push(format!("verify listTagged: {e}")),
        }

        let after = self.ledger.optimizations_in_sandbox(sandbox_id).await?;
        let mut spend_by_category: BTreeMap<String, f64> = BTreeMap::new();
        let mut spend = 0.0;
        let mut run_summaries = Vec::new();
        for o in &after {
            let usd = self.ledger.total_spend(&o.id).await?;
            spend += usd;
            let breakdown = self.ledger.spend_breakdown(&o.id).await?;
            for cat in ratchet_core::enums::SpendCategory::ALL {
                let v = breakdown.by_category.get(*cat);
                if v != 0.0 {
                    *spend_by_category.entry(cat.to_string()).or_default() += v;
                }
            }
            run_summaries.push(TeardownRun {
                optimization_id: o.id.clone(),
                status: o.status.to_string(),
                spend_usd: usd,
            });
        }
        let mut instances: Vec<String> = Vec::new();
        let mut add = |i: String| {
            if !instances.contains(&i) {
                instances.push(i);
            }
        };
        for o in &after {
            for src in self.ledger.spend_sources(&o.id, "ec2:").await? {
                add(src["ec2:".len()..].to_owned());
            }
        }
        for r in &reports {
            for i in &r.terminated_instance_ids {
                add(i.clone());
            }
        }
        let dedup = |it: Vec<String>| {
            let mut out: Vec<String> = Vec::new();
            for s in it {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
            out
        };

        let nothing_running =
            orphans.is_empty() && !errors.iter().any(|e| e.starts_with("verify listTagged"));
        let mut receipt = SandboxTeardownReceipt {
            sandbox_id: sandbox_id.to_owned(),
            region: region.clone(),
            trigger,
            created_at: row.applied_at,
            destroyed_at: self.clock.now(),
            runs: run_summaries,
            spend_usd: spend,
            spend_by_category,
            removed: TeardownRemoved {
                instances,
                agent_identities: dedup(
                    reports
                        .iter()
                        .flat_map(|r| r.deleted_identities_for.clone())
                        .collect(),
                ),
                secrets: dedup(
                    reports
                        .iter()
                        .flat_map(|r| r.deleted_secrets_for.clone())
                        .collect(),
                ),
                resources: outputs.as_ref().map(resources_of).unwrap_or_default(),
            },
            nothing_running,
            orphans,
            errors,
            summary: String::new(),
        };
        receipt.summary = summarize(&receipt);
        self.kv
            .set_json(&k::sandbox_receipt(sandbox_id), &receipt)
            .await?;
        for o in &after {
            self.kv
                .set_json(&k::opt_receipt(o.id.as_str()), &receipt)
                .await?;
        }
        self.kv.delete(&k::sandbox_auto(sandbox_id)).await?;
        self.kv.delete(&k::sandbox_idle(sandbox_id)).await?;
        lock(&self.auto.retry_after).remove(sandbox_id);
        self.phase(
            sandbox_id,
            SandboxPhase::Destroyed,
            Some(&region),
            receipt.summary.clone(),
        );
        Ok(receipt)
    }

    // ------------------------------------------------------------------ reads

    /// The receipt of the sandbox teardown that covered the optimization, or `None` while its sandbox is still alive.
    pub async fn teardown_receipt(
        &self,
        optimization_id: &str,
    ) -> EngineResult<Option<SandboxTeardownReceipt>> {
        crate::errors::assert_valid_id("optimization id", optimization_id)?;
        self.ready().await;
        self.kv.get_json(&k::opt_receipt(optimization_id)).await
    }

    /// The automatic sandbox seen from outside: running instances, spend of active runs, when it will be removed, last
    /// receipt.
    pub async fn sandbox_activity(&self, sandbox_id: &str) -> EngineResult<SandboxActivity> {
        self.ready().await;
        let row = self.ledger.get_sandbox(sandbox_id).await?;
        let auto = self.kv.has(&k::sandbox_auto(sandbox_id)).await?;
        let applied = row
            .as_ref()
            .is_some_and(|r| r.status == SandboxStatus::Applied);
        let active_runs = if applied {
            self.active_runs(sandbox_id).await?
        } else {
            vec![]
        };
        let instances_running = if applied {
            self.ports.compute.list_tagged(None).await.ok().map(|l| {
                l.iter()
                    .filter(|i| i.state != InstanceState::Terminated)
                    .count() as u64
            })
        } else {
            Some(0)
        };
        let idle = if applied && active_runs.is_empty() {
            Some(self.idle_since(sandbox_id).await?)
        } else {
            None
        };
        let mut spend = 0.0;
        for o in &active_runs {
            spend += self.ledger.total_spend(&o.id).await?;
        }
        Ok(SandboxActivity {
            sandbox_id: sandbox_id.to_owned(),
            status: row
                .as_ref()
                .map_or(SandboxStateStatus::Absent, |r| r.status.into()),
            region: row.map(|r| r.region),
            auto,
            active_runs: active_runs.iter().map(|o| o.id.clone()).collect(),
            instances_running,
            spend_usd: spend,
            idle_since: idle,
            destroy_at: idle
                .filter(|_| auto)
                .map(|i| i + self.settings.sandbox.idle_ms),
            last_receipt: self.kv.get_json(&k::sandbox_receipt(sandbox_id)).await?,
        })
    }

    /// End the sandbox now instead of after the idle window: tears down the finished runs' cloud resources, destroys the
    /// sandbox, verifies nothing is left and returns the receipt. Refuses (`sandbox_in_use`) while a run is active.
    pub async fn end_sandbox(&self, sandbox_id: &str) -> EngineResult<SandboxTeardownReceipt> {
        self.ready().await;
        self.destroy_auto(sandbox_id, TeardownTrigger::Manual).await
    }

    /// An explicit apply / destroy takes the sandbox out of automatic management.
    pub(crate) async fn release_auto(&self, sandbox_id: &str) -> EngineResult<()> {
        self.kv.delete(&k::sandbox_auto(sandbox_id)).await?;
        self.kv.delete(&k::sandbox_idle(sandbox_id)).await
    }
}

fn json_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn resources_of(o: &SandboxOutputs) -> Vec<String> {
    vec![
        format!("S3 bucket {} (artifacts)", o.artifacts_bucket),
        format!("S3 bucket {} (sealed holdout)", o.sealed_bucket),
        format!("network subnet {}", o.network.subnet_id),
        format!(
            "security groups {}, {}",
            o.network.agent_runner_security_group_id, o.network.bench_security_group_id
        ),
        format!("agent permission boundary {}", o.agent_boundary_ref),
        format!("evaluator identity {}", o.eval_identity_ref),
    ]
}

fn summarize(r: &SandboxTeardownReceipt) -> String {
    let usd = format!("${}", ratchet_core::numfmt::to_fixed(r.spend_usd, 2));
    let runs = format!(
        "{} run{}",
        r.runs.len(),
        if r.runs.len() == 1 { "" } else { "s" }
    );
    let left = if r.nothing_running {
        "Nothing is left running.".to_owned()
    } else {
        format!(
            "{} resource(s) may still be running: check the environment.",
            if r.orphans.is_empty() {
                "Some".to_owned()
            } else {
                r.orphans.len().to_string()
            }
        )
    };
    format!(
        "Environment in {} removed after {runs}, {usd} spent. {} instance(s) terminated, {} resources removed. {left}",
        r.region,
        r.removed.instances.len(),
        r.removed.resources.len()
    )
}
