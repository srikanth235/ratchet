//! The harness layer shared by the three adapters: per-session scratch files, the spawn-per-turn fallback session, and the common
//! way `Harness::open` chooses between a live session and that fallback.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    AgentEvent, AgentEventType, AgentSession, CancellationToken, EventSink, LiveSession, PortError,
    PortResult, SessionSpec, SharedEventSink,
};

/// A temp directory for one session's generated config (MCP JSON, a `CODEX_HOME` / `GROK_HOME` overlay, a prompt file). Removed
/// when dropped, so keep it alive as long as the CLI runs. Created `0700`: it can hold server environment values.
pub struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    pub fn new(prefix: &str) -> PortResult<Self> {
        tempfile::Builder::new()
            .prefix(&format!("ratchet-{prefix}-"))
            .tempdir()
            .map(|dir| Self { dir })
            .map_err(|e| PortError::Failed(format!("cannot create a scratch directory: {e}")))
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write `content` to `name` inside the directory (`0600`) and return the path.
    pub fn write(&self, name: &str, content: &str) -> PortResult<PathBuf> {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                PortError::Failed(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        std::fs::write(&path, content)
            .map_err(|e| PortError::Failed(format!("cannot write {}: {e}", path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(path)
    }
}

/// One CLI turn as an adapter runs it: what the spawn-per-turn session calls.
#[async_trait]
pub trait TurnRunner: Send + Sync {
    fn kind(&self) -> AgentKind;
    /// Run one turn to the end. `resume` is the CLI session to continue (`None`: a new one, `session.id` is set from the
    /// stream). `prompt` is this turn's user message.
    async fn run_turn(
        &self,
        session: &mut AgentSession,
        spec: &SessionSpec,
        resume: Option<&str>,
        prompt: &str,
        events: &EventSink<'_>,
        cancel: &CancellationToken,
    ) -> PortResult<()>;
}

/// The fallback [`LiveSession`]: no process is held, every `send` is a `start` (first turn) or `resume` (later turns). Slower to
/// the first token, same behaviour. Also what `Harness::open` returns when a CLI has no live mode or its live mode fails to open.
pub struct SpawnPerTurnSession {
    runner: Arc<dyn TurnRunner>,
    spec: SessionSpec,
    events: SharedEventSink,
    turn: tokio::sync::Mutex<AgentSession>,
    id: std::sync::Mutex<Option<String>>,
    cancel: std::sync::Mutex<CancellationToken>,
    interrupted: AtomicBool,
    closed: AtomicBool,
}

impl SpawnPerTurnSession {
    pub fn new(runner: Arc<dyn TurnRunner>, spec: SessionSpec, events: SharedEventSink) -> Self {
        let id = spec.resume.clone().filter(|i| !i.is_empty());
        let session = AgentSession {
            id: id.clone().unwrap_or_default(),
            kind: runner.kind(),
        };
        Self {
            runner,
            spec,
            events,
            turn: tokio::sync::Mutex::new(session),
            id: std::sync::Mutex::new(id),
            cancel: std::sync::Mutex::new(CancellationToken::new()),
            interrupted: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl LiveSession for SpawnPerTurnSession {
    async fn send(&self, text: &str) -> PortResult<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PortError::Failed("the session is closed".into()));
        }
        let mut session = self.turn.lock().await;
        let token = CancellationToken::new();
        *self.cancel.lock().unwrap() = token.clone();
        self.interrupted.store(false, Ordering::SeqCst);
        let resume = Some(session.id.clone()).filter(|i| !i.is_empty());
        let events: &EventSink<'_> = &*self.events;
        let res = self
            .runner
            .run_turn(
                &mut session,
                &self.spec,
                resume.as_deref(),
                text,
                events,
                &token,
            )
            .await;
        if !session.id.is_empty() {
            *self.id.lock().unwrap() = Some(session.id.clone());
        }
        match res {
            Err(PortError::Cancelled) if self.interrupted.load(Ordering::SeqCst) => Ok(()),
            other => other,
        }
    }

    async fn interrupt(&self) -> PortResult<()> {
        self.interrupted.store(true, Ordering::SeqCst);
        self.cancel.lock().unwrap().cancel();
        Ok(())
    }

    fn session_id(&self) -> Option<String> {
        self.id.lock().unwrap().clone()
    }

    async fn close(self: Box<Self>) {
        self.closed.store(true, Ordering::SeqCst);
        self.cancel.lock().unwrap().cancel();
    }
}

/// Status event telling the log why chat runs in per-turn mode.
pub fn fallback_notice(now: i64, cli: &str, reason: &str) -> AgentEvent {
    AgentEvent::new(now, AgentEventType::Status)
        .with_text(format!(
            "{cli}: no live session ({reason}); using one process per turn"
        ))
        .with_data(serde_json::json!({"liveSession": false}))
}

/// How long a live session waits for its process to leave after stdin closes.
pub const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// Request timeout for handshake calls of the JSON-RPC protocols.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// `open` for every adapter: a live session when enabled and it opens, else the spawn-per-turn fallback. The first message
/// (`spec.prompt`, when not empty) is sent before returning.
pub async fn open_with_fallback<F, Fut>(
    live_enabled: bool,
    now: impl Fn() -> i64,
    cli: &str,
    runner: Arc<dyn TurnRunner>,
    spec: &SessionSpec,
    events: SharedEventSink,
    try_live: F,
) -> PortResult<Box<dyn LiveSession>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = PortResult<Box<dyn LiveSession>>>,
{
    let mut session: Option<Box<dyn LiveSession>> = None;
    if live_enabled {
        match try_live().await {
            Ok(live) => session = Some(live),
            Err(err) => events(fallback_notice(now(), cli, &err.to_string())),
        }
    } else {
        events(fallback_notice(now(), cli, "disabled"));
    }
    let session = session.unwrap_or_else(|| {
        Box::new(SpawnPerTurnSession::new(
            runner,
            spec.clone(),
            events.clone(),
        ))
    });
    if !spec.prompt.is_empty()
        && let Err(err) = session.send(&spec.prompt).await
    {
        session.close().await;
        return Err(err);
    }
    Ok(session)
}
