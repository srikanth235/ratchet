//! The scripted chat harness: a conversation with no model and no CLI, so the app works offline, in snapshots and in tests.
//!
//! It speaks like a real harness: `open` takes a [`SessionSpec`] (read-only, the Ratchet MCP server with its session token, the
//! role prompt, `resume`), `send` streams `Delta`s, `Tool` steps, a `Message` and `Usage` into the sink, and a session closed and
//! reopened with `resume` remembers the conversation. It calls Ratchet's tools through the daemon with its session token, so a
//! scripted chat creates a real (simulated) optimization end to end.
//!
//! What it does with a message:
//!
//! | The message | The script |
//! |---|---|
//! | names a repo (`owner/name`) or the chat has one | looks at the templates, drafts an optimization (`$50`, "every night", "weekdays", "use Codex too" are understood) and says what it chose |
//! | asks for a change to the draft | drafts again with the change (the card updates) |
//! | says "create it", "go ahead", "yes" | creates the draft |
//! | any message in a chat bound to an optimization | sends it as a steering note |
//! | contains `[simulate:rate-limit]` | the harness runs out of quota (resets in an hour) |
//! | contains `[simulate:auth-expired]` | the login is rejected |
//! | contains `[simulate:crash]` | the process dies |
//! | contains `[simulate:slow]` | streams slowly until interrupted (or 30 s) |

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::ports::{
    AgentEvent, AgentEventType, AgentUsage, LiveSession, PortError, PortResult, SessionSpec,
    SharedEventSink,
};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::engine::{ChatEngine, ChatHarness, ChatToolCaller, session_token};
use crate::api::{HarnessAuth, HarnessKind};

/// How the scripted engine behaves.
#[derive(Clone, Debug)]
pub struct ScriptedOptions {
    /// Pause between streamed chunks (the typing effect). Zero in tests.
    pub chunk_delay: Duration,
    /// Where checkouts go; default a directory under the system temp dir.
    pub root: Option<PathBuf>,
}

impl Default for ScriptedOptions {
    fn default() -> Self {
        Self {
            chunk_delay: Duration::ZERO,
            root: None,
        }
    }
}

/// What a test can see of each `open`.
#[derive(Clone, Debug)]
pub struct OpenRecord {
    pub kind: HarnessKind,
    pub spec: SessionSpec,
    /// The CLI session id the session answers to.
    pub session_id: String,
}

