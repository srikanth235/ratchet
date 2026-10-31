//! Pull request, cost reconcile, templates and teardown. Part of the port of `packages/engine/src/engine.ts`.

use std::future::Future;

use ratchet_core::enums::{AgentsWhere, Direction, OptimizationStatus};
use ratchet_core::numfmt::{precision_trimmed, to_fixed};
use ratchet_core::ports::{
    AgentIdentity, CommitFile, CommitFilesInput, CostReconcileReport, CostReconcileStatus,
    EngineEvent, InstanceState, NoticeLevel, PortError, ProposeTemplateInput,
    ProposeTemplateResult, PullRequestInput, PullRequestRef, TeardownOrphan,
};
use ratchet_core::{
    ReconcileSpendOptions, TemplateInfo, build_template_proposal, get_template, list_templates,
    reconcile_spend,
};
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, oid};
use crate::errors::{EngineError, EngineErrorCode, EngineResult, assert_valid_id};
use crate::kv::k;
use crate::report::default_report;

/// Best-effort cost reconcile at teardown: `pending` is normal (Cost Explorer lags up to 24 h); re-run `reconcile_costs`
/// later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeardownCostReconcile {
    /// `reconciled | unchanged | pending`, or `error`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub written_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeardownReport {
    pub optimization_id: String,
    pub at: i64,
    pub deleted_secrets_for: Vec<String>,
    pub deleted_identities_for: Vec<String>,
    pub terminated_instance_ids: Vec<String>,
    pub orphans: Vec<TeardownOrphan>,
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_reconcile: Option<TeardownCostReconcile>,
}

fn fmt5(n: f64) -> String {
    precision_trimmed(n, 5)
}

async fn attempt<F, Fut>(errors: &mut Vec<String>, engine: &Engine, what: &str, f: F) -> bool
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), EngineError>>,
{
    match f().await {
        Ok(()) => true,
        Err(err) => {
            errors.push(format!("{what}: {}", err.message));
            engine.report_error(&err, &format!("teardown {what}"));
            false
        }
    }
}

impl Engine {
    // ------------------------------------------------------------------ templates

    pub async fn list_templates(&self) -> Vec<TemplateInfo> {
        list_templates(Default::default())
    }

