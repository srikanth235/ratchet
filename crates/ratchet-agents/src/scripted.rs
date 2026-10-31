//! Scripted agents for simulation and demos: plays a fixed plan of edits, patches and messages, one attempt per turn, with no model.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::clock::SystemClock;
use crate::domain::{
    AgentAdapter, AgentError, AgentEvent, AgentEventType, AgentKind, AgentSession, AgentSpec,
    AgentUsage, Clock, EventCallback, Preflight,
};
use crate::git::{GitCall, SharedGit, exec_git};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkipWhen {
    Present,
    Absent,
}

/// Skip a step when a marker is already present (or absent) in a file: patches stay idempotent after a rebase onto the best.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepCondition {
    pub file: String,
    pub contains: String,
    pub skip_when: SkipWhen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum ScriptStep {
    Message {
        text: String,
    },
    Tool {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Map<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u64>,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },
    /// `diff` is unified-diff text relative to the workdir root, applied with `git apply`.
    Patch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        diff: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip: Option<StepCondition>,
    },
    /// Replace `find` (must occur exactly once) with `replace`.
    Edit {
        path: String,
        find: String,
        replace: String,
    },
    Write {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    /// Run git in the workdir, e.g. `reset --hard HEAD~1` to drop a rejected attempt.
    Git {
        args: Vec<String>,
    },
    Sleep {
        ms: u64,
    },
    /// Crash the turn with an error (simulates a CLI failure).
    Fail {
        message: String,
    },
}

/// One agent turn: `start` plays attempt 0, each `resume` plays the next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptAttempt {
    pub name: String,
    pub steps: Vec<ScriptStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentScript {
    pub name: String,
    pub attempts: Vec<ScriptAttempt>,
}

impl AgentScript {
    pub fn new(name: impl Into<String>, attempts: Vec<Vec<ScriptStep>>) -> Self {
        Self {
            name: name.into(),
            attempts: attempts
                .into_iter()
                .enumerate()
                .map(|(i, steps)| ScriptAttempt {
                    name: format!("a{i}"),
                    steps,
                })
                .collect(),
        }
    }
}

/// Sonnet-class list prices, to give scripted usage a believable `costUsd`.
pub fn price_usage(input_tokens: u64, output_tokens: u64) -> f64 {
    (((input_tokens as f64 * 3.0 + output_tokens as f64 * 15.0) / 1e6) * 10_000.0 + 0.5).floor()
        / 10_000.0
}

/// Called with every attempt played and its index.
pub type PlayCallback = Arc<dyn Fn(&ScriptAttempt, usize) + Send + Sync>;

#[derive(Default, Clone)]
pub struct ScriptedAgentOptions {
    pub clock: Option<Arc<dyn Clock>>,
    pub git: Option<SharedGit>,
    /// Called for every attempt played; handy in tests.
    pub on_play: Option<PlayCallback>,
}

pub struct ScriptedAgent {
    pub script: AgentScript,
    clock: Arc<dyn Clock>,
    git: SharedGit,
    options: ScriptedAgentOptions,
    /// Messages received through `resume`, in order (verdict summaries, user messages).
    received: Mutex<Vec<String>>,
    session_ids: Mutex<HashSet<String>>,
    /// Next attempt to play. Shared by all sessions: a restarted agent keeps working through the same plan.
    cursor: Mutex<usize>,
    sessions: AtomicU64,
}

