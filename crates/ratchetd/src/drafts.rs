//! The environment-draft flow behind the `environment-drafts` routes, shared by the simulation and the real service.
//!
//! The work is `ratchet-env`: an [`EnvironmentDrafter`] (the real setup-agent drafter, or the deterministic mock) does
//! the drafting, validating, costing and approving. What lives here is the API side: the agent's turn runs in the
//! background (a `POST` answers at once with the draft in status `drafting`), every status change and every progress
//! line of the agent becomes an `environment-draft` event, and a draft id finds the drafter that made it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use ratchet_env::{
    ApproveEnvironmentOptions, DraftEnvironmentInput, EnvError, EnvErrorKind,
    EnvironmentDraftPhase as Phase, EnvironmentDraftRecord, EnvironmentDraftStatus as Status,
    EnvironmentDrafter,
};

use crate::api::*;
use crate::service::{ServiceError, ServiceResult};

/// Where drafters come from: one for the simulation, one per repository for the real engine.
#[async_trait]
pub trait DrafterSource: Send + Sync {
    async fn drafter_for(&self, repo: &str) -> ServiceResult<Arc<dyn EnvironmentDrafter>>;
}

/// A source that always answers with the same drafter.
pub struct FixedDrafter(pub Arc<dyn EnvironmentDrafter>);

#[async_trait]
impl DrafterSource for FixedDrafter {
    async fn drafter_for(&self, _repo: &str) -> ServiceResult<Arc<dyn EnvironmentDrafter>> {
        Ok(self.0.clone())
    }
}

pub type Emit = Arc<dyn Fn(ApiEvent) + Send + Sync>;

pub struct EnvDrafts {
    source: Arc<dyn DrafterSource>,
    emit: Emit,
    /// Draft id -> the drafter that holds it, in the order the drafts were made.
    routes: Mutex<Vec<(String, Arc<dyn EnvironmentDrafter>)>>,
}

impl EnvDrafts {
    pub fn new(source: Arc<dyn DrafterSource>, emit: Emit) -> Self {
        Self {
            source,
            emit,
            routes: Mutex::default(),
        }
    }

