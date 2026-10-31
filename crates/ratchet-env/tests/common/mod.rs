//! Test doubles shared by the drafter and mock tests: a scripted setup agent, a fake clock and a fake git host.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::enums::{AgentKind, EnvironmentScope};
use ratchet_core::ports::mock::ManualClock;
use ratchet_core::ports::{
    AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, CandidateCommit, Clock, CommitFilesInput, EventSink, FileDiff, GitHost,
    PortError, PortResult, Preflight, PullRequestInput, PullRequestRef,
};
use ratchet_env::{
    AgentAnswer, DraftEnvironmentInput, EnvironmentFile, MOCK_CONFIG_YAML, SampleOptions,
    format_agent_answer, sample_environment,
};

pub const REPO: &str = "https://example.test/acme/repo.git";

pub type Turn = Box<dyn Fn(&Path) -> Result<String, String> + Send + Sync>;

/// A setup agent that follows a script: each turn writes files into the workdir and answers like the brief asks.
pub struct ScriptedAgent {
    turns: Vec<Turn>,
    usd: f64,
    turn: AtomicUsize,
    pub prompts: Mutex<Vec<String>>,
    pub resumed: Mutex<Vec<AgentSession>>,
    pub preflight_problems: Mutex<Vec<String>>,
}

impl ScriptedAgent {
    pub fn new(turns: Vec<Turn>) -> Arc<Self> {
        Self::with_usd(turns, 0.5)
    }

    pub fn with_usd(turns: Vec<Turn>, usd: f64) -> Arc<Self> {
        Arc::new(Self {
            turns,
            usd,
            turn: AtomicUsize::new(0),
            prompts: Mutex::default(),
            resumed: Mutex::default(),
            preflight_problems: Mutex::default(),
        })
    }

    fn run(&self, spec: &AgentSpec, events: &EventSink<'_>) -> PortResult<()> {
        self.prompts.lock().unwrap().push(spec.prompt.clone());
        let i = self
            .turn
            .fetch_add(1, Ordering::SeqCst)
            .min(self.turns.len() - 1);
        let text = (self.turns[i])(Path::new(&spec.workdir)).map_err(PortError::agent_failed)?;
        let mut tool = AgentEvent::new(0, AgentEventType::Tool);
        tool.text = Some("writing infra/ratchet/main.tf".into());
        events(tool);
        let mut message = AgentEvent::new(0, AgentEventType::Message);
        message.text = Some(text);
        events(message);
        let mut usage = AgentEvent::new(0, AgentEventType::Usage);
        usage.usage = Some(AgentUsage {
            input_tokens: 1000,
            output_tokens: 1000,
            cost_usd: Some(self.usd),
        });
        events(usage);
        Ok(())
    }
}

#[async_trait]
impl AgentAdapter for ScriptedAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }
    async fn preflight(&self) -> PortResult<Preflight> {
        let problems = self.preflight_problems.lock().unwrap().clone();
        Ok(Preflight {
            ok: problems.is_empty(),
            problems,
        })
    }
    async fn start(
        &self,
        spec: &AgentSpec,
        events: &EventSink<'_>,
        _cancel: Option<&CancellationToken>,
    ) -> PortResult<AgentSession> {
        self.run(spec, events)?;
        Ok(AgentSession {
            id: "sess-1".into(),
            kind: AgentKind::ClaudeCode,
        })
    }
    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        _message: &str,
        events: &EventSink<'_>,
        _cancel: Option<&CancellationToken>,
    ) -> PortResult<()> {
        self.resumed.lock().unwrap().push(session.clone());
        self.run(spec, events)
    }
}

pub fn put(dir: &Path, files: &[EnvironmentFile]) {
    for f in files {
        let p = dir.join(&f.path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, &f.content).unwrap();
    }
}

pub fn sample_files(scope: EnvironmentScope, original: &str) -> Vec<EnvironmentFile> {
    sample_environment(
        scope,
        &SampleOptions {
            original_config_yaml: Some(original.to_owned()),
            ..Default::default()
        },
    )
    .files
}

/// The agent writes the sample environment of `scope` and answers.
pub fn writes(scope: EnvironmentScope) -> Turn {
    writes_with(scope, &format!("reason for {scope}"), |_| {})
}