    /// "Use template": add a template's files to a repository through a pull request. Nothing in the repo is changed
    /// except by the PR: the files go on a new branch (`ratchet/template-<id>`, with a suffix when it is taken), the PR
    /// description says what was added and how to run a baseline. Refuses when files of the template already exist on
    /// `base_ref` (`overwrite: true` replaces them) and when the git host cannot commit files.
    pub async fn propose_template(
        &self,
        input: &ProposeTemplateInput,
    ) -> EngineResult<ProposeTemplateResult> {
        self.ready().await;
        let bad = |m: String| Err(EngineError::invalid_input(m));
        if input.repo_url.is_empty() {
            return bad("repoUrl is required".into());
        }
        if input.base_ref.is_empty() {
            return bad("baseRef is required".into());
        }
        let Some(info) = get_template(&input.template_id) else {
            return bad(format!(
                "unknown template {}",
                serde_json::to_string(&input.template_id).unwrap_or_default()
            ));
        };
        if info.kind == ratchet_core::enums::TemplateKind::Demo {
            return bad(format!(
                "{} is a demo ({}); it runs in simulation and adds nothing to a repository",
                info.id,
                info.example.clone().unwrap_or_default()
            ));
        }
        if let Some(v) = input.value_usd
            && !(v.is_finite() && v > 0.0)
        {
            return bad("valueUsd must be a positive number".into());
        }
        let proposal = build_template_proposal(&info.id, &input.render_options())
            .map_err(|e| EngineError::invalid_input(e.to_string()))?;
        let git = self.git_for(&input.repo_url);
        let unavailable = || {
            EngineError::new(
                EngineErrorCode::Unavailable,
                "this git host cannot commit files, so it cannot open a template pull request",
            )
        };

        if !input.overwrite.unwrap_or(false) {
            let paths: Vec<String> = proposal.files.iter().map(|f| f.path.clone()).collect();
            let present = self.paths_present(&*git, &input.base_ref, &paths).await?;
            if !present.is_empty() {
                let one = present.len() == 1;
                return Err(EngineError::invalid_state(format!(
                    "{} already exist{} on {}; the template would replace {} (pass overwrite to allow that)",
                    present.join(", "),
                    if one { "s" } else { "" },
                    input.base_ref,
                    if one { "it" } else { "them" }
                ))
                .with_details(serde_json::json!({ "existing": present })));
            }
        }

        let files: Vec<CommitFile> = proposal
            .files
            .iter()
            .map(|f| CommitFile::new(f.path.clone(), f.content.clone()))
            .collect();
        let commit = |branch: &str| CommitFilesInput {
            base_ref: input.base_ref.clone(),
            branch: branch.to_owned(),
            message: proposal.commit_message.clone(),
            files: files.clone(),
        };
        let mut branch = proposal.branch.clone();
        let sha = match git.commit_files(&commit(&branch)).await {
            Ok(r) => r.sha,
            Err(PortError::Unsupported(_)) => return Err(unavailable()),
            Err(first) => {
                // most likely the branch exists from an earlier proposal: take a fresh name once, then give up
                branch = format!(
                    "{}-{}",
                    proposal.branch,
                    base36(self.clock.now().max(0) as u64)
                );
                match git.commit_files(&commit(&branch)).await {
                    Ok(r) => r.sha,
                    Err(_) => return Err(first.into()),
                }
            }
        };
        let pr = git
            .open_pull_request(&PullRequestInput {
                base: input.base_ref.clone(),
                head: branch.clone(),
                title: proposal.title.clone(),
                body: proposal.body.clone(),
                shas: vec![],
                commit_message: None,
            })
            .await?;
        Ok(ProposeTemplateResult {
            pr_url: pr.url,
            branch,
            sha,
            template_id: info.id,
            files: proposal.files.iter().map(|f| f.path.clone()).collect(),
        })
    }