    fn routes(&self) -> MutexGuard<'_, Vec<(String, Arc<dyn EnvironmentDrafter>)>> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn drafter_of(&self, id: &str) -> ServiceResult<Arc<dyn EnvironmentDrafter>> {
        self.routes()
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| ServiceError::not_found("environment draft", id))
    }

    fn record(
        &self,
        id: &str,
    ) -> ServiceResult<(Arc<dyn EnvironmentDrafter>, EnvironmentDraftRecord)> {
        let d = self.drafter_of(id)?;
        let rec = d
            .get(id)
            .ok_or_else(|| ServiceError::not_found("environment draft", id))?;
        Ok((d, rec))
    }

    fn announce(&self, rec: &EnvironmentDraftRecord) {
        (self.emit)(ApiEvent::EnvironmentDraft {
            draft_id: rec.id.clone(),
            status: status_vm(rec.status),
            revision: rec.revision,
            phase: None,
            text: None,
        });
    }

    /// Run the agent's turn in the background and announce how it ended.
    fn spawn(&self, drafter: Arc<dyn EnvironmentDrafter>, id: String, revision: bool) {
        let emit = self.emit.clone();
        tokio::spawn(async move {
            let done = if revision {
                drafter.run_revise(&id).await
            } else {
                drafter.run_draft(&id).await
            };
            if let Ok(rec) = done {
                emit(ApiEvent::EnvironmentDraft {
                    draft_id: rec.id.clone(),
                    status: status_vm(rec.status),
                    revision: rec.revision,
                    phase: None,
                    text: None,
                });
            }
        });
    }

    /// `POST /v1/environment-drafts`: records the draft (`drafting`) and starts the setup agent.
    pub async fn create(
        &self,
        input: CreateEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        let goal = input.goal.trim();
        if goal.is_empty() || goal.chars().count() > 2000 {
            return Err(invalid("goal", "the goal must be 1 to 2000 characters"));
        }
        if input.repo.trim().is_empty() {
            return Err(invalid("repo", "name the repository (owner/name)"));
        }
        if let Some(t) = input.time_limit_seconds
            && !(t.is_finite() && t > 0.0)
        {
            return Err(invalid(
                "timeLimitSeconds",
                "the time limit must be a positive number of seconds",
            ));
        }
        let drafter = self.source.drafter_for(input.repo.trim()).await?;
        let mut di = DraftEnvironmentInput::new(input.repo.trim(), goal, input.agent_connector);
        di.base_ref = input.base_ref.filter(|b| !b.trim().is_empty());
        di.aws_profile = input.aws_profile.filter(|p| !p.trim().is_empty());
        di.region = input.region.filter(|r| !r.trim().is_empty());
        di.time_limit_seconds = input.time_limit_seconds;
        di.config_path = input.config_path.filter(|p| !p.trim().is_empty());
        let emit = self.emit.clone();
        let progress_drafter = drafter.clone();
        di.on_progress = Some(Arc::new(move |p| {
            let revision = progress_drafter.get(&p.draft_id).map_or(0, |r| r.revision);
            emit(ApiEvent::EnvironmentDraft {
                draft_id: p.draft_id,
                status: if p.phase == Phase::Failed {
                    EnvironmentDraftStatus::Failed
                } else {
                    EnvironmentDraftStatus::Drafting
                },
                revision,
                phase: Some(phase_vm(p.phase)),
                text: Some(p.text),
            });
        }));
        let rec = drafter.begin_draft(di).map_err(env_error)?;
        self.routes().push((rec.id.clone(), drafter.clone()));
        self.announce(&rec);
        self.spawn(drafter, rec.id.clone(), false);
        Ok(draft_vm(&rec))
    }

    pub fn get(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        self.record(id).map(|(_, r)| draft_vm(&r))
    }

    /// Every draft, newest first.
    pub fn list(&self) -> Vec<EnvironmentDraftVm> {
        let mut seen: Vec<EnvironmentDraftRecord> = Vec::new();
        let routes: Vec<_> = self.routes().clone();
        for (id, d) in routes {
            if let Some(r) = d.get(&id) {
                seen.push(r);
            }
        }
        seen.reverse(); // made later = newer; the stable sort keeps that among drafts of the same millisecond
        seen.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        seen.iter().map(draft_vm).collect()
    }

    pub fn discard(&self, id: &str) -> ServiceResult<()> {
        let d = self.drafter_of(id)?;
        d.discard(id).map_err(env_error)?;
        self.routes().retain(|(i, _)| i != id);
        Ok(())
    }

    pub async fn revise(
        &self,
        id: &str,
        input: ReviseEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        let message = input.message.trim();
        if message.is_empty() || message.chars().count() > 2000 {
            return Err(invalid(
                "message",
                "say what to change, in 1 to 2000 characters",
            ));
        }
        let d = self.drafter_of(id)?;
        let rec = d.begin_revise(id, message).map_err(env_error)?;
        self.announce(&rec);
        self.spawn(d, id.to_owned(), true);
        Ok(draft_vm(&rec))
    }

    pub async fn approve(
        &self,
        id: &str,
        input: ApproveEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        let d = self.drafter_of(id)?;
        let was = d.get(id).map(|r| r.status);
        let mode = input.mode.map(|m| match m {
            EnvironmentApprovalMode::Commit => ratchet_env::EnvironmentApprovalMode::Commit,
            EnvironmentApprovalMode::Pr => ratchet_env::EnvironmentApprovalMode::Pr,
        });
        let rec = d
            .approve(id, ApproveEnvironmentOptions { mode })
            .await
            .map_err(env_error)?;
        if was != Some(Status::Approved) {
            self.announce(&rec);
        }
        Ok(draft_vm(&rec))
    }

    /// 409 unless the draft exists and is approved: what `environmentDraftId` of `POST /v1/optimizations` needs.
    pub fn require_approved(&self, id: &str) -> ServiceResult<()> {
        match self.record(id)?.1.status {
            Status::Approved => Ok(()),
            _ => Err(ServiceError::NotReady(
                "approve the environment before starting the optimization".into(),
            )),
        }
    }

    /// Drafts waiting for the user's approval (ready, with every check passed).
    pub fn waiting(&self) -> Vec<EnvironmentDraftVm> {
        let mut v: Vec<_> = self
            .list()
            .into_iter()
            .filter(|d| {
                d.status == EnvironmentDraftStatus::Ready
                    && d.draft
                        .as_ref()
                        .is_some_and(|p| !p.checks.is_empty() && p.checks.iter().all(|c| c.ok))
            })
            .collect();
        v.sort_by_key(|d| d.created_at);
        v
    }
}

fn invalid(path: &str, message: &str) -> ServiceError {
    ServiceError::InvalidFields {
        message: message.into(),
        fields: vec![FieldError::new(path, message)],
    }
}