pub fn writes_with(
    scope: EnvironmentScope,
    reason: &str,
    extra: impl Fn(&Path) + Send + Sync + 'static,
) -> Turn {
    let reason = reason.to_owned();
    Box::new(move |dir| {
        put(dir, &sample_files(scope, MOCK_CONFIG_YAML));
        extra(dir);
        Ok(format!(
            "Done.\n{}",
            format_agent_answer(&AgentAnswer {
                scope,
                reason: reason.clone()
            })
        ))
    })
}

/// A fake git host (the TS `FakeGit`): `checkout` writes the project files, `commit_files` records what it got.
pub struct FakeGit {
    pub files: Mutex<BTreeMap<String, String>>,
    pub committed: Mutex<Vec<(CommitFilesInput, String)>>,
    pub prs: Mutex<Vec<PullRequestInput>>,
    pub existing_branches: Mutex<Vec<String>>,
    pub can_commit: bool,
}

impl FakeGit {
    pub fn new(files: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::new(
                files
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            ),
            committed: Mutex::default(),
            prs: Mutex::default(),
            existing_branches: Mutex::default(),
            can_commit: true,
        })
    }

    pub fn without_commit(files: &[(&str, &str)]) -> Arc<Self> {
        let g = Self::new(files);
        Arc::new(Self {
            files: Mutex::new(g.files.lock().unwrap().clone()),
            committed: Mutex::default(),
            prs: Mutex::default(),
            existing_branches: Mutex::default(),
            can_commit: false,
        })
    }
}

#[async_trait]
impl GitHost for FakeGit {
    async fn create_branch(&self, _from_ref: &str, _branch: &str) -> PortResult<()> {
        Ok(())
    }
    async fn list_candidates(
        &self,
        _b: &str,
        _s: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>> {
        Ok(vec![])
    }
    async fn changed_files(&self, _b: &str, _s: &str) -> PortResult<Vec<String>> {
        Ok(vec![])
    }
    async fn diff(&self, _b: &str, _s: &str) -> PortResult<Vec<FileDiff>> {
        Ok(vec![])
    }
    async fn checkout(&self, _sha: &str, dir: &Path) -> PortResult<()> {
        for (p, c) in self.files.lock().unwrap().iter() {
            let full = dir.join(p);
            std::fs::create_dir_all(full.parent().unwrap())?;
            std::fs::write(full, c)?;
        }
        Ok(())
    }
    async fn open_pull_request(&self, input: &PullRequestInput) -> PortResult<PullRequestRef> {
        let mut prs = self.prs.lock().unwrap();
        prs.push(input.clone());
        Ok(PullRequestRef {
            url: format!("https://example.test/pr/{}", prs.len()),
        })
    }
    async fn resolve_ref(&self, git_ref: &str) -> PortResult<String> {
        Ok(format!("{git_ref}-sha"))
    }
    fn remote_url(&self) -> String {
        REPO.into()
    }
    async fn commit_files(
        &self,
        input: &CommitFilesInput,
    ) -> PortResult<ratchet_core::ports::CommitRef> {
        if !self.can_commit {
            return Err(PortError::Unsupported("commit_files"));
        }
        let mut existing = self.existing_branches.lock().unwrap();
        if existing.contains(&input.branch) {
            return Err(PortError::failed(format!(
                "branch {} already exists",
                input.branch
            )));
        }
        existing.push(input.branch.clone());
        let mut committed = self.committed.lock().unwrap();
        let sha = format!("files-{}", committed.len() + 1);
        committed.push((input.clone(), sha.clone()));
        Ok(ratchet_core::ports::CommitRef { sha })
    }
}

pub fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(1_700_000_000_000))
}

pub fn ids() -> Arc<dyn Fn() -> String + Send + Sync> {
    let n = Arc::new(AtomicUsize::new(0));
    Arc::new(move || format!("env-{:04}", n.fetch_add(1, Ordering::SeqCst) + 1))
}

pub fn input(goal: &str) -> DraftEnvironmentInput {
    DraftEnvironmentInput::new(REPO, goal, AgentKind::ClaudeCode)
}

pub fn dyn_clock(c: &Arc<ManualClock>) -> Arc<dyn Clock> {
    c.clone()
}
