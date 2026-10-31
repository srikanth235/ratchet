//! The real drafter (`drafter.ts`): a setup agent (the user's agent connector, local) reads the repo and the goal and
//! writes only the environment (`environment:` in ratchet.yaml and files under `infra/ratchet/`); Ratchet validates
//! it against the sandbox contract ([`validate_environment`]), runs `tofu plan` in the user's account
//! ([`EnvironmentChecker`]), and prices it. `revise` resumes the same agent session with the user's change request;
//! `approve` writes the files to the protected ref (commit or pull request).
//!
//! The drafts live in memory: they are a conversation, not state worth surviving a restart (the approved files live
//! in the repo).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::config::{DEFAULT_ENVIRONMENT_INFRA_DIR, parse_config};
use ratchet_core::enums::{AgentKind, EnvironmentScope};
use ratchet_core::ports::{
    AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, CancellationToken, Clock,
    GitHost, PortError, PriceBook,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::approve::{CommitDraftOptions, commit_draft};
use crate::brief::{SetupBriefInput, build_revise_brief, build_setup_brief, parse_agent_answer};
use crate::config_yaml::{
    EnvironmentConfigPatch, apply_environment_to_config_yaml, read_environment_scope,
};
use crate::cost::{CostEstimateOptions, estimate_environment_cost_live};
use crate::error::{EnvError, EnvErrorKind, EnvResult};
use crate::tree::{TreeError, read_environment_tree};
use crate::types::*;
use crate::validate::{ValidateEnvironmentInput, validate_environment};

/// The user's connector: the adapter of their own Claude Code / Codex login, run locally. Errors when the agent
/// cannot run here.
pub type AdapterFactory =
    Arc<dyn Fn(AgentKind) -> Result<Arc<dyn AgentAdapter>, PortError> + Send + Sync>;
/// The git host of a repository (one host, or a factory when the app drives several).
pub type GitFactory = Arc<dyn Fn(&str) -> Arc<dyn GitHost> + Send + Sync>;

/// A [`GitFactory`] that always answers with `host`.
pub fn fixed_git(host: Arc<dyn GitHost>) -> GitFactory {
    Arc::new(move |_| host.clone())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenPricing {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

/// Used when the agent reports tokens but no cost (the engine's default).
pub const DEFAULT_TOKEN_PRICING: TokenPricing = TokenPricing {
    input_usd_per_mtok: 3.0,
    output_usd_per_mtok: 15.0,
};

type ToolProgress = Arc<dyn Fn(&str) + Send + Sync>;
pub type ErrorHook = Arc<dyn Fn(&EnvError, &str) + Send + Sync>;

#[derive(Clone)]
pub struct AgentEnvironmentDrafterOptions {
    pub git: GitFactory,
    pub adapter_for: AdapterFactory,
    pub clock: Arc<dyn Clock>,
    /// Checks the draft's OpenTofu against the user's AWS account.
    pub checker: Option<Arc<dyn EnvironmentChecker>>,
    /// Live prices (the AWS price book); the static table is the fallback.
    pub prices: Option<Arc<dyn PriceBook>>,
    /// Where the setup agent's checkout lives (default: the OS temp dir).
    pub scratch_dir: Option<PathBuf>,
    /// Region when neither the input nor ratchet.yaml names one. Default `us-east-1`.
    pub default_region: Option<String>,
    pub token_pricing: Option<TokenPricing>,
    pub model: Option<String>,
    pub max_turns: Option<u32>,
    /// The agent is aborted after this long (default 15 min).
    pub agent_timeout: Option<Duration>,
    pub new_id: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub on_error: Option<ErrorHook>,
}

impl AgentEnvironmentDrafterOptions {
    pub fn new(git: GitFactory, adapter_for: AdapterFactory, clock: Arc<dyn Clock>) -> Self {
        Self {
            git,
            adapter_for,
            clock,
            checker: None,
            prices: None,
            scratch_dir: None,
            default_region: None,
            token_pricing: None,
            model: None,
            max_turns: None,
            agent_timeout: None,
            new_id: None,
            on_error: None,
        }
    }
}

/// What `begin_draft` keeps for `run_draft`.
struct Pending {
    input: DraftEnvironmentInput,
}

struct Work {
    dir: TempDir,
    session: AgentSession,
    adapter: Arc<dyn AgentAdapter>,
    kind: AgentKind,
    original_config_yaml: String,
    config_path: String,
    time_limit_seconds: f64,
    aws_profile: Option<String>,
    on_progress: Option<ProgressFn>,
}

#[derive(Default)]
struct State {
    records: HashMap<String, EnvironmentDraftRecord>,
    /// Ids in the order the drafts were begun (`list` keeps that order among drafts created at the same time).
    order: Vec<String>,
    pending: HashMap<String, Pending>,
    work: HashMap<String, Work>,
    /// Status a revision falls back to when it fails.
    revising: HashMap<String, EnvironmentDraftStatus>,
    busy: HashSet<String>,
}

pub struct AgentEnvironmentDrafter {
    o: AgentEnvironmentDrafterOptions,
    state: Mutex<State>,
}

const SKIP_DIRS: [&str; 8] = [
    ".git",
    "node_modules",
    "dist",
    "build",
    ".next",
    "target",
    ".venv",
    "__pycache__",
];

/// path -> content hash of the text files of a tree (what the agent was allowed to change is read separately).
fn digest(dir: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, String>) {
        if out.len() > 20_000 {
            return;
        }
        let here = if rel.is_empty() {
            dir.to_path_buf()
        } else {
            dir.join(rel)
        };
        let Ok(rd) = std::fs::read_dir(&here) else {
            return;
        };
        let mut names: Vec<String> = rd
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let Ok(meta) = std::fs::symlink_metadata(dir.join(&r)) else {
                continue;
            };
            if meta.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    walk(dir, &r, out);
                }
            } else if meta.is_file()
                && meta.len() <= 1_000_000
                && let Ok(bytes) = std::fs::read(dir.join(&r))
            {
                let h = Sha256::digest(&bytes);
                out.insert(r, h.iter().map(|b| format!("{b:02x}")).collect());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

/// JavaScript `Math.round` for non-negative values.
pub(crate) fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn random_id() -> String {
    format!("env-{:08x}", rand::random::<u32>())
}

struct AgentRun {
    session: AgentSession,
    text: String,
    usd: f64,
}

impl AgentEnvironmentDrafter {
    pub fn new(o: AgentEnvironmentDrafterOptions) -> Self {
        Self {
            o,
            state: Mutex::new(State::default()),
        }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("environment drafter state")
    }

    fn git_for(&self, repo: &str) -> Arc<dyn GitHost> {
        (self.o.git)(repo)
    }

    fn fail_hook(&self, err: &EnvError, context: &str) {
        if let Some(h) = &self.o.on_error {
            h(err, context);
        }
    }

    fn new_id(&self) -> String {
        self.o.new_id.as_ref().map_or_else(random_id, |f| f())
    }

    fn require(&self, draft_id: &str) -> EnvResult<EnvironmentDraftRecord> {
        self.st()
            .records
            .get(draft_id)
            .cloned()
            .ok_or_else(|| EnvError::not_found(format!("environment draft {draft_id} not found")))
    }

    fn update<R>(
        &self,
        draft_id: &str,
        f: impl FnOnce(&mut EnvironmentDraftRecord) -> R,
    ) -> Option<R> {
        self.st().records.get_mut(draft_id).map(f)
    }

    /// Run one agent turn (`start`, or `resume` when `session` is given) in `workdir`.
    async fn run_agent(
        &self,
        adapter: &Arc<dyn AgentAdapter>,
        session: Option<AgentSession>,
        workdir: &Path,
        prompt: &str,
        progress: Option<ToolProgress>,
    ) -> EnvResult<AgentRun> {
        #[derive(Default)]
        struct Acc {
            text: Vec<String>,
            usd: f64,
        }
        let acc = Arc::new(Mutex::new(Acc::default()));
        let pricing = self.o.token_pricing.unwrap_or(DEFAULT_TOKEN_PRICING);
        let sink = {
            let acc = acc.clone();
            move |e: AgentEvent| {
                let mut a = acc.lock().expect("agent events");
                if e.event_type == AgentEventType::Message
                    && let Some(t) = e.text.as_ref().filter(|t| !t.is_empty())
                {
                    a.text.push(t.clone());
                }
                if let Some(u) = e.usage {
                    a.usd += u.cost_usd.unwrap_or(
                        (u.input_tokens as f64 * pricing.input_usd_per_mtok
                            + u.output_tokens as f64 * pricing.output_usd_per_mtok)
                            / 1e6,
                    );
                }
                if e.event_type == AgentEventType::Tool
                    && let (Some(t), Some(p)) =
                        (e.text.as_ref().filter(|t| !t.is_empty()), &progress)
                {
                    p(t);
                }
            }
        };
        let spec = AgentSpec {
            workdir: workdir.to_string_lossy().into_owned(),
            prompt: prompt.to_owned(),
            model: self.o.model.clone(),
            env: None,
            max_turns: self.o.max_turns,
        };
        let timeout = self.o.agent_timeout.unwrap_or(Duration::from_secs(15 * 60));
        let cancel = CancellationToken::new();
        let timer = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                cancel.cancel();
            })
        };
        let result = match session {
            Some(mut s) => adapter
                .resume(&mut s, &spec, prompt, &sink, Some(&cancel))
                .await
                .map(|()| s),
            None => adapter.start(&spec, &sink, Some(&cancel)).await,
        };
        timer.abort();
        let session = match result {
            Ok(s) => s,
            Err(err) => {
                if cancel.is_cancelled() {
                    return Err(EnvError::aborted(format!(
                        "the setup agent did not finish within {} min",
                        js_round(timeout.as_secs_f64() / 60.0)
                    )));
                }
                return Err(err.into());
            }
        };
        let a = acc.lock().expect("agent events");
        Ok(AgentRun {
            session,
            text: a.text.join("\n"),
            usd: a.usd,
        })
    }

    /// Read what the agent wrote, validate, price: the draft the UI shows.
    async fn assemble(
        &self,
        rec: &EnvironmentDraftRecord,
        w: &AssembleCtx<'_>,
        run: &AgentRun,
        started: i64,
        before: &BTreeMap<String, String>,
        previous: Option<&EnvironmentDraft>,
    ) -> EnvResult<(EnvironmentDraft, Vec<String>)> {
        let answer = parse_agent_answer(&run.text);
        let mut config_yaml = tokio::fs::read_to_string(w.dir.join(w.config_path))
            .await
            .unwrap_or_else(|_| w.original_config_yaml.to_owned());
        let declared = read_environment_scope(&config_yaml);
        let scope: EnvironmentScope = answer
            .as_ref()
            .map(|a| a.scope)
            .or(declared)
            .or(previous.map(|p| p.scope))
            .ok_or_else(|| {
                EnvError::invalid_config(
                    "the setup agent did not say which scope the environment needs (none, single or full)",
                )
            })?;
        // Ratchet records the decision itself when the agent forgot to, or wrote a different one.
        if declared != Some(scope) {
            config_yaml = apply_environment_to_config_yaml(
                &config_yaml,
                &EnvironmentConfigPatch::scope(scope),
            )
            .map_err(EnvError::invalid_config)?;
        }

        let infra_dir = DEFAULT_ENVIRONMENT_INFRA_DIR
            .split('/')
            .fold(w.dir.to_path_buf(), |p, s| p.join(s));
        let infra = {
            let dir = infra_dir.clone();
            tokio::task::spawn_blocking(move || read_environment_tree(&dir))
                .await
                .map_err(|e| EnvError::new(EnvErrorKind::Port, e.to_string()))?
        };
        let infra = match infra {
            Ok(f) => f,
            Err(TreeError::NotFound) => vec![],
            Err(TreeError::Other(m)) => {
                return Err(EnvError::invalid_config(format!(
                    "cannot read {DEFAULT_ENVIRONMENT_INFRA_DIR}/: {m}"
                )));
            }
        };
        let mut files: Vec<EnvironmentFile> =
            vec![EnvironmentFile::new(w.config_path, config_yaml)];
        files.extend(infra.iter().map(|f| {
            EnvironmentFile::new(
                format!("{DEFAULT_ENVIRONMENT_INFRA_DIR}/{}", f.path),
                f.content.clone(),
            )
        }));
        let mut vin = ValidateEnvironmentInput::new(scope, files.clone());
        vin.original_config_yaml = Some(w.original_config_yaml.to_owned());
        vin.config_path = Some(w.config_path.to_owned());
        vin.time_limit_seconds = Some(w.time_limit_seconds);
        let v = validate_environment(&vin);
        let mut resources = v.resources.clone();
        let mut cost = v.cost;
        if let Some(prices) = &self.o.prices
            && !v.machines.is_empty()
        {
            let live = estimate_environment_cost_live(
                &v.machines,
                &CostEstimateOptions {
                    time_limit_seconds: w.time_limit_seconds,
                    prices: None,
                },
                prices.as_ref(),
            )
            .await;
            resources = live.resources;
            cost = live.cost;
        }

        let mut checks: Vec<EnvironmentCheck> = v
            .checks
            .iter()
            .filter(|c| c.id != "plan")
            .cloned()
            .collect();
        let plan = |ok: bool, detail: String| EnvironmentCheck {
            id: "plan".into(),
            label: "Plans cleanly in your AWS account".into(),
            ok,
            detail,
        };
        let has_tf = infra.iter().any(|f| f.path.ends_with(".tf"));
        if !has_tf {
            checks.push(plan(
                true,
                "no OpenTofu to plan: the standard sandbox is used".into(),
            ));
        } else if !v.ok {
            checks.push(plan(false, "not run: fix the failed checks first".into()));
        } else if let Some(checker) = &self.o.checker {
            let prefix = format!("{DEFAULT_ENVIRONMENT_INFRA_DIR}/");
            let outcome = checker
                .plan(&EnvironmentPlanInput {
                    scope,
                    files: files
                        .iter()
                        .filter(|f| f.path.starts_with(&prefix))
                        .cloned()
                        .collect(),
                    region: rec.region.clone(),
                    aws_profile: w.aws_profile.map(str::to_owned),
                })
                .await;
            checks.push(match outcome {
                Ok(r) => plan(r.ok, r.detail),
                Err(err) => plan(false, format!("could not run tofu: {err}")),
            });
        } else {
            checks.push(plan(
                false,
                "not run: connect AWS (Settings) so Ratchet can plan this in your account".into(),
            ));
        }

        let dir = w.dir.to_path_buf();
        let after = tokio::task::spawn_blocking(move || digest(&dir))
            .await
            .map_err(|e| EnvError::new(EnvErrorKind::Port, e.to_string()))?;
        let infra_prefix = format!("{DEFAULT_ENVIRONMENT_INFRA_DIR}/");
        let allowed = |p: &str| p == w.config_path || p.starts_with(&infra_prefix);
        let strays: Vec<String> = before
            .keys()
            .chain(after.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|p| !allowed(p) && before.get(*p) != after.get(*p))
            .cloned()
            .collect();

        files.sort_by(|a, b| {
            let a_cfg = a.path == w.config_path;
            let b_cfg = b.path == w.config_path;
            b_cfg.cmp(&a_cfg).then_with(|| utf16_cmp(&a.path, &b.path))
        });
        let now = self.o.clock.now();
        let draft = EnvironmentDraft {
            scope,
            reason: answer
                .map(|a| a.reason)
                .filter(|r| !r.is_empty())
                .or_else(|| previous.map(|p| p.reason.clone()).filter(|r| !r.is_empty()))
                .unwrap_or_default(),
            resources,
            files,
            checks,
            cost,
            agent: w.kind,
            duration_s: js_round(((now - started) as f64 / 1000.0).max(0.0)) as u64
                + previous.map_or(0, |p| p.duration_s),
            usd_spent: js_round((run.usd + previous.map_or(0.0, |p| p.usd_spent)) * 10_000.0)
                / 10_000.0,
        };
        Ok((draft, strays))
    }

    fn release(&self, draft_id: &str) {
        let w = self.st().work.remove(draft_id);
        drop(w); // removes the scratch checkout
    }

    fn progress(
        &self,
        draft_id: &str,
        cb: &Option<ProgressFn>,
        phase: EnvironmentDraftPhase,
        text: impl Into<String>,
    ) {
        if let Some(cb) = cb {
            cb(EnvironmentDraftProgress {
                draft_id: draft_id.to_owned(),
                phase,
                text: text.into(),
            });
        }
    }
}

