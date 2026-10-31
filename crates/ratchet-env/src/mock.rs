//! A deterministic stand-in for the real drafter (`mock.ts`): no agent, no AWS, no network. Drafts are the example
//! environments of [`sample_environment`], validated by the real validator and priced by the real cost estimator,
//! so everything the UI shows (the rows, the checks, the cost) is computed, not canned. Used by simulation, demos
//! and the tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use async_trait::async_trait;
use ratchet_core::enums::{AgentKind, EnvironmentScope};
use ratchet_core::ports::Clock;
use regex::Regex;

use crate::approve::{CommitDraftOptions, commit_draft};
use crate::drafter::{GitFactory, js_round};
use crate::error::{EnvError, EnvResult};
use crate::samples::{
    MOCK_CONFIG_YAML, SampleOptions, sample_effort, sample_environment, sample_reason,
};
use crate::types::*;
use crate::validate::{ValidateEnvironmentInput, validate_environment};

/// The scope every draft gets.
#[derive(Clone)]
pub enum ScopeChoice {
    Fixed(EnvironmentScope),
    /// A function of the request.
    With(Arc<dyn Fn(&DraftEnvironmentInput) -> EnvironmentScope + Send + Sync>),
}

#[derive(Clone, Default)]
pub struct MockEnvironmentDrafterOptions {
    pub clock: Option<Arc<dyn Clock>>,
    /// Default: chosen from the goal's words ([`scope_from_goal`]).
    pub scope: Option<ScopeChoice>,
    /// The repo's ratchet.yaml the drafts are made on (default: a minimal http-latency config).
    pub config_yaml: Option<String>,
    /// Pretend drafting takes this long (a real `clock.sleep`), so a UI can show its progress. Default 0.
    pub delay_ms: Option<u64>,
    /// With a git host, `approve` really commits (directly or as a PR) through `commit_files`, exactly like the real
    /// drafter; without one the approval is synthetic (a fake sha / PR url), for demos that have no repository.
    pub git: Option<GitFactory>,
    pub new_id: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

#[derive(Default)]
struct State {
    records: HashMap<String, EnvironmentDraftRecord>,
    order: Vec<String>,
    /// The ratchet.yaml each draft was made on.
    originals: HashMap<String, String>,
    pending: HashMap<String, DraftEnvironmentInput>,
}

pub struct MockEnvironmentDrafter {
    o: MockEnvironmentDrafterOptions,
    state: Mutex<State>,
    seq: AtomicU64,
}

static FULL_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(p9\d|latency|throughput|checkout|database|postgres|redis|queries|requests?/s|rps|load test|api|server|stack)",
    )
    .expect("static regex")
});
static NONE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(unit test|tests?(?-u:\b)|lint|type ?check|compile|typecheck|refactor|dead code|coverage)",
    )
    .expect("static regex")
});
static SMALLER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(smaller|simpler|cheaper|less|single|one machine|fewer)").expect("static regex")
});
static BIGGER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(bigger|full stack|more realistic|add a database|postgres|redis|load generator)")
        .expect("static regex")
});
static RDS_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u:\b)(rds|aurora|managed database)(?-u:\b)").expect("static regex")
});

/// The scope a goal's words suggest (the mock's default chooser): databases and load -> full, tests -> none,
/// otherwise one machine.
pub fn scope_from_goal(goal: &str) -> EnvironmentScope {
    let g = goal.to_lowercase();
    if FULL_WORDS.is_match(&g) {
        EnvironmentScope::Full
    } else if NONE_WORDS.is_match(&g) {
        EnvironmentScope::None
    } else {
        EnvironmentScope::Single
    }
}

impl MockEnvironmentDrafter {
    pub fn new(o: MockEnvironmentDrafterOptions) -> Self {
        Self {
            o,
            state: Mutex::new(State::default()),
            seq: AtomicU64::new(0),
        }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("mock environment drafter state")
    }