#[derive(Clone, Debug, Default)]
struct Params {
    repo: String,
    goal: String,
    budget: Option<f64>,
    schedule: Option<String>,
    agents: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Memory {
    draft_id: Option<String>,
    params: Option<Params>,
    turns: u32,
}

pub struct ScriptedChatEngine {
    me: Weak<ScriptedChatEngine>,
    opts: ScriptedOptions,
    caller: OnceLock<Arc<dyn ChatToolCaller>>,
    harnesses: Mutex<Vec<ChatHarness>>,
    memory: Mutex<HashMap<String, Memory>>,
    opened: Mutex<Vec<OpenRecord>>,
    closed: Mutex<Vec<String>>,
    next: AtomicU64,
    root: PathBuf,
}

fn ready(kind: HarnessKind, auth: HarnessAuth, model: &str) -> ChatHarness {
    ChatHarness {
        kind,
        ready: true,
        auth,
        hint: String::new(),
        default_model: Some(model.to_owned()),
    }
}

impl ScriptedChatEngine {
    pub fn new(opts: ScriptedOptions) -> Arc<Self> {
        let root = opts.root.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("ratchet-sim-chats-{}", std::process::id()))
        });
        Arc::new_cyclic(|me| {
            Self {
            me: me.clone(),
            opts,
            caller: OnceLock::new(),
            harnesses: Mutex::new(vec![
                ready(HarnessKind::ClaudeCode, HarnessAuth::SignedIn, "sonnet"),
                ready(HarnessKind::Codex, HarnessAuth::ApiKey, "gpt-5.1-codex-mini"),
                ChatHarness {
                    kind: HarnessKind::Grok,
                    ready: false,
                    auth: HarnessAuth::SignedOut,
                    hint: "Sign in to Grok: run `grok login` in a terminal, then check again in Customize."
                        .into(),
                    default_model: Some("grok-code-fast-1".into()),
                },
            ]),
            memory: Mutex::new(HashMap::new()),
            opened: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            root,
        }
        })
    }

    /// Replace the harness list (tests: sign one out, make none ready).
    pub fn set_harnesses(&self, list: Vec<ChatHarness>) {
        *self.harnesses.lock().unwrap_or_else(|e| e.into_inner()) = list;
    }

    /// Every session opened so far.
    pub fn opened(&self) -> Vec<OpenRecord> {
        self.opened
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// CLI session ids that were closed, in order.
    pub fn closed(&self) -> Vec<String> {
        self.closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn caller(&self) -> Option<Arc<dyn ChatToolCaller>> {
        self.caller.get().cloned()
    }
}

#[async_trait]
impl ChatEngine for ScriptedChatEngine {
    async fn harnesses(&self) -> Vec<ChatHarness> {
        self.harnesses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn attach_tools(&self, caller: Arc<dyn ChatToolCaller>) {
        let _ = self.caller.set(caller);
    }

    async fn prepare_workdir(&self, chat_id: &str, repo: Option<&str>) -> PortResult<PathBuf> {
        let dir = self.root.join(chat_id);
        tokio::fs::create_dir_all(&dir).await?;
        if let Some(repo) = repo {
            tokio::fs::write(
                dir.join("README.md"),
                format!("Simulated checkout of {repo} (read-only).\n"),
            )
            .await?;
        }
        Ok(dir)
    }

    async fn release_workdir(&self, chat_id: &str) {
        let _ = tokio::fs::remove_dir_all(self.root.join(chat_id)).await;
    }

    async fn open(
        &self,
        kind: HarnessKind,
        spec: SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>> {
        let id = spec
            .resume
            .clone()
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| format!("scripted-{}", self.next.fetch_add(1, Ordering::SeqCst)));
        self.opened
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(OpenRecord {
                kind,
                spec: spec.clone(),
                session_id: id.clone(),
            });
        let engine = self
            .me
            .upgrade()
            .ok_or_else(|| PortError::failed("the scripted engine is gone"))?;
        Ok(Box::new(ScriptedSession {
            id,
            token: session_token(&spec),
            model: spec.model.clone(),
            engine,
            events,
            interrupted: AtomicBool::new(false),
            wake: Notify::new(),
        }))
    }
}

struct ScriptedSession {
    id: String,
    token: Option<String>,
    model: Option<String>,
    engine: Arc<ScriptedChatEngine>,
    events: SharedEventSink,
    interrupted: AtomicBool,
    wake: Notify,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl ScriptedSession {
    fn emit(&self, ev: AgentEvent) {
        (self.events)(ev);
    }

    fn memory(&self) -> Memory {
        self.engine
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.id)
            .cloned()
            .unwrap_or_default()
    }

    fn remember(&self, m: Memory) {
        self.engine
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.id.clone(), m);
    }

    fn stopped(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Stream `text` word by word, then the whole message and its usage. False when interrupted on the way.
    async fn say(&self, text: &str) -> bool {
        let mut chunk = String::new();
        let mut words = text.split_inclusive(' ').peekable();
        while let Some(w) = words.next() {
            chunk.push_str(w);
            if chunk.len() >= 12 || words.peek().is_none() {
                self.emit(
                    AgentEvent::new(now_ms(), AgentEventType::Delta).with_text(chunk.clone()),
                );
                chunk.clear();
                if self.engine.opts.chunk_delay.is_zero() {
                    tokio::task::yield_now().await;
                } else {
                    self.nap(self.engine.opts.chunk_delay).await;
                }
                if self.stopped() {
                    return false;
                }
            }
        }
        self.emit(AgentEvent::new(now_ms(), AgentEventType::Message).with_text(text));
        self.emit(
            AgentEvent::new(now_ms(), AgentEventType::Usage).with_usage(AgentUsage {
                input_tokens: 120 + text.len() as u64,
                output_tokens: (text.split_whitespace().count() as u64) * 2,
                cost_usd: Some(0.0004),
                ..AgentUsage::default()
            }),
        );
        true
    }

    /// Sleep, waking early when interrupted.
    async fn nap(&self, d: Duration) {
        tokio::select! {
            () = tokio::time::sleep(d) => {}
            () = self.wake.notified() => {}
        }
    }

    async fn tool(&self, n: u32, name: &str, input: Value) -> Result<Value, String> {
        let id = format!("toolu_{}_{n}", self.id);
        self.emit(
            AgentEvent::new(now_ms(), AgentEventType::Tool)
                .with_text(super::cards::tool_label(name))
                .with_data(json!({"phase": "call", "id": id, "name": format!("mcp__ratchet__{name}"), "input": input})),
        );
        let result = match (&self.token, self.engine.caller()) {
            (Some(t), Some(c)) => c.call(t, name, input).await,
            _ => Err("this session has no Ratchet tools".to_owned()),
        };
        self.emit(
            AgentEvent::new(now_ms(), AgentEventType::Tool)
                .with_data(json!({"phase": "result", "id": id, "isError": result.is_err()})),
        );
        result
    }

    async fn script(&self, text: &str) -> PortResult<()> {
        let lower = text.to_lowercase();
        if lower.contains("[simulate:rate-limit]") {
            let resets = now_ms() / 1000 + 3600;
            self.emit(
                AgentEvent::new(now_ms(), AgentEventType::RateLimited)
                    .with_text("scripted usage limit reached")
                    .with_data(json!({"resetsAt": resets})),
            );
            self.emit(
                AgentEvent::new(now_ms(), AgentEventType::Error)
                    .with_text("You've hit your usage limit."),
            );
            return Err(PortError::agent_failed("You've hit your usage limit."));
        }
        if lower.contains("[simulate:auth-expired]") {
            self.emit(
                AgentEvent::new(now_ms(), AgentEventType::AuthExpired)
                    .with_text("scripted: not logged in"),
            );
            self.emit(
                AgentEvent::new(now_ms(), AgentEventType::Error)
                    .with_text("Not logged in · Please run /login"),
            );
            return Err(PortError::agent_failed("Not logged in · Please run /login"));
        }
        if lower.contains("[simulate:crash]") {
            return Err(PortError::agent_failed(
                "the scripted CLI exited with status 1",
            ));
        }
        if lower.contains("[simulate:slow]") {
            let reply =
                "Working on it, reading the repository and thinking about what to measure. "
                    .repeat(40);
            for _ in 0..1500 {
                self.emit(
                    AgentEvent::new(now_ms(), AgentEventType::Delta).with_text("still working, "),
                );
                self.nap(Duration::from_millis(20)).await;
                if self.stopped() {
                    return Ok(());
                }
            }
            self.say(&reply).await;
            return Ok(());
        }

        let scope = match (&self.token, self.engine.caller()) {
            (Some(t), Some(c)) => c.scope(t).await.unwrap_or_default(),
            _ => Default::default(),
        };
        let mut mem = self.memory();
        mem.turns += 1;

        // steering
        if let Some(opt) = &scope.optimization {
            let note = text.trim().to_owned();
            match self
                .tool(
                    1,
                    "add_steering_note",
                    json!({"optimization": opt, "text": note}),
                )
                .await
            {
                Ok(_) => {
                    self.say(&format!(
                        "Noted. I passed that to the agents on {opt}; they read it at the start of their next turn."
                    ))
                    .await;
                }
                Err(e) => {
                    self.say(&format!("I couldn't send that note: {e}")).await;
                }
            }
            self.remember(mem);
            return Ok(());
        }

        // create
        let wants_create = [
            "create it",
            "go ahead",
            "create the optimization",
            "looks good",
        ]
        .iter()
        .any(|k| lower.contains(k))
            || lower.trim_start().starts_with("yes");
        if wants_create && let Some(draft) = mem.draft_id.clone() {
            match self
                .tool(1, "create_optimization", json!({"draft_id": draft}))
                .await
            {
                Ok(v) => {
                    let id = v["optimization"].as_str().unwrap_or("the optimization");
                    self.say(&format!(
                        "Created {id}. It is running now; it will not wait for you, and a verified win becomes a pull request."
                    ))
                    .await;
                    mem.draft_id = None;
                }
                Err(e) => {
                    self.say(&format!("I couldn't create it: {e}")).await;
                }
            }
            self.remember(mem);
            return Ok(());
        }

        // draft or redraft
        let repo = slug_in(text)
            .or_else(|| scope.repo.clone())
            .or_else(|| mem.params.as_ref().map(|p| p.repo.clone()));
        let Some(repo) = repo else {
            self.say("Which repository should I look at? Give me its owner/name.")
                .await;
            self.remember(mem);
            return Ok(());
        };
        let mut p = mem.params.clone().unwrap_or_else(|| Params {
            repo: repo.clone(),
            goal: text.trim().to_owned(),
            agents: vec!["claude-code".into()],
            ..Params::default()
        });
        let first = mem.params.is_none();
        apply_hints(&mut p, text);
        if first && self.tool(1, "list_templates", json!({})).await.is_err() {
            self.say("I couldn't look at the templates.").await;
            self.remember(mem);
            return Ok(());
        }
        let mut args = json!({"repo": p.repo, "goal": p.goal, "agents": p.agents});
        if let Some(b) = p.budget {
            args["budget"] = json!(b);
        }
        if let Some(s) = &p.schedule {
            args["schedule"] = json!(s);
        }
        match self.tool(2, "draft_optimization", args).await {
            Ok(v) => {
                mem.draft_id = v["draftId"].as_str().map(str::to_owned);
                mem.params = Some(p);
                let reply = if first {
                    "Here is the optimization I would run. Create it and it starts at once; change anything in plain words first if you like."
                } else {
                    "Updated the draft. Say \"create it\" when it looks right."
                };
                self.say(reply).await;
            }
            Err(e) => {
                self.say(&format!("I couldn't draft that: {e}")).await;
            }
        }
        self.remember(mem);
        Ok(())
    }
}

fn slug_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| ",.;:()\"'".contains(c)))
        .filter(|w| w.contains('/') && !w.starts_with('/'))
        .find_map(ratchet_github::slug_of)
}