    /// Which of `paths` exist in the tree of `git_ref`.
    async fn paths_present(
        &self,
        git: &dyn ratchet_core::ports::GitHost,
        git_ref: &str,
        paths: &[String],
    ) -> EngineResult<Vec<String>> {
        let root = self.scratch_root();
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| EngineError::internal(e.to_string()))?;
        let dir = tempfile::Builder::new()
            .prefix("ratchet-engine-")
            .tempdir_in(&root)
            .map_err(|e| EngineError::internal(e.to_string()))?;
        git.checkout(git_ref, dir.path()).await?;
        let mut present = Vec::new();
        for p in paths {
            if tokio::fs::metadata(dir.path().join(p)).await.is_ok() {
                present.push(p.clone());
            }
        }
        Ok(present)
    }

    // ------------------------------------------------------------------ pull request

    pub async fn open_pull_request(&self, id: &str) -> EngineResult<PullRequestRef> {
        assert_valid_id("optimization id", id)?;
        self.ready().await;
        let opt = self.require_opt(id).await?;
        if let Some(existing) = self.kv.get(&k::pr(id)).await? {
            return Ok(PullRequestRef { url: existing });
        }
        if opt.status != OptimizationStatus::Stopped && opt.status != OptimizationStatus::Completed
        {
            return Err(EngineError::invalid_state(format!(
                "optimization {id} is {}; stop it before opening the pull request",
                opt.status
            )));
        }
        let history = self.ledger.ratchet_history(&oid(id)).await?;
        if history.is_empty() {
            return Err(EngineError::new(
                EngineErrorCode::NoVerified,
                format!("optimization {id} has no verified candidate to propose"),
            ));
        }
        let shas: Vec<String> = history.iter().map(|m| m.sha.clone()).collect();
        let baseline = self.ledger.get_baseline(&oid(id)).await?;
        let best = self.ledger.current_best(&oid(id)).await?.unwrap_or(0.0);
        let gain = match &baseline {
            Some(b) if b.value != 0.0 => (best - b.value).abs() / b.value.abs() * 100.0,
            _ => 0.0,
        };
        let title = format!(
            "Ratchet: {} {} -> {} ({}% {}, {} verified)",
            opt.metric,
            baseline.as_ref().map_or("?".to_owned(), |b| fmt5(b.value)),
            fmt5(best),
            fmt5(gain),
            if opt.direction == Direction::Minimize {
                "lower"
            } else {
                "higher"
            },
            shas.len()
        );
        let body = match &self.settings.report_builder {
            Some(b) => b
                .build(&self.ledger, &oid(id))
                .await
                .map_err(EngineError::internal)?,
            None => default_report(&self.ledger, &oid(id)).await?,
        };
        let res = self
            .git_for(&opt.repo)
            .open_pull_request(&PullRequestInput {
                base: opt.base_ref.clone(),
                head: format!("ratchet/{id}/result"),
                title,
                body: body.clone(),
                shas,
                commit_message: None,
            })
            .await?;
        self.kv.set(&k::pr(id), &res.url).await?;
        if let Err(e) = self
            .ports
            .artifacts
            .put_str(&format!("engine/{id}/report.md"), &body)
            .await
        {
            self.report_error(e, "artifacts.put report");
        }
        Ok(res)
    }

    // ------------------------------------------------------------------ cost reconcile

    /// Compare the ledger's estimated spend with Cost Explorer (by cost-allocation tag) and append correction rows
    /// (`estimated=false`, `source='cost-explorer'`, may be negative). Idempotent. Writes nothing and returns `pending`
    /// while billing data is incomplete (optimization still running, within the billing lag of its end, data flagged as
    /// estimated, or no data yet).
    pub async fn reconcile_costs(&self, id: &str) -> EngineResult<CostReconcileReport> {
        assert_valid_id("optimization id", id)?;
        self.ready().await;
        self.require_opt(id).await?;
        let Some(reconciler) = self.ports.costs.clone() else {
            return Err(EngineError::new(
                EngineErrorCode::Unavailable,
                "no cost reconciler is configured (billing data is not available in this runtime)",
            ));
        };
        let report = reconcile_spend(
            &self.ledger,
            &*reconciler,
            &oid(id),
            ReconcileSpendOptions {
                now: self.clock.now(),
                lag_ms: Some(self.settings.cost_lag_ms),
                epsilon_usd: None,
            },
        )
        .await
        .map_err(|e| match e {
            ratchet_core::cost::ReconcileError::NotFound(_) => {
                EngineError::not_found(e.to_string())
            }
            ratchet_core::cost::ReconcileError::Ledger(l) => l.into(),
            ratchet_core::cost::ReconcileError::Billing(p) => p.into(),
        })?;
        self.kv.set_json(&k::reconcile(id), &report).await?;
        if report.status == CostReconcileStatus::Reconciled {
            self.emit(EngineEvent::Spend {
                optimization_id: oid(id),
                total_usd: self.ledger.total_spend(&oid(id)).await?,
            });
            self.emit(EngineEvent::Notice {
                optimization_id: oid(id),
                level: NoticeLevel::Info,
                text: format!(
                    "spend reconciled with Cost Explorer: {}${}",
                    if report.written_usd >= 0.0 { "+" } else { "-" },
                    to_fixed(report.written_usd.abs(), 4)
                ),
            });
        }
        Ok(report)
    }

    // ------------------------------------------------------------------ teardown

    pub async fn teardown(&self, id: &str) -> EngineResult<()> {
        assert_valid_id("optimization id", id)?;
        self.ready().await;
        let mut opt = self.require_opt(id).await?;
        if opt.status == OptimizationStatus::Completed {
            return Ok(());
        }
        if matches!(
            opt.status,
            OptimizationStatus::Running
                | OptimizationStatus::Baselining
                | OptimizationStatus::Stopping
        ) {
            self.stop(id, Some("teardown")).await?;
            opt = self.require_opt(id).await?;
        }
        let _ = opt;
        let compute = self.ports.compute.clone();
        let mut errors: Vec<String> = Vec::new();
        let mut report = TeardownReport {
            optimization_id: id.to_owned(),
            at: self.clock.now(),
            deleted_secrets_for: vec![],
            deleted_identities_for: vec![],
            terminated_instance_ids: vec![],
            orphans: vec![],
            errors: vec![],
            cost_reconcile: None,
        };

        let local = self.agents_where(id).await == AgentsWhere::Local;
        for agent in self.ledger.list_agents(&oid(id)).await? {
            if !local {
                let c = compute.clone();
                let (o, a) = (oid(id), agent.id.clone());
                if attempt(
                    &mut errors,
                    self,
                    &format!("deleteSecrets {}", agent.id),
                    || async move { c.delete_secrets(&o, &a).await.map_err(Into::into) },
                )
                .await
                {
                    report.deleted_secrets_for.push(agent.id.to_string());
                }
            }
            if let Some(identity) = self
                .kv
                .get_json::<AgentIdentity>(&k::identity(agent.id.as_str()))
                .await?
            {
                self.revoke_agent(&agent).await;
                let c = compute.clone();
                if attempt(
                    &mut errors,
                    self,
                    &format!("deleteAgentIdentity {}", agent.id),
                    || async move { c.delete_agent_identity(&identity).await.map_err(Into::into) },
                )
                .await
                {
                    report.deleted_identities_for.push(agent.id.to_string());
                    self.kv.delete(&k::identity(agent.id.as_str())).await?;
                }
            }
        }
        // Local worktrees and the local clone (the commits the agents pushed are on the remote).
        let runners = self.runners_for(id).await;
        {
            let o = oid(id);
            attempt(
                &mut errors,
                self,
                "release agent workspaces",
                || async move { runners.release(&o).await.map_err(Into::into) },
            )
            .await;
        }
        match self.terminate_all(id).await {
            Ok(ids) => report.terminated_instance_ids = ids,
            Err(err) => {
                errors.push(format!("terminate leftovers: {}", err.message));
                self.report_error(&err, "teardown terminate leftovers");
            }
        }
        self.safe_meter(id).await;
        match compute.list_tagged(Some(&oid(id))).await {
            Ok(list) => {
                report.orphans = list
                    .into_iter()
                    .filter(|i| i.state != InstanceState::Terminated)
                    .map(|i| TeardownOrphan {
                        id: i.id,
                        role: serde_json::to_value(i.role)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        state: serde_json::to_value(i.state)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                    })
                    .collect();
            }
            Err(err) => {
                errors.push(format!("verify listTagged: {err}"));
                self.report_error(&err, "teardown verify listTagged");
            }
        }

        // Best effort and never a teardown failure: Cost Explorer lags up to 24 h, so `pending` is the usual answer here.
        if self.ports.costs.is_some() {
            match self.reconcile_costs(id).await {
                Ok(r) => {
                    report.cost_reconcile = Some(TeardownCostReconcile {
                        status: serde_json::to_value(r.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        reason: r.reason.clone(),
                        written_usd: r.written_usd,
                    });
                }
                Err(err) => {
                    report.cost_reconcile = Some(TeardownCostReconcile {
                        status: "error".into(),
                        reason: Some(err.message.clone()),
                        written_usd: 0.0,
                    });
                    self.report_error(&err, "teardown reconcileCosts");
                }
            }
        }

        report.errors = errors.clone();
        self.kv.set_json(&k::teardown(id), &report).await?;
        if let Err(e) = self
            .ports
            .artifacts
            .put_str(
                &format!("engine/{id}/teardown.json"),
                &serde_json::to_string_pretty(&report)?,
            )
            .await
        {
            self.report_error(e, "artifacts.put teardown");
        }
        if !report.orphans.is_empty() {
            return Err(EngineError::new(
                EngineErrorCode::Orphans,
                format!(
                    "teardown left {} orphaned instance(s): {}",
                    report.orphans.len(),
                    report
                        .orphans
                        .iter()
                        .map(|o| o.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_details(serde_json::to_value(&report)?));
        }
        if !errors.is_empty() {
            return Err(EngineError::new(
                EngineErrorCode::TeardownIncomplete,
                format!("teardown incomplete: {}", errors.join("; ")),
            )
            .with_details(serde_json::to_value(&report)?));
        }
        self.set_status(id, OptimizationStatus::Completed).await
    }
}

fn base36(mut n: u64) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}