    fn now(&self) -> i64 {
        self.o.clock.as_ref().map_or_else(
            || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64)
            },
            |c| c.now(),
        )
    }

    fn pick_scope(&self, input: &DraftEnvironmentInput) -> EnvironmentScope {
        match &self.o.scope {
            Some(ScopeChoice::Fixed(s)) => *s,
            Some(ScopeChoice::With(f)) => f(input),
            None => scope_from_goal(&input.goal),
        }
    }

    async fn delay(&self) {
        if let (Some(ms), Some(clock)) = (self.o.delay_ms.filter(|m| *m > 0), &self.o.clock) {
            let _ = clock.sleep(ms, None).await;
        }
    }

    fn require(&self, id: &str) -> EnvResult<EnvironmentDraftRecord> {
        self.st()
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| EnvError::not_found(format!("environment draft {id} not found")))
    }

    /// The repo's ratchet.yaml: the option, else the one on the base ref when there is a host, else a minimal config.
    async fn original_config(&self, rec: &EnvironmentDraftRecord, config_path: &str) -> String {
        if let Some(y) = &self.o.config_yaml {
            return y.clone();
        }
        let Some(git) = self.o.git.as_ref().map(|f| f(&rec.repo)) else {
            return MOCK_CONFIG_YAML.to_owned();
        };
        let Ok(dir) = tempfile::Builder::new()
            .prefix("ratchet-mock-env-")
            .tempdir()
        else {
            return MOCK_CONFIG_YAML.to_owned();
        };
        if git.checkout(&rec.base_ref, dir.path()).await.is_err() {
            return MOCK_CONFIG_YAML.to_owned();
        }
        tokio::fs::read_to_string(dir.path().join(config_path))
            .await
            .unwrap_or_else(|_| MOCK_CONFIG_YAML.to_owned())
    }

    /// Build the draft for `scope` through the real validator and estimator.
    fn make(
        &self,
        rec_id: &str,
        scope: EnvironmentScope,
        reason: &str,
        agent: AgentKind,
        time_limit_seconds: Option<f64>,
        previous: Option<&EnvironmentDraft>,
    ) -> EnvironmentDraft {
        let original = self
            .st()
            .originals
            .get(rec_id)
            .cloned()
            .or_else(|| self.o.config_yaml.clone())
            .unwrap_or_else(|| MOCK_CONFIG_YAML.to_owned());
        let sample = sample_environment(
            scope,
            &SampleOptions {
                original_config_yaml: Some(original.clone()),
                config_path: None,
                reason: Some(reason.to_owned()),
            },
        );
        let mut vin = ValidateEnvironmentInput::new(scope, sample.files.clone());
        vin.original_config_yaml = Some(original);
        vin.time_limit_seconds = Some(time_limit_seconds.unwrap_or(2.0 * 3600.0));
        let v = validate_environment(&vin);
        let plan = EnvironmentCheck {
            id: "plan".into(),
            label: "Plans cleanly in your AWS account".into(),
            ok: true,
            detail: if sample.files.iter().any(|f| f.path.ends_with(".tf")) {
                "simulated: no AWS account is touched in demo mode".into()
            } else {
                "no OpenTofu to plan: the standard sandbox is used".into()
            },
        };
        let (duration_s, usd_spent) = sample_effort(scope);
        let mut checks = v.checks.clone();
        checks.push(plan);
        EnvironmentDraft {
            scope,
            reason: reason.to_owned(),
            resources: v.resources,
            files: sample.files,
            checks,
            cost: v.cost,
            agent,
            duration_s: duration_s + previous.map_or(0, |p| p.duration_s),
            usd_spent: js_round((usd_spent + previous.map_or(0.0, |p| p.usd_spent)) * 100.0)
                / 100.0,
        }
    }
}

#[async_trait]
impl EnvironmentDrafter for MockEnvironmentDrafter {
    fn begin_draft(&self, input: DraftEnvironmentInput) -> EnvResult<EnvironmentDraftRecord> {
        if input.repo.is_empty() {
            return Err(EnvError::invalid_input("repo is required"));
        }
        if input.goal.trim().is_empty() {
            return Err(EnvError::invalid_input("goal is required"));
        }
        let t = self.now();
        let id = input.id.clone().unwrap_or_else(|| {
            self.o
                .new_id
                .as_ref()
                .map_or_else(|| format!("env-{:08x}", rand::random::<u32>()), |f| f())
        });
        let rec = EnvironmentDraftRecord {
            id: id.clone(),
            repo: input.repo.clone(),
            base_ref: input.base_ref.clone().unwrap_or_else(|| "main".into()),
            goal: input.goal.trim().to_owned(),
            aws_profile: input.aws_profile.clone().filter(|p| !p.is_empty()),
            region: input.region.clone().unwrap_or_else(|| "us-east-1".into()),
            status: EnvironmentDraftStatus::Drafting,
            revision: 0,
            draft: None,
            messages: vec![EnvironmentDraftMessage {
                from: MessageFrom::User,
                text: input.goal.trim().to_owned(),
                at: t,
            }],
            error: None,
            approval: None,
            created_at: t,
            updated_at: t,
        };
        let mut st = self.st();
        if st.records.contains_key(&id) {
            return Err(EnvError::invalid_input(format!(
                "environment draft {id} already exists"
            )));
        }
        st.records.insert(id.clone(), rec.clone());
        st.order.push(id.clone());
        st.pending.insert(id, input);
        Ok(rec)
    }