fn apply_hints(p: &mut Params, text: &str) {
    let lower = text.to_lowercase();
    if let Some(at) = text.find('$') {
        let digits: String = text[at + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(b) = digits.parse::<f64>()
            && b > 0.0
        {
            p.budget = Some(b);
        }
    }
    if lower.contains("every night") || lower.contains("nightly") {
        p.schedule = Some("every night at 02:00".into());
    }
    if lower.contains("weekday") {
        p.schedule = Some("every weekday at 02:00".into());
    }
    for (word, id) in [("codex", "codex"), ("grok", "grok")] {
        if lower.contains(word) && !p.agents.iter().any(|a| a == id) {
            p.agents.push(id.into());
        }
    }
}

#[async_trait]
impl LiveSession for ScriptedSession {
    async fn send(&self, text: &str) -> PortResult<()> {
        self.interrupted.store(false, Ordering::SeqCst);
        if self.memory().turns == 0 {
            self.emit(
                AgentEvent::new(now_ms(), AgentEventType::Status)
                    .with_text("scripted session started")
                    .with_data(
                        json!({"sessionId": self.id, "model": self.model, "liveSession": true}),
                    ),
            );
        }
        self.script(text).await
    }

    async fn interrupt(&self) -> PortResult<()> {
        self.interrupted.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
        Ok(())
    }

    fn session_id(&self) -> Option<String> {
        Some(self.id.clone())
    }

    async fn close(self: Box<Self>) {
        self.engine
            .closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(self.id.clone());
    }
}