/// Lexical `path.resolve(base, path)`: no file system access.
fn resolve(base: &Path, path: &str) -> PathBuf {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

static PATCH_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

impl ScriptedAgent {
    pub fn new(script: AgentScript) -> Self {
        Self::with_options(script, ScriptedAgentOptions::default())
    }

    pub fn with_options(script: AgentScript, options: ScriptedAgentOptions) -> Self {
        Self {
            script,
            clock: options
                .clock
                .clone()
                .unwrap_or_else(|| Arc::new(SystemClock)),
            git: options.git.clone().unwrap_or_else(exec_git),
            options,
            received: Mutex::new(Vec::new()),
            session_ids: Mutex::new(HashSet::new()),
            cursor: Mutex::new(0),
            sessions: AtomicU64::new(0),
        }
    }

    /// Messages received through `resume`, in order.
    pub fn received(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
    }

    fn emit(
        &self,
        on_event: EventCallback<'_>,
        kind: AgentEventType,
        text: Option<String>,
        data: Option<Value>,
    ) {
        let mut e = AgentEvent::new(self.clock.now(), kind);
        e.text = text;
        if let Some(d) = data {
            e = e.with_data(d);
        }
        on_event(e);
    }

    fn inside(&self, workdir: &Path, path: &str) -> Result<PathBuf, AgentError> {
        let root = resolve(workdir, ".");
        let full = resolve(workdir, path);
        if full != root && !full.starts_with(&root) {
            return Err(AgentError::failed(format!(
                "path escapes the workdir: {path}"
            )));
        }
        Ok(full)
    }

    async fn skip_applies(&self, skip: &StepCondition, workdir: &Path) -> Result<bool, AgentError> {
        let text = tokio::fs::read_to_string(self.inside(workdir, &skip.file)?)
            .await
            .unwrap_or_default();
        Ok(text.contains(&skip.contains) == (skip.skip_when == SkipWhen::Present))
    }

    async fn is_redundant(&self, steps: &[ScriptStep], workdir: &Path) -> Result<bool, AgentError> {
        let patches: Vec<&Option<StepCondition>> = steps
            .iter()
            .filter_map(|s| {
                if let ScriptStep::Patch { skip, .. } = s {
                    Some(skip)
                } else {
                    None
                }
            })
            .collect();
        if patches.is_empty()
            || steps.iter().any(|s| {
                matches!(
                    s,
                    ScriptStep::Edit { .. } | ScriptStep::Write { .. } | ScriptStep::Delete { .. }
                )
            })
        {
            return Ok(false);
        }
        for skip in patches {
            match skip {
                Some(cond) if self.skip_applies(cond, workdir).await? => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Plays the next attempt. An attempt whose every patch is already in the tree (another agent's win arrived through a rebase) is
    /// not replayed: only its `git` housekeeping runs, and the agent moves straight on to its next idea within the same turn, as a
    /// real agent reading the new tree would.
    async fn play_next(
        &self,
        session: &AgentSession,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        loop {
            let (attempt, index) = {
                let mut cursor = self.cursor.lock().unwrap();
                let index = *cursor;
                *cursor += 1;
                (self.script.attempts.get(index).cloned(), index)
            };
            let Some(attempt) = attempt else {
                self.emit(
                    on_event,
                    AgentEventType::Message,
                    Some("I have run out of ideas worth trying; leaving the tree as it is.".into()),
                    None,
                );
                return Ok(());
            };
            if let Some(cb) = &self.options.on_play {
                cb(&attempt, index);
            }
            // leading git steps (e.g. dropping a rejected attempt) always run, before redundancy is judged
            let lead = attempt
                .steps
                .iter()
                .position(|st| !matches!(st, ScriptStep::Git { .. }));
            let housekeeping_len = lead.unwrap_or(attempt.steps.len());
            let (housekeeping, rest) = attempt.steps.split_at(housekeeping_len);
            self.play(housekeeping, session, spec, on_event, cancel)
                .await?;
            let redundant = self.is_redundant(rest, &spec.workdir).await?;
            if redundant {
                self.emit(
                    on_event,
                    AgentEventType::Message,
                    Some(format!("\"{}\" is already in the tree (it came in with the ratchet best); moving on to my next idea.", attempt.name)),
                    None,
                );
            } else {
                self.play(rest, session, spec, on_event, cancel).await?;
            }
            if cancel.is_cancelled() {
                return Err(AgentError::Aborted {
                    session: Some(session.clone()),
                });
            }
            if !redundant {
                return Ok(());
            }
        }
    }

    async fn play(
        &self,
        steps: &[ScriptStep],
        session: &AgentSession,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        for step in steps {
            if cancel.is_cancelled() {
                self.emit(
                    on_event,
                    AgentEventType::Status,
                    Some("scripted turn aborted".into()),
                    None,
                );
                return Err(AgentError::Aborted {
                    session: Some(session.clone()),
                });
            }
            self.play_step(step, spec, on_event, cancel).await?;
        }
        Ok(())
    }

    fn tool(&self, on_event: EventCallback<'_>, name: &str, text: &str, is_error: bool) {
        self.emit(
            on_event,
            AgentEventType::Tool,
            Some(format!("{name}: {text}")),
            Some(json!({"phase": "call", "name": name})),
        );
        self.emit(
            on_event,
            AgentEventType::Tool,
            Some(if is_error { "failed" } else { "ok" }.into()),
            Some(json!({"phase": "result", "isError": is_error})),
        );
    }

    async fn git(&self, args: &[String], cwd: &Path) -> crate::git::GitResult {
        self.git
            .run(
                args,
                &GitCall {
                    cwd: cwd.to_path_buf(),
                    env: None,
                },
            )
            .await
    }

    async fn play_step(
        &self,
        step: &ScriptStep,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        match step {
            ScriptStep::Message { text } => {
                self.emit(on_event, AgentEventType::Message, Some(text.clone()), None)
            }
            ScriptStep::Tool {
                name,
                input,
                output,
                ms,
            } => {
                let input_json = serde_json::to_string(&input.clone().unwrap_or_default())
                    .unwrap_or_else(|_| "{}".into());
                let shown: String = input_json.chars().take(200).collect();
                self.emit(
                    on_event,
                    AgentEventType::Tool,
                    Some(format!("{name}: {shown}")),
                    Some(json!({"phase": "call", "name": name, "input": input.as_ref().map(|i| Value::Object(i.clone()))})),
                );
                if let Some(ms) = ms.filter(|ms| *ms > 0) {
                    self.clock
                        .sleep(std::time::Duration::from_millis(ms), cancel)
                        .await;
                }
                self.emit(
                    on_event,
                    AgentEventType::Tool,
                    Some(output.clone().unwrap_or_else(|| "ok".into())),
                    Some(json!({"phase": "result", "isError": false})),
                );
            }
            ScriptStep::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                let cost = cost_usd.unwrap_or_else(|| price_usage(*input_tokens, *output_tokens));
                on_event(
                    AgentEvent::new(self.clock.now(), AgentEventType::Usage).with_usage(
                        AgentUsage {
                            input_tokens: *input_tokens,
                            output_tokens: *output_tokens,
                            cost_usd: Some(cost),
                        },
                    ),
                );
            }
            ScriptStep::Sleep { ms } => {
                self.clock
                    .sleep(std::time::Duration::from_millis(*ms), cancel)
                    .await
            }
            ScriptStep::Write { path, content } => {
                let full = self.inside(&spec.workdir, path)?;
                if let Some(dir) = full.parent() {
                    tokio::fs::create_dir_all(dir)
                        .await
                        .map_err(|e| AgentError::failed(e.to_string()))?;
                }
                tokio::fs::write(&full, content)
                    .await
                    .map_err(|e| AgentError::failed(e.to_string()))?;
                self.tool(on_event, "Write", path, false);
            }
            ScriptStep::Delete { path } => {
                let full = self.inside(&spec.workdir, path)?;
                let _ = tokio::fs::remove_file(&full).await;
                self.tool(on_event, "Delete", path, false);
            }
            ScriptStep::Edit {
                path,
                find,
                replace,
            } => {
                let full = self.inside(&spec.workdir, path)?;
                let text = tokio::fs::read_to_string(&full).await.ok();
                let count = text
                    .as_ref()
                    .map_or(0, |t| t.matches(find.as_str()).count());
                let Some(text) = text.filter(|_| count == 1) else {
                    self.tool(on_event, "Edit", path, true);
                    self.emit(
                        on_event,
                        AgentEventType::Error,
                        Some(format!(
                            "edit {path}: expected exactly one match, found {count}"
                        )),
                        Some(json!({"fatal": false})),
                    );
                    return Ok(());
                };
                tokio::fs::write(&full, text.replacen(find.as_str(), replace, 1))
                    .await
                    .map_err(|e| AgentError::failed(e.to_string()))?;
                self.tool(on_event, "Edit", path, false);
            }
            ScriptStep::Git { args } => {
                let res = self.git(args, &spec.workdir).await;
                self.tool(
                    on_event,
                    "Bash",
                    &format!("git {}", args.join(" ")),
                    res.code != 0,
                );
                if res.code != 0 {
                    self.emit(
                        on_event,
                        AgentEventType::Error,
                        Some(res.stderr.trim().to_string()),
                        Some(json!({"fatal": false})),
                    );
                }
            }
            ScriptStep::Patch { name, diff, skip } => {
                if let Some(cond) = skip
                    && self.skip_applies(cond, &spec.workdir).await?
                {
                    self.emit(
                        on_event,
                        AgentEventType::Message,
                        Some(format!(
                            "{} is already in the tree; skipping it.",
                            name.as_deref().unwrap_or("patch")
                        )),
                        None,
                    );
                    return Ok(());
                }
                let dir = std::env::temp_dir().join(format!(
                    "ratchet-patch-{}-{}",
                    std::process::id(),
                    PATCH_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(|e| AgentError::failed(e.to_string()))?;
                let result = async {
                    let file = dir.join("change.patch");
                    tokio::fs::write(&file, diff)
                        .await
                        .map_err(|e| AgentError::failed(e.to_string()))?;
                    let args = vec![
                        "apply".to_string(),
                        "--whitespace=nowarn".to_string(),
                        file.to_string_lossy().into_owned(),
                    ];
                    let res = self.git(&args, &spec.workdir).await;
                    self.tool(
                        on_event,
                        "Edit",
                        name.as_deref().unwrap_or("patch"),
                        res.code != 0,
                    );
                    if res.code != 0 {
                        self.emit(
                            on_event,
                            AgentEventType::Error,
                            Some(format!(
                                "patch {} does not apply: {}",
                                name.as_deref().unwrap_or(""),
                                res.stderr.trim()
                            )),
                            Some(json!({"fatal": false})),
                        );
                    }
                    Ok::<(), AgentError>(())
                }
                .await;
                let _ = tokio::fs::remove_dir_all(&dir).await;
                result?;
            }
            ScriptStep::Fail { message } => {
                self.emit(
                    on_event,
                    AgentEventType::Error,
                    Some(message.clone()),
                    Some(json!({"fatal": true})),
                );
                return Err(AgentError::failed(message.clone()));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl AgentAdapter for ScriptedAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Mock
    }

    async fn preflight(&self) -> Preflight {
        let ok = !self.script.attempts.is_empty();
        Preflight {
            ok,
            problems: if ok {
                Vec::new()
            } else {
                vec!["script has no attempts".to_string()]
            },
        }
    }

    async fn start(
        &self,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<AgentSession, AgentError> {
        let n = self.sessions.fetch_add(1, Ordering::SeqCst) + 1;
        let session = AgentSession {
            id: format!("mock-{}-{n}", self.script.name),
            kind: AgentKind::Mock,
        };
        self.session_ids.lock().unwrap().insert(session.id.clone());
        self.emit(
            on_event,
            AgentEventType::Status,
            Some(format!("scripted agent '{}' started", self.script.name)),
            Some(json!({"sessionId": session.id})),
        );
        self.play_next(&session, spec, on_event, cancel).await?;
        Ok(session)
    }

    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        if !self.session_ids.lock().unwrap().contains(&session.id) {
            return Err(AgentError::failed(format!(
                "unknown scripted session {}",
                session.id
            )));
        }
        self.received.lock().unwrap().push(message.to_string());
        self.play_next(session, spec, on_event, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_is_lexical() {
        assert_eq!(
            resolve(Path::new("/w/a"), "../evil"),
            PathBuf::from("/w/evil")
        );
        assert_eq!(
            resolve(Path::new("/w/a"), "x/./y/../z"),
            PathBuf::from("/w/a/x/z")
        );
        assert_eq!(
            resolve(Path::new("/w/a"), "/etc/passwd"),
            PathBuf::from("/etc/passwd")
        );
    }

    #[test]
    fn steps_have_the_same_json_shape_as_the_ts_scripts() {
        let step: ScriptStep =
            serde_json::from_str(r#"{"type":"usage","inputTokens":5,"outputTokens":2}"#).unwrap();
        assert_eq!(
            step,
            ScriptStep::Usage {
                input_tokens: 5,
                output_tokens: 2,
                cost_usd: None
            }
        );
        let step: ScriptStep =
            serde_json::from_str(r#"{"type":"patch","name":"p","diff":"d","skip":{"file":"a","contains":"b","skipWhen":"present"}}"#).unwrap();
        assert!(matches!(
            step,
            ScriptStep::Patch {
                skip: Some(StepCondition {
                    skip_when: SkipWhen::Present,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(
            serde_json::to_string(&ScriptStep::Sleep { ms: 3 }).unwrap(),
            r#"{"type":"sleep","ms":3}"#
        );
    }
}