/// JavaScript string `<` ordering (UTF-16 code units).
fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

struct AssembleCtx<'a> {
    dir: &'a Path,
    kind: AgentKind,
    original_config_yaml: &'a str,
    config_path: &'a str,
    time_limit_seconds: f64,
    aws_profile: Option<&'a str>,
}

fn agent_note(reason: &str, strays: &[String], lead: &str) -> String {
    let stray = if strays.is_empty() {
        String::new()
    } else {
        format!(
            "({lead} {}: those changes are not part of the draft and are never committed.)",
            strays.join(", ")
        )
    };
    [reason.to_owned(), stray]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl EnvironmentDrafter for AgentEnvironmentDrafter {
    fn begin_draft(&self, input: DraftEnvironmentInput) -> EnvResult<EnvironmentDraftRecord> {
        if input.repo.is_empty() {
            return Err(EnvError::invalid_input("repo is required"));
        }
        if input.goal.trim().is_empty() {
            return Err(EnvError::invalid_input("goal is required"));
        }
        let now = self.o.clock.now();
        let id = input.id.clone().unwrap_or_else(|| self.new_id());
        let rec = EnvironmentDraftRecord {
            id: id.clone(),
            repo: input.repo.clone(),
            base_ref: input.base_ref.clone().unwrap_or_else(|| "main".into()),
            goal: input.goal.trim().to_owned(),
            aws_profile: input.aws_profile.clone().filter(|p| !p.is_empty()),
            region: input
                .region
                .clone()
                .or_else(|| self.o.default_region.clone())
                .unwrap_or_else(|| "us-east-1".into()),
            status: EnvironmentDraftStatus::Drafting,
            revision: 0,
            draft: None,
            messages: vec![EnvironmentDraftMessage {
                from: MessageFrom::User,
                text: input.goal.trim().to_owned(),
                at: now,
            }],
            error: None,
            approval: None,
            created_at: now,
            updated_at: now,
        };
        let mut st = self.st();
        if st.records.contains_key(&id) {
            return Err(EnvError::invalid_input(format!(
                "environment draft {id} already exists"
            )));
        }
        st.records.insert(id.clone(), rec.clone());
        st.order.push(id.clone());
        st.pending.insert(id, Pending { input });
        Ok(rec)
    }

    async fn run_draft(&self, draft_id: &str) -> EnvResult<EnvironmentDraftRecord> {
        self.require(draft_id)?;
        let Some(Pending { input }) = self.st().pending.remove(draft_id) else {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} was already run"
            )));
        };
        let id = draft_id.to_owned();
        let cb = input.on_progress.clone();
        let progress = |phase, text: &str| self.progress(&id, &cb, phase, text);
        let outcome: EnvResult<()> = async {
            progress(EnvironmentDraftPhase::Reading, "reading the repository");
            let git = self.git_for(&input.repo);
            let base_ref = input.base_ref.clone().unwrap_or_else(|| "main".into());
            let base_sha = git.resolve_ref(&base_ref).await?;
            let scratch: TempDir = match &self.o.scratch_dir {
                Some(d) => {
                    tokio::fs::create_dir_all(d).await?;
                    tempfile::Builder::new().prefix("ratchet-env-draft-").tempdir_in(d)?
                }
                None => tempfile::Builder::new().prefix("ratchet-env-draft-").tempdir()?,
            };
            git.checkout(&base_sha, scratch.path()).await?;
            let config_path = input.config_path.clone().unwrap_or_else(|| "ratchet.yaml".into());
            let original_config_yaml = tokio::fs::read_to_string(scratch.path().join(&config_path))
                .await
                .map_err(|_| {
                    EnvError::invalid_config(format!(
                        "{config_path} not found on {base_ref}: add a ratchet.yaml (or use a template) before drafting an environment"
                    ))
                })?;
            let parsed = parse_config(&original_config_yaml);
            let hardware = parsed.as_ref().ok().map(|c| c.evaluator.hardware.clone());
            if let Ok(c) = &parsed
                && let Some(r) = c.environment.as_ref().and_then(|e| e.region.clone())
                && input.region.is_none()
            {
                self.update(&id, |rec| rec.region = r);
            }
            let adapter = (self.o.adapter_for)(input.agent_connector)?;
            let pre = adapter.preflight().await?;
            if !pre.ok {
                return Err(EnvError::unavailable(format!(
                    "{} cannot draft here: {}",
                    input.agent_connector,
                    pre.problems.join("; ")
                )));
            }

            let prompt = build_setup_brief(&SetupBriefInput {
                goal: input.goal.clone(),
                hardware,
                time_limit_seconds: input.time_limit_seconds.filter(|t| *t > 0.0),
            });
            progress(
                EnvironmentDraftPhase::Drafting,
                &format!("{} is designing the environment", input.agent_connector),
            );
            let started = self.o.clock.now();
            let before = digest(scratch.path());
            let tool_progress: ToolProgress = {
                let cb = cb.clone();
                let id = id.clone();
                Arc::new(move |t: &str| {
                    if let Some(cb) = &cb {
                        cb(EnvironmentDraftProgress {
                            draft_id: id.clone(),
                            phase: EnvironmentDraftPhase::Drafting,
                            text: t.to_owned(),
                        });
                    }
                })
            };
            let run = self
                .run_agent(&adapter, None, scratch.path(), &prompt, Some(tool_progress))
                .await?;
            let time_limit_seconds = input.time_limit_seconds.unwrap_or(2.0 * 3600.0);
            progress(
                EnvironmentDraftPhase::Validating,
                "checking the draft against the sandbox contract",
            );
            let rec = self.require(&id)?;
            let (draft, strays) = self
                .assemble(
                    &rec,
                    &AssembleCtx {
                        dir: scratch.path(),
                        kind: input.agent_connector,
                        original_config_yaml: &original_config_yaml,
                        config_path: &config_path,
                        time_limit_seconds,
                        aws_profile: input.aws_profile.as_deref(),
                    },
                    &run,
                    started,
                    &before,
                    None,
                )
                .await?;
            let note = agent_note(&draft.reason, &strays, "The agent also changed");
            let at = self.o.clock.now();
            self.st().work.insert(
                id.clone(),
                Work {
                    dir: scratch,
                    session: run.session,
                    adapter,
                    kind: input.agent_connector,
                    original_config_yaml,
                    config_path,
                    time_limit_seconds,
                    aws_profile: input.aws_profile.clone(),
                    on_progress: cb.clone(),
                },
            );
            self.update(&id, |rec| {
                rec.draft = Some(draft);
                rec.revision = 1;
                rec.status = EnvironmentDraftStatus::Ready;
                rec.messages.push(EnvironmentDraftMessage {
                    from: MessageFrom::Agent,
                    text: note,
                    at,
                });
            });
            Ok(())
        }
        .await;
        if let Err(err) = outcome {
            self.update(&id, |rec| {
                rec.status = EnvironmentDraftStatus::Failed;
                rec.error = Some(err.message.clone());
            });
            self.fail_hook(&err, &format!("draft {id}"));
            self.release(&id);
            progress(EnvironmentDraftPhase::Failed, &err.message);
        }
        let now = self.o.clock.now();
        let rec = self
            .update(&id, |rec| {
                rec.updated_at = now;
                rec.clone()
            })
            .ok_or_else(|| EnvError::not_found(format!("environment draft {id} not found")))?;
        if rec.status == EnvironmentDraftStatus::Ready {
            progress(EnvironmentDraftPhase::Done, "the draft is ready");
        }
        Ok(rec)
    }

    fn begin_revise(&self, draft_id: &str, message: &str) -> EnvResult<EnvironmentDraftRecord> {
        let mut st = self.st();
        let rec = st.records.get(draft_id).cloned().ok_or_else(|| {
            EnvError::not_found(format!("environment draft {draft_id} not found"))
        })?;
        if message.trim().is_empty() {
            return Err(EnvError::invalid_input("say what to change"));
        }
        if rec.status == EnvironmentDraftStatus::Approved {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} is approved: the environment is locked; start a new draft to change it"
            )));
        }
        if rec.status == EnvironmentDraftStatus::Drafting || st.busy.contains(draft_id) {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} is being drafted"
            )));
        }
        if !st.work.contains_key(draft_id) || rec.status == EnvironmentDraftStatus::Failed {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} has no agent session to continue ({}); draft again",
                rec.error.as_deref().unwrap_or("it failed")
            )));
        }
        st.busy.insert(draft_id.to_owned());
        st.revising.insert(draft_id.to_owned(), rec.status);
        let now = self.o.clock.now();
        let rec = st.records.get_mut(draft_id).expect("checked above");
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
        let message = rec
            .messages
            .iter()
            .rev()
            .find(|m| m.from == MessageFrom::User)
            .map(|m| m.text.clone())
            .unwrap_or_default();
        let (previous_status, ctx) = {
            let st = self.st();
            let Some(previous) = st.revising.get(draft_id).copied() else {
                return Err(EnvError::invalid_state(format!(
                    "draft {draft_id} has no revision to run"
                )));
            };
            let w = st.work.get(draft_id).ok_or_else(|| {
                EnvError::invalid_state(format!(
                    "draft {draft_id} has no agent session to continue"
                ))
            })?;
            (
                previous,
                (
                    w.adapter.clone(),
                    w.session.clone(),
                    w.dir.path().to_path_buf(),
                    w.kind,
                    w.original_config_yaml.clone(),
                    w.config_path.clone(),
                    w.time_limit_seconds,
                    w.aws_profile.clone(),
                    w.on_progress.clone(),
                ),
            )
        };
        let (adapter, session, dir, kind, original, config_path, time_limit, aws_profile, cb) = ctx;
        let _ = cb;
        let outcome: EnvResult<()> = async {
            let started = self.o.clock.now();
            let d = dir.clone();
            let before = tokio::task::spawn_blocking(move || digest(&d))
                .await
                .map_err(|e| EnvError::new(EnvErrorKind::Port, e.to_string()))?;
            let run = self
                .run_agent(
                    &adapter,
                    Some(session),
                    &dir,
                    &build_revise_brief(&message),
                    None,
                )
                .await?;
            if let Some(w) = self.st().work.get_mut(draft_id) {
                w.session = run.session.clone();
            }
            let (draft, strays) = self
                .assemble(
                    &rec,
                    &AssembleCtx {
                        dir: &dir,
                        kind,
                        original_config_yaml: &original,
                        config_path: &config_path,
                        time_limit_seconds: time_limit,
                        aws_profile: aws_profile.as_deref(),
                    },
                    &run,
                    started,
                    &before,
                    rec.draft.as_ref(),
                )
                .await?;
            let note = agent_note(&draft.reason, &strays, "Also changed");
            let at = self.o.clock.now();
            self.update(draft_id, |r| {
                r.draft = Some(draft);
                r.revision += 1;
                r.status = EnvironmentDraftStatus::Ready;
                r.messages.push(EnvironmentDraftMessage {
                    from: MessageFrom::Agent,
                    text: note,
                    at,
                });
            });
            Ok(())
        }
        .await;
        if let Err(err) = outcome {
            let at = self.o.clock.now();
            self.update(draft_id, |r| {
                r.status = previous_status; // the earlier draft is still there
                r.error = Some(err.message.clone());
                r.messages.push(EnvironmentDraftMessage {
                    from: MessageFrom::Agent,
                    text: format!("I could not make that change: {}", err.message),
                    at,
                });
            });
            self.fail_hook(&err, &format!("revise {draft_id}"));
        }
        {
            let mut st = self.st();
            st.busy.remove(draft_id);
            st.revising.remove(draft_id);
        }
        let now = self.o.clock.now();
        self.update(draft_id, |r| {
            r.updated_at = now;
            r.clone()
        })
        .ok_or_else(|| EnvError::not_found(format!("environment draft {draft_id} not found")))
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
        if rec.status != EnvironmentDraftStatus::Ready {
            return Err(EnvError::invalid_state(format!(
                "draft {draft_id} is {}; only a ready draft can be approved",
                rec.status
            )));
        }
        {
            let mut st = self.st();
            if !st.busy.insert(draft_id.to_owned()) {
                return Err(EnvError::invalid_state(format!(
                    "draft {draft_id} is being approved"
                )));
            }
        }
        let (config_path, expected) = {
            let st = self.st();
            match st.work.get(draft_id) {
                Some(w) => (w.config_path.clone(), Some(w.original_config_yaml.clone())),
                None => ("ratchet.yaml".to_owned(), None),
            }
        };
        let clock = self.o.clock.clone();
        let result = commit_draft(
            self.git_for(&rec.repo).as_ref(),
            &rec,
            CommitDraftOptions {
                mode: options.mode,
                expected_config_yaml: expected,
                config_path,
                scratch_dir: self.o.scratch_dir.clone(),
                now: Arc::new(move || clock.now()),
                branch_suffix: rec.id.strip_prefix("env-").unwrap_or(&rec.id).to_owned(),
            },
        )
        .await;
        self.st().busy.remove(draft_id);
        let approval = result?;
        let now = self.o.clock.now();
        let rec = self
            .update(draft_id, |r| {
                r.approval = Some(approval);
                r.status = EnvironmentDraftStatus::Approved;
                r.updated_at = now;
                r.clone()
            })
            .ok_or_else(|| {
                EnvError::not_found(format!("environment draft {draft_id} not found"))
            })?;
        self.release(draft_id);
        Ok(rec)
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
        v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
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
        st.work.remove(draft_id);
        st.pending.remove(draft_id);
        st.records.remove(draft_id);
        st.order.retain(|i| i != draft_id);
        Ok(())
    }
}