/// An environment error as the API shows it.
pub fn env_error(e: EnvError) -> ServiceError {
    match e.kind {
        EnvErrorKind::InvalidInput => ServiceError::Invalid(e.message),
        EnvErrorKind::InvalidConfig => ServiceError::Invalid(e.message),
        EnvErrorKind::InvalidState => ServiceError::Conflict(e.message),
        EnvErrorKind::NotFound => ServiceError::NotFound(e.message),
        EnvErrorKind::Unavailable => ServiceError::Unsupported(e.message),
        EnvErrorKind::Aborted | EnvErrorKind::Port => ServiceError::Upstream(e.message),
    }
}

fn status_vm(s: Status) -> EnvironmentDraftStatus {
    match s {
        Status::Drafting => EnvironmentDraftStatus::Drafting,
        Status::Ready => EnvironmentDraftStatus::Ready,
        Status::Failed => EnvironmentDraftStatus::Failed,
        Status::Approved => EnvironmentDraftStatus::Approved,
    }
}

fn phase_vm(p: Phase) -> EnvironmentDraftPhase {
    match p {
        Phase::Reading => EnvironmentDraftPhase::Reading,
        Phase::Drafting => EnvironmentDraftPhase::Drafting,
        Phase::Validating => EnvironmentDraftPhase::Validating,
        Phase::Done => EnvironmentDraftPhase::Done,
        Phase::Failed => EnvironmentDraftPhase::Failed,
    }
}

fn role_vm(r: ratchet_env::EnvironmentRole) -> EnvironmentRole {
    use ratchet_env::EnvironmentRole as R;
    match r {
        R::App => EnvironmentRole::App,
        R::Database => EnvironmentRole::Database,
        R::Cache => EnvironmentRole::Cache,
        R::Load => EnvironmentRole::Load,
        R::Evaluator => EnvironmentRole::Evaluator,
        R::Other => EnvironmentRole::Other,
    }
}

/// A record as the wire shows it.
pub fn draft_vm(r: &EnvironmentDraftRecord) -> EnvironmentDraftVm {
    EnvironmentDraftVm {
        id: r.id.clone(),
        repo: r.repo.clone(),
        base_ref: r.base_ref.clone(),
        goal: r.goal.clone(),
        aws_profile: r.aws_profile.clone(),
        region: r.region.clone(),
        status: status_vm(r.status),
        revision: r.revision,
        draft: r.draft.as_ref().map(|d| EnvironmentProposalVm {
            scope: d.scope,
            reason: d.reason.clone(),
            resources: d
                .resources
                .iter()
                .map(|x| EnvironmentResourceVm {
                    name: x.name.clone(),
                    role: role_vm(x.role),
                    machine: x.machine.clone(),
                    detail: x.detail.clone(),
                    usd_per_hour: x.usd_per_hour,
                })
                .collect(),
            files: d
                .files
                .iter()
                .map(|f| EnvironmentFileVm {
                    path: f.path.clone(),
                    content: f.content.clone(),
                })
                .collect(),
            checks: d
                .checks
                .iter()
                .map(|c| EnvironmentCheckVm {
                    id: c.id.clone(),
                    label: c.label.clone(),
                    ok: c.ok,
                    detail: c.detail.clone(),
                })
                .collect(),
            cost: EnvironmentCostVm {
                usd_per_hour: d.cost.usd_per_hour,
                startup_minutes: d.cost.startup_minutes,
                estimate_for_run_usd: d.cost.estimate_for_run_usd,
            },
            agent: d.agent,
            duration_s: d.duration_s,
            usd_spent: d.usd_spent,
        }),
        messages: r
            .messages
            .iter()
            .map(|m| EnvironmentMessageVm {
                from: match m.from {
                    ratchet_env::MessageFrom::User => EnvironmentMessageFrom::User,
                    ratchet_env::MessageFrom::Agent => EnvironmentMessageFrom::Agent,
                },
                text: m.text.clone(),
                at: m.at,
            })
            .collect(),
        error: r.error.clone(),
        approval: r.approval.as_ref().map(|a| EnvironmentApprovalVm {
            mode: match a.mode {
                ratchet_env::EnvironmentApprovalMode::Commit => EnvironmentApprovalMode::Commit,
                ratchet_env::EnvironmentApprovalMode::Pr => EnvironmentApprovalMode::Pr,
            },
            sha: a.sha.clone(),
            branch: a.branch.clone(),
            pr_url: a.pr_url.clone(),
            files: a.files.clone(),
            locked: a.locked,
            at: a.at,
        }),
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}