    async fn run_draft(&self, draft_id: &str) -> EnvResult<EnvironmentDraftRecord> {
        let rec = self.require(draft_id)?;
        let Some(input) = self.st().pending.remove(draft_id) else {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} was already run"
            )));
        };
        let progress = |phase, text: &str| {
            if let Some(cb) = &input.on_progress {
                cb(EnvironmentDraftProgress {
                    draft_id: draft_id.to_owned(),
                    phase,
                    text: text.to_owned(),
                });
            }
        };
        progress(EnvironmentDraftPhase::Reading, "reading the repository");
        progress(
            EnvironmentDraftPhase::Drafting,
            &format!("{} is designing the environment", input.agent_connector),
        );
        self.delay().await;
        progress(
            EnvironmentDraftPhase::Validating,
            "checking the draft against the sandbox contract",
        );
        let scope = self.pick_scope(&input);
        let original = self
            .original_config(&rec, input.config_path.as_deref().unwrap_or("ratchet.yaml"))
            .await;
        self.st().originals.insert(draft_id.to_owned(), original);
        let draft = self.make(
            draft_id,
            scope,
            sample_reason(scope),
            input.agent_connector,
            input.time_limit_seconds,
            None,
        );
        let now = self.now();
        let out = {
            let mut st = self.st();
            let rec = st.records.get_mut(draft_id).ok_or_else(|| {
                EnvError::not_found(format!("environment draft {draft_id} not found"))
            })?;
            rec.messages.push(EnvironmentDraftMessage {
                from: MessageFrom::Agent,
                text: draft.reason.clone(),
                at: now,
            });
            rec.draft = Some(draft);
            rec.revision = 1;
            rec.status = EnvironmentDraftStatus::Ready;
            rec.updated_at = now;
            rec.clone()
        };
        progress(EnvironmentDraftPhase::Done, "the draft is ready");
        Ok(out)
    }

    fn begin_revise(&self, draft_id: &str, message: &str) -> EnvResult<EnvironmentDraftRecord> {
        let rec = self.require(draft_id)?;
        if message.trim().is_empty() {
            return Err(EnvError::invalid_input("say what to change"));
        }
        if rec.status == EnvironmentDraftStatus::Approved {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} is approved: the environment is locked; start a new draft to change it"
            )));
        }
        if rec.draft.is_none() || rec.status == EnvironmentDraftStatus::Drafting {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} has nothing to revise"
            )));
        }
        let now = self.now();
        let mut st = self.st();
        let rec = st.records.get_mut(draft_id).expect("required above");
        rec.messages.push(EnvironmentDraftMessage {
            from: MessageFrom::User,
            text: message.trim().to_owned(),
            at: now,
        });
        rec.status = EnvironmentDraftStatus::Drafting;
        rec.updated_at = now;
        Ok(rec.clone())
    }

    async fn run_revise(&self, draft_id: &str) -> EnvResult<EnvironmentDraftRecord> {
        let rec = self.require(draft_id)?;
        let Some(current) = rec.draft.clone() else {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} has nothing to revise"
            )));
        };
        let message = rec
            .messages
            .iter()
            .rev()
            .find(|m| m.from == MessageFrom::User)
            .map(|m| m.text.clone())
            .unwrap_or_default();
        self.delay().await;
        let m = message.to_lowercase();
        let mut scope = current.scope;
        let reason: String;
        if SMALLER.is_match(&m) {
            scope = if current.scope == EnvironmentScope::Full {
                EnvironmentScope::Single
            } else {
                EnvironmentScope::None
            };
            reason = if scope == current.scope {
                format!("Already as small as it gets ({scope}).")
            } else {
                format!("Smaller setup: scope {scope}. {}", sample_reason(scope))
            };
        } else if BIGGER.is_match(&m) && !m.contains("rds") {
            scope = if current.scope == EnvironmentScope::None {
                EnvironmentScope::Single
            } else {
                EnvironmentScope::Full
            };
            reason = if scope == current.scope {
                format!("Already the full stack ({scope}).")
            } else {
                format!("Larger setup: scope {scope}. {}", sample_reason(scope))
            };
        } else if RDS_WORD.is_match(&m) {
            reason = format!(
                "{} I kept Postgres in a container: a managed database (RDS) needs subnets in two zones, and the isolated sandbox has one private subnet with no route out.",
                sample_reason(scope)
            );
        } else {
            reason = format!(
                "{} Noted: \"{}\". I kept the setup as it is because it already covers that.",
                current.reason,
                message.trim()
            );
        }
        let draft = self.make(
            draft_id,
            scope,
            &reason,
            current.agent,
            None,
            Some(&current),
        );
        let now = self.now();
        let mut st = self.st();
        let rec = st.records.get_mut(draft_id).ok_or_else(|| {
            EnvError::not_found(format!("environment draft {draft_id} not found"))
        })?;
        rec.draft = Some(draft);
        rec.revision += 1;
        rec.status = EnvironmentDraftStatus::Ready;
        rec.messages.push(EnvironmentDraftMessage {
            from: MessageFrom::Agent,
            text: reason,
            at: now,
        });
        rec.updated_at = now;
        Ok(rec.clone())
    }

    async fn approve(
        &self,
        draft_id: &str,
        options: ApproveEnvironmentOptions,
    ) -> EnvResult<EnvironmentDraftRecord> {
        let rec = self.require(draft_id)?;
        if rec.status == EnvironmentDraftStatus::Approved {
            return Ok(rec);
        }
        let Some(draft) = rec
            .draft
            .as_ref()
            .filter(|_| rec.status == EnvironmentDraftStatus::Ready)
        else {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} is {}; only a ready draft can be approved",
                rec.status
            )));
        };
        let approval = if let Some(git) = self.o.git.as_ref().map(|f| f(&rec.repo)) {
            let clock = self.o.clock.clone();
            commit_draft(
                git.as_ref(),
                &rec,
                CommitDraftOptions {
                    mode: options.mode,
                    expected_config_yaml: None,
                    config_path: "ratchet.yaml".into(),
                    scratch_dir: None,
                    now: Arc::new(move || {
                        clock.as_ref().map_or_else(
                            || {
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_or(0, |d| d.as_millis() as i64)
                            },
                            |c| c.now(),
                        )
                    }),
                    branch_suffix: draft_id.strip_prefix("env-").unwrap_or(draft_id).to_owned(),
                },
            )
            .await?
        } else {
            let failed: Vec<&str> = draft
                .checks
                .iter()
                .filter(|c| !c.ok)
                .map(|c| c.label.as_str())
                .collect();
            if !failed.is_empty() {
                return Err(EnvError::invalid_state(format!(
                    "draft {draft_id} cannot be approved: these checks did not pass: {}",
                    failed.join(", ")
                )));
            }
            let mode = options.mode.unwrap_or_default();
            let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
            let word: String = draft_id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(8)
                .collect();
            EnvironmentApproval {
                mode,
                sha: format!("mock{n:04}{word}"),
                branch: (mode == EnvironmentApprovalMode::Pr).then(|| {
                    format!(
                        "ratchet/environment-{}",
                        draft_id.strip_prefix("env-").unwrap_or(draft_id)
                    )
                }),
                pr_url: (mode == EnvironmentApprovalMode::Pr)
                    .then(|| format!("https://example.test/ratchet/pull/{n}")),
                files: draft.files.iter().map(|f| f.path.clone()).collect(),
                locked: true,
                at: self.now(),
            }
        };
        let now = self.now();
        let mut st = self.st();
        let rec = st.records.get_mut(draft_id).ok_or_else(|| {
            EnvError::not_found(format!("environment draft {draft_id} not found"))
        })?;
        rec.approval = Some(approval);
        rec.status = EnvironmentDraftStatus::Approved;
        rec.updated_at = now;
        Ok(rec.clone())
    }

    fn get(&self, draft_id: &str) -> Option<EnvironmentDraftRecord> {
        self.st().records.get(draft_id).cloned()
    }

    fn list(&self) -> Vec<EnvironmentDraftRecord> {
        let st = self.st();
        let mut v: Vec<_> = st
            .order
            .iter()
            .filter_map(|id| st.records.get(id).cloned())
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    fn discard(&self, draft_id: &str) -> EnvResult<()> {
        let mut st = self.st();
        if st
            .records
            .get(draft_id)
            .is_some_and(|r| r.status == EnvironmentDraftStatus::Approved)
        {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} is approved and cannot be discarded"
            )));
        }
        st.records.remove(draft_id);
        st.pending.remove(draft_id);
        st.order.retain(|i| i != draft_id);
        Ok(())
    }
}
