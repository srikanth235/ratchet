//! `ChatService`: chats, their live sessions and their streams.
//!
//! ```text
//! POST /messages ──► ChatService::send_message ──► ledger (user row) ──► stream: message
//!                         │ spawn
//!                         ▼
//!                    turn task: ensure_live (checkout, MCP token, engine.open) ── session.send(text) ──► (returns at turn end)
//!                         │ harness sink (sync, never blocks)                                │
//!                         ▼                                                                  ▼
//!                    EventQueue ◄──────────────────────────── TurnEnd pushed behind every event of the turn
//!                         │ coalesces neighbouring deltas
//!                         ▼
//!                    pump task: delta/thinking → stream only; message/tool/usage → ledger + stream; TurnEnd → done / error
//! ```
//!
//! Hot path: a delta goes queue → pump → broadcast, with no ledger write and no refetch; the partial reply is kept in a capped
//! buffer so a client that joins late (or lagged) gets it from `ready`. Replies that finish are stored once.
//!
//! One live session per open chat, closed after [`ChatOptions::idle`] without a message (its MCP token is revoked) and reopened by
//! the next message with `resume`. At most [`ChatOptions::max_live`] sessions are held; opening one more closes the one idle the
//! longest.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use ratchet_core::ports::{
    AgentEvent, AgentEventType, LiveSession, PortError, SessionSpec, SharedEventSink,
};
use ratchet_ledger::{ChatMessageRow, ChatRow, Ledger, NewChat, NewChatMessage};
use ratchet_mcp::ToolSet;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::cards::{self, Failure};
use super::engine::{ChatEngine, ChatToolCaller, ToolScope};
use super::prompt::role_prompt;
use super::queue::{EventQueue, Item, TurnEnd};
use crate::api::*;
use crate::mcp::{McpHub, Scope, SessionInfo};
use crate::routes::AppState;
use crate::service::{AppService, ServiceError, ServiceResult};

/// Ledger key of the user's chat harness (`claude-code`, `codex`, `grok`); absent: the first signed-in one.
pub const DEFAULT_HARNESS_KEY: &str = "chat.defaultHarness";

/// Knobs of the service. The defaults are the product's.
#[derive(Clone)]
pub struct ChatOptions {
    /// A live session closes after this long without a message.
    pub idle: Duration,
    /// How many live sessions the pool keeps.
    pub max_live: usize,
    /// The binary a session's MCP server runs (`ratchetd mcp --session <token>`).
    pub ratchetd: String,
    /// Stream events kept per chat for `Last-Event-ID` resume.
    pub ring: usize,
    /// Most partial-reply text kept per chat, bytes.
    pub pending_cap: usize,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(600),
            max_live: 4,
            ratchetd: std::env::current_exe()
                .ok()
                .map_or_else(|| "ratchetd".to_owned(), |p| p.display().to_string()),
            ring: 256,
            pending_cap: 64 * 1024,
        }
    }
}

/// Longest text of one stored message, characters.
const MAX_STORED_CHARS: usize = 200_000;
/// Longest tool summary, characters.
const MAX_SUMMARY_CHARS: usize = 300;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn internal(e: impl std::fmt::Display) -> ServiceError {
    ServiceError::Internal(e.to_string())
}

/// A steering note waiting for an agent to take it up.
#[derive(Clone)]
struct PendingNote {
    chat: String,
    note: String,
    optimization: String,
}

struct Shared {
    ledger: Ledger,
    engine: Arc<dyn ChatEngine>,
    mcp: Arc<McpHub>,
    service: Option<Arc<dyn AppService>>,
    opts: ChatOptions,
    socket: Mutex<Option<PathBuf>>,
    /// MCP session id → chat id, for the tool hook.
    by_session: Mutex<HashMap<String, String>>,
    notes: Mutex<Vec<PendingNote>>,
    /// Counts every use of a live session, across chats, so the pool knows which has been idle the longest.
    clock: AtomicU64,
}

struct Live {
    session: Arc<Box<dyn LiveSession>>,
    mcp_id: String,
    /// Bumped on every use; the pool closes the lowest.
    used: u64,
}

#[derive(Default)]
struct State {
    replying: bool,
    turn: u64,
    interrupted: bool,
    pending: String,
    live: Option<Live>,
    workdir: Option<PathBuf>,
    cli_session: Option<String>,
    ring: VecDeque<(u64, Arc<ChatStreamEvent>)>,
    next_id: u64,
    idle_gen: u64,
    usage: ChatUsageVm,
    last_reply_seq: Option<i64>,
    model_seen: Option<String>,
    failure: Option<Failure>,
    /// CLI tool call id → the row that shows it.
    tool_rows: HashMap<String, i64>,
    /// The draft card that can still be replaced or created.
    open_draft: Option<(String, i64)>,
    billing: Option<HarnessBilling>,
}

/// One chat in memory: its identity (fixed at creation), its stream and its live session.
struct Chat {
    id: String,
    repo: Option<String>,
    optimization: Option<String>,
    harness: HarnessKind,
    model: Option<String>,
    created_at: i64,
    shared: Arc<Shared>,
    queue: Arc<EventQueue>,
    state: Mutex<State>,
    tx: broadcast::Sender<Arc<(u64, Arc<ChatStreamEvent>)>>,
    closed: AtomicBool,
    pump: Mutex<Option<JoinHandle<()>>>,
}

/// What `GET .../stream` starts from: the events to send first, then the live receiver.
pub struct ChatStream {
    /// `Ready`, then (on a resume) the events missed.
    pub head: Vec<(Option<u64>, ChatStreamEvent)>,
    pub live: broadcast::Receiver<Arc<(u64, Arc<ChatStreamEvent>)>>,
}

pub struct ChatService {
    shared: Arc<Shared>,
    chats: Mutex<HashMap<String, Arc<Chat>>>,
    tools: Mutex<Option<AppState>>,
    watcher: Mutex<Option<JoinHandle<()>>>,
}

impl ChatService {
    /// `service` validates optimizations and names the agent that read a note; without it chats still work.
    pub fn new(
        ledger: Ledger,
        engine: Arc<dyn ChatEngine>,
        mcp: Arc<McpHub>,
        service: Option<Arc<dyn AppService>>,
        opts: ChatOptions,
    ) -> Arc<Self> {
        let svc = Arc::new_cyclic(|me: &Weak<ChatService>| {
            engine.attach_tools(Arc::new(DaemonTools(me.clone())));
            Self {
                shared: Arc::new(Shared {
                    ledger,
                    engine,
                    mcp,
                    service,
                    opts,
                    socket: Mutex::new(None),
                    by_session: Mutex::new(HashMap::new()),
                    notes: Mutex::new(Vec::new()),
                    clock: AtomicU64::new(0),
                }),
                chats: Mutex::new(HashMap::new()),
                tools: Mutex::new(None),
                watcher: Mutex::new(None),
            }
        });
        svc
    }

    /// Connect the service to the running daemon: tool calls of scripted harnesses go through its router state, and agent
    /// activity marks steering notes as read. Called once by `Daemon::start`.
    pub fn bind(self: &Arc<Self>, state: AppState) {
        *lock(&self.shared.socket) = state.socket_path.as_ref().map(PathBuf::from);
        let mut events = state.service.subscribe();
        *lock(&self.tools) = Some(state);
        let weak: Weak<ChatService> = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(ApiEvent::AgentEvent {
                        optimization_id,
                        agent_id,
                        event: Some(e),
                    }) if is_agent_activity(&e) => {
                        let Some(svc) = weak.upgrade() else { break };
                        svc.mark_notes_read(&optimization_id, &agent_id, e.at).await;
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        *lock(&self.watcher) = Some(task);
    }

    // ---- create, read, delete ------------------------------------------------------------------------------------

    pub async fn create_chat(self: &Arc<Self>, input: CreateChatInput) -> ServiceResult<ChatVm> {
        let mut repo = match input.repo.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => Some(ratchet_github::slug_of(r).ok_or_else(|| {
                ServiceError::InvalidFields {
                    message: format!("{r} is not a GitHub repository (owner/name)"),
                    fields: vec![FieldError::new("repo", "use owner/name")],
                }
            })?),
            None => None,
        };
        let optimization = input
            .optimization
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(str::to_owned);
        if let (Some(o), Some(svc)) = (&optimization, &self.shared.service) {
            let detail = svc.get_optimization(o).await?;
            if repo.is_none() {
                repo = ratchet_github::slug_of(&detail.summary.repo);
            }
        }

        let harnesses = self.shared.engine.harnesses().await;
        let ready_first = |k: Option<HarnessKind>| {
            k.and_then(|k| harnesses.iter().find(|h| h.kind == k && h.ready))
        };
        let chosen = if let Some(want) = input.harness {
            let h = harnesses.iter().find(|h| h.kind == want);
            match h {
                Some(h) if h.ready => h,
                Some(h) => {
                    return Err(ServiceError::NotReady(format!(
                        "{} is not ready: {}",
                        want.label(),
                        h.hint
                    )));
                }
                None => {
                    return Err(ServiceError::NotReady(format!(
                        "{} was not found on this Mac. Install it, or pick another agent.",
                        want.label()
                    )));
                }
            }
        } else {
            let saved = self
                .shared
                .ledger
                .kv_get(DEFAULT_HARNESS_KEY)
                .await
                .ok()
                .flatten()
                .and_then(|id| HarnessKind::from_id(&id));
            ready_first(saved)
                .or_else(|| harnesses.iter().find(|h| h.ready))
                .ok_or_else(|| {
                    ServiceError::NotReady(
                        "No agent is signed in on this Mac. Sign in to Claude Code, Codex or Grok in Customize, then try again."
                            .into(),
                    )
                })?
        };
        let model = input
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .or_else(|| chosen.default_model.clone());

        let id = format!("chat_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let row = self
            .shared
            .ledger
            .create_chat(NewChat {
                id: id.clone(),
                repo,
                optimization_id: optimization,
                harness: chosen.kind.id().to_owned(),
                model,
            })
            .await?;
        let chat = self.adopt(row);
        self.view(&chat, None, 0, false).await
    }

    pub async fn get_chat(
        self: &Arc<Self>,
        id: &str,
        query: &ChatQuery,
    ) -> ServiceResult<ChatVm> {
        let chat = self.chat(id).await?;
        let limit = query.limit.unwrap_or(200).clamp(1, 1000);
        self.view(&chat, Some(limit), query.after.unwrap_or(0).max(0), true)
            .await
    }

    /// Stop the reply, close the session (its token stops working), remove the checkout and forget the chat and its messages.
    pub async fn delete_chat(self: &Arc<Self>, id: &str) -> ServiceResult<()> {
        let chat = self.chat(id).await?;
        chat.closed.store(true, Ordering::SeqCst);
        lock(&self.chats).remove(id);
        chat.interrupt().await;
        chat.close_live().await;
        self.shared.engine.release_workdir(id).await;
        lock(&self.shared.notes).retain(|n| n.chat != id);
        chat.queue.close();
        self.shared.ledger.delete_chat(id).await?;
        Ok(())
    }

    // ---- messages and turns --------------------------------------------------------------------------------------

    pub async fn send_message(
        self: &Arc<Self>,
        id: &str,
        input: SendChatMessageInput,
    ) -> ServiceResult<ChatMessageAccepted> {
        let text = input.text.trim().to_owned();
        let chars = text.chars().count();
        if chars == 0 || chars > MAX_CHAT_MESSAGE_CHARS {
            return Err(ServiceError::InvalidFields {
                message: format!("A message is 1 to {MAX_CHAT_MESSAGE_CHARS} characters."),
                fields: vec![FieldError::new(
                    "text",
                    format!("must be 1 to {MAX_CHAT_MESSAGE_CHARS} characters"),
                )],
            });
        }
        let chat = self.chat(id).await?;
        let turn = {
            let mut s = chat.lock();
            if s.replying {
                return Err(ServiceError::Conflict(
                    "Ratchet is still replying. Wait for it to finish, or press Stop.".into(),
                ));
            }
            s.replying = true;
            s.turn += 1;
            s.interrupted = false;
            s.pending.clear();
            s.usage = ChatUsageVm {
                billing: s.billing,
                ..ChatUsageVm::default()
            };
            s.last_reply_seq = None;
            s.failure = None;
            s.tool_rows.clear();
            s.idle_gen += 1; // a pending idle close is void
            s.turn
        };
        let row = match chat
            .append(NewChatMessage {
                role: "user".into(),
                text: text.clone(),
                ..NewChatMessage::default()
            })
            .await
        {
            Some(r) => r,
            None => {
                chat.lock().replying = false;
                return Err(ServiceError::Internal(
                    "the message could not be saved".into(),
                ));
            }
        };
        chat.publish(ChatStreamEvent::Message {
            message: message_vm(&row),
        });
        self.make_room(id).await;
        let task = Arc::clone(&chat);
        tokio::spawn(async move { task.run_turn(text).await });
        Ok(ChatMessageAccepted { seq: row.seq, turn })
    }

    /// Stop the reply in progress. Nothing to stop is fine.
    pub async fn interrupt(self: &Arc<Self>, id: &str) -> ServiceResult<()> {
        self.chat(id).await?.interrupt().await;
        Ok(())
    }

    /// Follow a chat. `last_event_id` is the client's `Last-Event-ID`.
    pub async fn stream(
        self: &Arc<Self>,
        id: &str,
        last_event_id: Option<u64>,
    ) -> ServiceResult<ChatStream> {
        let chat = self.chat(id).await?;
        let last_seq = self
            .shared
            .ledger
            .last_chat_seq(id)
            .await
            .map_err(internal)?;
        Ok(chat.subscribe(last_event_id, last_seq))
    }

    // ---- the pool ------------------------------------------------------------------------------------------------

    /// Close the session idle the longest while `keep` is about to need one and the pool is full.
    async fn make_room(&self, keep: &str) {
        loop {
            let victim = {
                let chats = lock(&self.chats);
                let mut live: Vec<(u64, Arc<Chat>)> = Vec::new();
                let mut count = 0;
                for (id, c) in chats.iter() {
                    let s = c.lock();
                    if s.live.is_some() {
                        count += 1;
                        if id != keep && !s.replying {
                            live.push((s.live.as_ref().map_or(0, |l| l.used), Arc::clone(c)));
                        }
                    }
                }
                // `keep` may need a new session: make room for it unless it already has one
                let keep_has = chats.get(keep).is_some_and(|c| c.lock().live.is_some());
                let needed = if keep_has { count } else { count + 1 };
                if needed <= self.shared.opts.max_live {
                    return;
                }
                live.sort_by_key(|(used, _)| *used);
                live.into_iter().next().map(|(_, c)| c)
            };
            match victim {
                Some(c) => c.close_live().await,
                None => return,
            }
        }
    }

    /// How many sessions are open (tests, diagnostics).
    pub fn live_count(&self) -> usize {
        lock(&self.chats)
            .values()
            .filter(|c| c.lock().live.is_some())
            .count()
    }

    /// Close every session and stop the background work (daemon shutdown).
    pub async fn shutdown(&self) {
        if let Some(w) = lock(&self.watcher).take() {
            w.abort();
        }
        let chats: Vec<Arc<Chat>> = lock(&self.chats).drain().map(|(_, c)| c).collect();
        for c in chats {
            c.closed.store(true, Ordering::SeqCst);
            c.interrupt().await;
            c.close_live().await;
            c.queue.close();
        }
        *lock(&self.tools) = None;
    }

    // ---- internals -----------------------------------------------------------------------------------------------

    fn adopt(self: &Arc<Self>, row: ChatRow) -> Arc<Chat> {
        let (tx, _) = broadcast::channel(512);
        let harness = HarnessKind::from_id(&row.harness).unwrap_or(HarnessKind::ClaudeCode);
        let chat = Arc::new(Chat {
            id: row.id.clone(),
            repo: row.repo,
            optimization: row.optimization_id,
            harness,
            model: row.model,
            created_at: row.created_at,
            shared: Arc::clone(&self.shared),
            queue: Arc::new(EventQueue::default()),
            state: Mutex::new(State {
                cli_session: row.cli_session_id,
                next_id: 1,
                ..State::default()
            }),
            tx,
            closed: AtomicBool::new(false),
            pump: Mutex::new(None),
        });
        let pump = tokio::spawn(Arc::clone(&chat).run_pump());
        *lock(&chat.pump) = Some(pump);
        lock(&self.chats).insert(row.id, Arc::clone(&chat));
        chat
    }

    /// The chat in memory, or the one the ledger remembers from before a restart.
    async fn chat(self: &Arc<Self>, id: &str) -> ServiceResult<Arc<Chat>> {
        if let Some(c) = lock(&self.chats).get(id) {
            return Ok(Arc::clone(c));
        }
        let row = self
            .shared
            .ledger
            .get_chat(id)
            .await?
            .ok_or_else(|| ServiceError::not_found("chat", id))?;
        let mut chats = lock(&self.chats);
        if let Some(c) = chats.get(id) {
            return Ok(Arc::clone(c));
        }
        drop(chats);
        let chat = self.adopt(row);
        chats = lock(&self.chats);
        Ok(chats.get(id).cloned().unwrap_or(chat))
    }

    async fn view(
        &self,
        chat: &Arc<Chat>,
        limit: Option<u32>,
        after: i64,
        with_messages: bool,
    ) -> ServiceResult<ChatVm> {
        let (messages, has_more) = if with_messages {
            let limit = limit.unwrap_or(200);
            let mut rows = self
                .shared
                .ledger
                .chat_messages(&chat.id, after, limit + 1)
                .await?;
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            (rows.iter().map(message_vm).collect(), more)
        } else {
            (Vec::new(), false)
        };
        let first = self
            .shared
            .ledger
            .first_chat_of_harness(chat.harness.id())
            .await?;
        let (state, live, pending, last_event_id) = {
            let s = chat.lock();
            (
                if s.replying {
                    ChatState::Replying
                } else {
                    ChatState::Idle
                },
                s.live.is_some(),
                (!s.pending.is_empty()).then(|| PendingReplyVm {
                    text: s.pending.clone(),
                }),
                s.next_id.saturating_sub(1),
            )
        };
        let updated_at = self
            .shared
            .ledger
            .get_chat(&chat.id)
            .await?
            .map_or(chat.created_at, |r| r.updated_at);
        Ok(ChatVm {
            id: chat.id.clone(),
            repo: chat.repo.clone(),
            optimization: chat.optimization.clone(),
            harness: chat.harness,
            model: chat.model.clone(),
            state,
            live,
            created_at: chat.created_at,
            updated_at,
            data_notice: (first.as_deref() == Some(chat.id.as_str()))
                .then(|| cards::data_notice(chat.harness)),
            messages,
            has_more,
            pending,
            last_event_id,
        })
    }

    // ---- the tool hook -------------------------------------------------------------------------------------------

    /// Called after a chat session's tool ran (from `/v1/mcp/call` and from the scripted harness): cards and notes.
    pub async fn observe(
        self: &Arc<Self>,
        session: &SessionInfo,
        tool: &str,
        arguments: &Value,
        outcome: &Result<Value, ServiceError>,
    ) {
        if session.set != ToolSet::Chat {
            return;
        }
        let Some(chat_id) = lock(&self.shared.by_session).get(&session.id).cloned() else {
            return;
        };
        let Some(chat) = lock(&self.chats).get(&chat_id).cloned() else {
            return;
        };
        let ready: Vec<HarnessKind> = self
            .shared
            .engine
            .harnesses()
            .await
            .into_iter()
            .filter(|h| h.ready)
            .map(|h| h.kind)
            .collect();
        chat.on_tool(tool, arguments, outcome, &ready).await;
    }

    /// A steering note counts as read once an agent of its optimization does something after it.
    async fn mark_notes_read(self: &Arc<Self>, optimization: &str, agent_id: &str, at: i64) {
        let due: Vec<PendingNote> = {
            let mut notes = lock(&self.shared.notes);
            let (due, keep): (Vec<_>, Vec<_>) = notes
                .drain(..)
                .partition(|n| n.optimization == optimization);
            *notes = keep;
            due
        };
        if due.is_empty() {
            return;
        }
        let by = match &self.shared.service {
            Some(svc) => svc
                .get_optimization(optimization)
                .await
                .ok()
                .and_then(|d| d.agents.into_iter().find(|a| a.id == agent_id))
                .and_then(|a| HarnessKind::from_agent_kind(a.kind))
                .map(|k| k.label().to_owned()),
            None => None,
        }
        .unwrap_or_else(|| agent_id.to_owned());
        for n in due {
            let chat = lock(&self.chats).get(&n.chat).cloned();
            if let Some(chat) = chat {
                chat.note_read(&n.note, &by, at).await;
            }
        }
    }
}

fn is_agent_activity(e: &AgentEventVm) -> bool {
    match e.event_type.as_str() {
        "tool" | "commit" | "verdict" => true,
        "message" => !e.text.starts_with("You: "),
        _ => false,
    }
}

// ---- the daemon's tool dispatch as a ChatToolCaller ----------------------------------------------------------------

struct DaemonTools(Weak<ChatService>);

#[async_trait::async_trait]
impl ChatToolCaller for DaemonTools {
    async fn call(&self, token: &str, tool: &str, arguments: Value) -> Result<Value, String> {
        let svc = self.0.upgrade().ok_or("the daemon is shutting down")?;
        let state = lock(&svc.tools)
            .clone()
            .ok_or("the chat service is not connected to a daemon")?;
        let session = svc
            .shared
            .mcp
            .sessions
            .resolve(token)
            .ok_or("this session was closed")?;
        crate::mcp::routes::dispatch(&state, &session, tool, arguments)
            .await
            .map_err(|e| e.to_string())
    }

    async fn scope(&self, token: &str) -> Option<ToolScope> {
        let svc = self.0.upgrade()?;
        let s = svc.shared.mcp.sessions.resolve(token)?;
        Some(ToolScope {
            repo: s.scope.repo,
            optimization: s.scope.optimization,
        })
    }
}

// ---- one chat ------------------------------------------------------------------------------------------------------

fn parse_ledger(e: impl std::fmt::Display, what: &str) {
    tracing::warn!("chat: {what}: {e}");
}

impl Chat {
    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Number the event, remember it for resume and hand it to every connected stream.
    fn publish(&self, ev: ChatStreamEvent) -> u64 {
        let mut s = self.lock();
        let id = s.next_id;
        s.next_id += 1;
        let ev = Arc::new(ev);
        s.ring.push_back((id, Arc::clone(&ev)));
        while s.ring.len() > self.shared.opts.ring {
            s.ring.pop_front();
        }
        let _ = self.tx.send(Arc::new((id, ev)));
        id
    }

    fn subscribe(&self, last: Option<u64>, last_seq: i64) -> ChatStream {
        let s = self.lock();
        let newest = s.next_id.saturating_sub(1);
        let (replay, resync): (Vec<(u64, Arc<ChatStreamEvent>)>, bool) = match last {
            None => (Vec::new(), false),
            Some(n) if n > newest => (Vec::new(), true), // from before a restart
            Some(n) if n == newest => (Vec::new(), false),
            Some(n) => match s.ring.front() {
                Some((first, _)) if *first <= n + 1 => (
                    s.ring.iter().filter(|(id, _)| *id > n).cloned().collect(),
                    false,
                ),
                _ => (Vec::new(), true),
            },
        };
        // the partial reply goes in `ready` unless the replay carries the deltas that make it
        let pending = (replay.is_empty() && !s.pending.is_empty()).then(|| PendingReplyVm {
            text: s.pending.clone(),
        });
        let ready = ChatStreamEvent::Ready {
            chat_id: self.id.clone(),
            harness: self.harness,
            model: self.model.clone(),
            state: if s.replying {
                ChatState::Replying
            } else {
                ChatState::Idle
            },
            last_seq,
            last_event_id: newest,
            resync,
            pending,
        };
        let live = self.tx.subscribe(); // under the lock: nothing is published between the snapshot and here
        let mut head = vec![(None, ready)];
        head.extend(replay.into_iter().map(|(id, e)| (Some(id), (*e).clone())));
        ChatStream { head, live }
    }

    async fn append(&self, new: NewChatMessage) -> Option<ChatMessageRow> {
        match self.shared.ledger.append_chat_message(&self.id, new).await {
            Ok(r) => Some(r),
            Err(e) => {
                parse_ledger(e, "cannot store a message");
                None
            }
        }
    }

    // -- the turn --

    async fn run_turn(self: Arc<Self>, text: String) {
        let live = match self.ensure_live().await {
            Ok(l) => l,
            Err(failure) => {
                self.queue.push_end(TurnEnd {
                    result: Ok(()),
                    session_id: None,
                    not_started: Some(failure),
                });
                return;
            }
        };
        if self.lock().interrupted {
            self.queue.push_end(TurnEnd {
                result: Ok(()),
                session_id: live.session_id(),
                not_started: None,
            });
            return;
        }
        let result = live.send(&text).await;
        self.queue.push_end(TurnEnd {
            result,
            session_id: live.session_id(),
            not_started: None,
        });
    }

    /// The open session, opening one (checkout, token, CLI) when there is none.
    async fn ensure_live(self: &Arc<Self>) -> Result<Arc<Box<dyn LiveSession>>, Failure> {
        {
            let used = self.shared.clock.fetch_add(1, Ordering::SeqCst);
            let mut s = self.lock();
            if let Some(l) = s.live.as_mut() {
                l.used = used;
                return Ok(Arc::clone(&l.session));
            }
        }
        let harness = self
            .shared
            .engine
            .harnesses()
            .await
            .into_iter()
            .find(|h| h.kind == self.harness);
        match &harness {
            Some(h) if h.ready => {}
            Some(h) => {
                return Err(Failure::Unavailable(format!(
                    "{} is not ready: {}",
                    self.harness.label(),
                    h.hint
                )));
            }
            None => {
                return Err(Failure::Unavailable(format!(
                    "{} was not found on this Mac.",
                    self.harness.label()
                )));
            }
        }
        self.lock().billing = harness.as_ref().and_then(|h| match h.auth {
            HarnessAuth::SignedIn => Some(HarnessBilling::Subscription),
            HarnessAuth::ApiKey => Some(HarnessBilling::ApiKey),
            HarnessAuth::SignedOut => None,
        });

        // the checkout, once per chat
        let existing = self.lock().workdir.clone();
        let workdir = match existing {
            Some(w) => w,
            None => {
                let step = self.repo.as_ref().map(|r| format!("Reading {r}"));
                if let Some(label) = &step {
                    self.tool_step("checkout", "checkout", label, ChatToolState::Running);
                }
                match self
                    .shared
                    .engine
                    .prepare_workdir(&self.id, self.repo.as_deref())
                    .await
                {
                    Ok(w) => {
                        if let Some(label) = &step {
                            self.tool_step("checkout", "checkout", label, ChatToolState::Done);
                        }
                        self.lock().workdir = Some(w.clone());
                        w
                    }
                    Err(e) => {
                        if let Some(label) = &step {
                            self.tool_step("checkout", "checkout", label, ChatToolState::Failed);
                        }
                        let why = e.to_string();
                        return Err(Failure::Failed(format!(
                            "Ratchet could not read {}: {}",
                            self.repo.as_deref().unwrap_or("the repository"),
                            why.lines().next().unwrap_or("")
                        )));
                    }
                }
            }
        };

        let issued = self.shared.mcp.sessions.issue(
            ToolSet::Chat,
            Scope {
                repo: self.repo.clone(),
                optimization: self.optimization.clone(),
                agent: None,
            },
        );
        lock(&self.shared.by_session).insert(issued.id.clone(), self.id.clone());
        let socket = lock(&self.shared.socket).clone();
        let mut spec = SessionSpec {
            workdir: workdir.display().to_string(),
            prompt: String::new(),
            model: self.model.clone(),
            access: ratchet_core::ports::Access::ReadOnly,
            system_append: Some(role_prompt(self.repo.as_deref(), self.optimization.as_deref())),
            resume: self.lock().cli_session.clone(),
            ..SessionSpec::default()
        };
        ratchet_mcp::launch::attach(
            &mut spec,
            ratchet_mcp::launch::ratchetd_server(
                &self.shared.opts.ratchetd,
                &issued.token,
                socket.as_deref(),
            ),
        );
        let queue = Arc::clone(&self.queue);
        let sink: SharedEventSink = Arc::new(move |ev: AgentEvent| queue.push_agent(ev));
        match self.shared.engine.open(self.harness, spec, sink).await {
            Ok(session) => {
                let session: Arc<Box<dyn LiveSession>> = Arc::new(session);
                if self.closed.load(Ordering::SeqCst) {
                    // the chat was deleted while the CLI started
                    self.shared.mcp.sessions.revoke(&issued.id);
                    lock(&self.shared.by_session).remove(&issued.id);
                    if let Ok(b) = Arc::try_unwrap(session) {
                        b.close().await;
                    }
                    return Err(Failure::Failed("this chat was deleted".into()));
                }
                let used = self.shared.clock.fetch_add(1, Ordering::SeqCst);
                let mut s = self.lock();
                s.live = Some(Live {
                    session: Arc::clone(&session),
                    mcp_id: issued.id,
                    used,
                });
                Ok(session)
            }
            Err(e) => {
                self.shared.mcp.sessions.revoke(&issued.id);
                lock(&self.shared.by_session).remove(&issued.id);
                Err(Failure::Failed(format!(
                    "{} did not start: {}",
                    self.harness.label(),
                    e.to_string().lines().next().unwrap_or("")
                )))
            }
        }
    }

    fn tool_step(&self, id: &str, name: &str, summary: &str, state: ChatToolState) {
        self.publish(ChatStreamEvent::Tool {
            tool: ChatToolVm {
                id: id.to_owned(),
                name: name.to_owned(),
                state,
                summary: summary.to_owned(),
            },
        });
    }

    async fn interrupt(&self) {
        let live = {
            let mut s = self.lock();
            if !s.replying {
                return;
            }
            s.interrupted = true;
            s.live.as_ref().map(|l| Arc::clone(&l.session))
        };
        if let Some(session) = live
            && let Err(e) = session.interrupt().await
        {
            tracing::warn!("chat {}: interrupt failed: {e}", self.id);
        }
    }

    /// End the live session: remember its CLI session for resume, revoke its token, stop the process.
    async fn close_live(&self) {
        let live = self.lock().live.take();
        let Some(live) = live else { return };
        if let Some(sid) = live.session.session_id() {
            self.remember_cli_session(&sid).await;
        }
        self.shared.mcp.sessions.revoke(&live.mcp_id);
        lock(&self.shared.by_session).remove(&live.mcp_id);
        let mut arc = live.session;
        // a turn that is still ending holds a reference for a moment
        for _ in 0..100 {
            match Arc::try_unwrap(arc) {
                Ok(session) => {
                    session.close().await;
                    return;
                }
                Err(a) => {
                    arc = a;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
        tracing::warn!("chat {}: a session would not close; dropping it", self.id);
    }

    async fn remember_cli_session(&self, sid: &str) {
        let changed = {
            let mut s = self.lock();
            let changed = s.cli_session.as_deref() != Some(sid);
            if changed {
                s.cli_session = Some(sid.to_owned());
            }
            changed
        };
        if changed
            && let Err(e) = self
                .shared
                .ledger
                .set_chat_cli_session(&self.id, Some(sid))
                .await
        {
            parse_ledger(e, "cannot store the CLI session");
        }
    }

    /// After a turn: close the session when the chat stays quiet.
    fn schedule_idle_close(self: &Arc<Self>) {
        let gen_now = {
            let mut s = self.lock();
            s.idle_gen += 1;
            s.idle_gen
        };
        // weak: a deleted chat must not stay alive (and its stream open) until the timer fires
        let chat = Arc::downgrade(self);
        let idle = self.shared.opts.idle;
        tokio::spawn(async move {
            tokio::time::sleep(idle).await;
            let Some(chat) = chat.upgrade() else { return };
            let still_idle = {
                let s = chat.lock();
                s.idle_gen == gen_now && !s.replying
            };
            if still_idle && !chat.closed.load(Ordering::SeqCst) {
                chat.close_live().await;
            }
        });
    }

    // -- the pump --

    async fn run_pump(self: Arc<Self>) {
        while let Some(batch) = self.queue.next_batch().await {
            for item in batch {
                match item {
                    Item::Agent(ev) => self.on_agent_event(ev).await,
                    Item::End(end) => self.finish_turn(end).await,
                }
            }
        }
    }

    async fn on_agent_event(self: &Arc<Self>, ev: AgentEvent) {
        match ev.event_type {
            AgentEventType::Delta => {
                let Some(text) = ev.text.filter(|t| !t.is_empty()) else {
                    return;
                };
                {
                    let mut s = self.lock();
                    if s.pending.len() + text.len() <= self.shared.opts.pending_cap {
                        s.pending.push_str(&text);
                    }
                }
                self.publish(ChatStreamEvent::Delta { text });
            }
            AgentEventType::Thinking => {
                if let Some(text) = ev.text.filter(|t| !t.is_empty()) {
                    self.publish(ChatStreamEvent::Thinking { text });
                }
            }
            AgentEventType::Message => {
                let Some(text) = ev.text.filter(|t| !t.trim().is_empty()) else {
                    return;
                };
                self.reply(&text).await;
            }
            AgentEventType::Tool => self.on_tool_event(&ev).await,
            AgentEventType::Usage => {
                let Some(u) = ev.usage else { return };
                let total = {
                    let mut s = self.lock();
                    s.usage.input_tokens += u.input_tokens;
                    s.usage.output_tokens += u.output_tokens;
                    if let Some(c) = u.cost_usd {
                        *s.usage.cost_usd.get_or_insert(0.0) += c;
                    }
                    s.usage.billing = s.billing;
                    // a subscription costs no extra dollars
                    if s.billing == Some(HarnessBilling::Subscription) {
                        s.usage.cost_usd = Some(0.0);
                    }
                    s.usage
                };
                let billing = total.billing;
                self.publish(ChatStreamEvent::Usage {
                    usage: ChatUsageVm {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        cost_usd: if billing == Some(HarnessBilling::Subscription) {
                            Some(0.0)
                        } else {
                            u.cost_usd
                        },
                        billing,
                    },
                });
            }
            AgentEventType::Status => {
                if let Some(m) = ev.data_str("model").filter(|m| !m.is_empty()) {
                    self.lock().model_seen = Some(m.to_owned());
                }
            }
            AgentEventType::RateLimited => {
                let resets = ev
                    .data
                    .as_ref()
                    .and_then(|d| d.get("resetsAt"))
                    .and_then(Value::as_i64)
                    .map(|s| s * 1000);
                self.lock().failure = Some(Failure::RateLimited {
                    resets_at_ms: resets,
                });
            }
            AgentEventType::AuthExpired => {
                self.lock().failure = Some(Failure::AuthExpired);
            }
            AgentEventType::Error => {
                let mut s = self.lock();
                if s.failure.is_none() {
                    s.failure = Some(Failure::Failed(
                        ev.text.as_deref().unwrap_or("").lines().next().unwrap_or("").to_owned(),
                    ));
                }
            }
            AgentEventType::Commit | AgentEventType::Verdict => {}
        }
    }

    /// A finished reply: stored once, named with its harness and model.
    async fn reply(self: &Arc<Self>, text: &str) {
        let model = self.lock().model_seen.clone().or_else(|| self.model.clone());
        let row = self
            .append(NewChatMessage {
                role: "assistant".into(),
                text: cut(text, MAX_STORED_CHARS),
                harness: Some(self.harness.id().to_owned()),
                model,
                ..NewChatMessage::default()
            })
            .await;
        let Some(row) = row else { return };
        {
            let mut s = self.lock();
            s.pending.clear();
            s.last_reply_seq = Some(row.seq);
        }
        self.publish(ChatStreamEvent::Message {
            message: message_vm(&row),
        });
    }

    async fn on_tool_event(self: &Arc<Self>, ev: &AgentEvent) {
        let phase = ev.data_str("phase").unwrap_or("call");
        let id = ev.data_str("id").unwrap_or("").to_owned();
        if id.is_empty() {
            return;
        }
        if phase == "result" {
            let is_error = ev
                .data
                .as_ref()
                .and_then(|d| d.get("isError"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let seq = self.lock().tool_rows.get(&id).copied();
            let Some(seq) = seq else { return };
            let found = self.shared.ledger.find_chat_card(&self.id, &id).await;
            let Ok(Some(row)) = found else { return };
            let Some(mut tool) = row
                .card
                .and_then(|c| serde_json::from_value::<ChatToolVm>(c).ok())
            else {
                return;
            };
            tool.state = if is_error {
                ChatToolState::Failed
            } else {
                ChatToolState::Done
            };
            if let Ok(v) = serde_json::to_value(&tool)
                && let Err(e) = self
                    .shared
                    .ledger
                    .set_chat_message_card(&self.id, seq, &v)
                    .await
            {
                parse_ledger(e, "cannot update a tool step");
            }
            self.publish(ChatStreamEvent::Tool { tool });
            return;
        }
        let name = cards::tool_name(ev.data_str("name").unwrap_or("tool"));
        let summary = cut(
            ev.text
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map_or_else(|| cards::tool_label(&name), str::to_owned)
                .as_str(),
            MAX_SUMMARY_CHARS,
        );
        let tool = ChatToolVm {
            id: id.clone(),
            name,
            state: ChatToolState::Running,
            summary: summary.clone(),
        };
        let payload = serde_json::to_value(&tool).ok();
        if let Some(row) = self
            .append(NewChatMessage {
                role: "tool".into(),
                text: summary,
                card: payload,
                ..NewChatMessage::default()
            })
            .await
        {
            self.lock().tool_rows.insert(id, row.seq);
        }
        self.publish(ChatStreamEvent::Tool { tool });
    }

    /// The end of a turn: what was half written is kept, the failure (if any) is told, `done` closes the turn.
    async fn finish_turn(self: &Arc<Self>, end: TurnEnd) {
        let partial = std::mem::take(&mut self.lock().pending);
        if !partial.trim().is_empty() {
            // deltas that never became a message (the reply was cut short)
            self.reply(&partial).await;
        }
        if let Some(sid) = &end.session_id {
            self.remember_cli_session(sid).await;
        }
        let (interrupted, usage, last_reply, turn, recorded) = {
            let s = self.lock();
            (
                s.interrupted,
                s.usage,
                s.last_reply_seq,
                s.turn,
                s.failure.clone(),
            )
        };
        let failure = if interrupted {
            None
        } else if let Some(f) = end.not_started.clone() {
            Some(f)
        } else {
            match &end.result {
                Ok(()) => None,
                Err(PortError::Cancelled) => None,
                Err(e) => Some(recorded.unwrap_or_else(|| Failure::from_port(e))),
            }
        };
        if let (Some(seq), true) = (last_reply, usage != ChatUsageVm::default())
            && let Ok(v) = serde_json::to_value(usage)
            && let Err(e) = self
                .shared
                .ledger
                .set_chat_message_usage(&self.id, seq, &v)
                .await
        {
            parse_ledger(e, "cannot store the usage");
        }
        let outcome = if interrupted {
            ChatOutcome::Interrupted
        } else if failure.is_some() {
            ChatOutcome::Failed
        } else {
            ChatOutcome::Completed
        };
        if let Some(f) = &failure {
            let others: Vec<HarnessKind> = self
                .shared
                .engine
                .harnesses()
                .await
                .into_iter()
                .filter(|h| h.ready && h.kind != self.harness)
                .map(|h| h.kind)
                .collect();
            self.publish(ChatStreamEvent::Error {
                error: cards::error_for(self.harness, f, &others, now_ms()),
            });
            // a CLI that failed may be gone: the next message opens a fresh session (with resume)
            self.close_live().await;
        }
        let last_seq = self
            .shared
            .ledger
            .last_chat_seq(&self.id)
            .await
            .unwrap_or_default();
        self.publish(ChatStreamEvent::Done {
            turn,
            outcome,
            last_seq,
            usage,
        });
        {
            let mut s = self.lock();
            s.replying = false;
            s.interrupted = false;
            s.failure = None;
            s.tool_rows.clear();
        }
        if !self.closed.load(Ordering::SeqCst) {
            self.schedule_idle_close();
        }
    }

    // -- cards --

    async fn on_tool(
        self: &Arc<Self>,
        tool: &str,
        arguments: &Value,
        outcome: &Result<Value, ServiceError>,
        ready: &[HarnessKind],
    ) {
        match (tool, outcome) {
            ("draft_optimization", Ok(result)) => {
                match serde_json::from_value::<cards::DraftFields>(result.clone()) {
                    Ok(d) => self.draft_card(&d, ready).await,
                    Err(e) => parse_ledger(e, "an unreadable draft"),
                }
            }
            ("create_optimization", Ok(result)) => {
                let draft = result["draftId"].as_str().unwrap_or_default().to_owned();
                let optimization = result["optimization"].as_str().map(str::to_owned);
                self.update_draft(&draft, |c| {
                    c.status = ChatCardStatus::Created;
                    c.optimization = optimization.clone();
                    c.error = None;
                })
                .await;
            }
            ("create_optimization", Err(e)) => {
                let draft = arguments["draft_id"].as_str().unwrap_or_default().to_owned();
                let message = e.to_string();
                self.update_draft(&draft, |c| {
                    c.status = ChatCardStatus::Failed;
                    c.error = Some(CardErrorVm {
                        title: "Couldn't create the optimization".into(),
                        message: message.clone(),
                    });
                })
                .await;
            }
            ("add_steering_note", Ok(_)) => {
                let optimization = arguments["optimization"].as_str().unwrap_or_default();
                let text = arguments["text"].as_str().unwrap_or_default();
                let note_id = format!("note_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
                let card = cards::note_card(&note_id, optimization, text, ChatCardStatus::Queued);
                let row = self
                    .append(NewChatMessage {
                        role: "note".into(),
                        text: cut(text, MAX_STORED_CHARS),
                        card: serde_json::to_value(&card).ok(),
                        ..NewChatMessage::default()
                    })
                    .await;
                if let Some(row) = row {
                    lock(&self.shared.notes).push(PendingNote {
                        chat: self.id.clone(),
                        note: note_id,
                        optimization: optimization.to_owned(),
                    });
                    self.publish(ChatStreamEvent::Note {
                        seq: row.seq,
                        card,
                    });
                }
            }
            _ => {}
        }
    }

    async fn draft_card(self: &Arc<Self>, d: &cards::DraftFields, ready: &[HarnessKind]) {
        let card = cards::draft_card(d, ready);
        let previous = self.lock().open_draft.take();
        let mut replaces = None;
        if let Some((prev_id, prev_seq)) = previous
            && prev_id != d.draft_id
        {
            self.update_card_at(&prev_id, prev_seq, |c| c.status = ChatCardStatus::Replaced)
                .await;
            replaces = Some(prev_id);
        }
        let row = self
            .append(NewChatMessage {
                role: "card".into(),
                text: card.title.clone(),
                card: serde_json::to_value(&card).ok(),
                ..NewChatMessage::default()
            })
            .await;
        let Some(row) = row else { return };
        self.lock().open_draft = Some((d.draft_id.clone(), row.seq));
        self.publish(ChatStreamEvent::Card {
            seq: row.seq,
            card,
            replaces,
        });
    }

    async fn update_draft(self: &Arc<Self>, draft: &str, f: impl FnOnce(&mut ChatCardVm)) {
        let Ok(Some(row)) = self.shared.ledger.find_chat_card(&self.id, draft).await else {
            return;
        };
        self.update_card_at(draft, row.seq, f).await;
        let mut s = self.lock();
        if s.open_draft.as_ref().is_some_and(|(id, _)| id == draft) {
            s.open_draft = None;
        }
    }

    /// Change a stored card and tell the streams (`card` with the same id: an update).
    async fn update_card_at(&self, id: &str, seq: i64, f: impl FnOnce(&mut ChatCardVm)) {
        let Ok(Some(row)) = self.shared.ledger.find_chat_card(&self.id, id).await else {
            return;
        };
        let Some(mut card) = row
            .card
            .and_then(|c| serde_json::from_value::<ChatCardVm>(c).ok())
        else {
            return;
        };
        f(&mut card);
        if let Ok(v) = serde_json::to_value(&card)
            && let Err(e) = self
                .shared
                .ledger
                .set_chat_message_card(&self.id, seq, &v)
                .await
        {
            parse_ledger(e, "cannot update a card");
        }
        let ev = if card.kind == ChatCardKind::Note {
            ChatStreamEvent::Note { seq, card }
        } else {
            ChatStreamEvent::Card {
                seq,
                card,
                replaces: None,
            }
        };
        self.publish(ev);
    }

    async fn note_read(self: &Arc<Self>, note: &str, by: &str, at: i64) {
        let Ok(Some(row)) = self.shared.ledger.find_chat_card(&self.id, note).await else {
            return;
        };
        let by = by.to_owned();
        self.update_card_at(note, row.seq, |c| {
            c.status = ChatCardStatus::Read;
            c.read_by = Some(by.clone());
            c.read_at = Some(at);
        })
        .await;
    }
}

// ---- rows → view models --------------------------------------------------------------------------------------------

fn message_vm(row: &ChatMessageRow) -> ChatMessageVm {
    let role = ChatRole::parse(&row.role).unwrap_or(ChatRole::Assistant);
    let payload = row.card.clone();
    ChatMessageVm {
        seq: row.seq,
        role,
        text: row.text.clone(),
        harness: row.harness.as_deref().and_then(HarnessKind::from_id),
        model: row.model.clone(),
        card: match role {
            ChatRole::Card | ChatRole::Note => payload
                .clone()
                .and_then(|c| serde_json::from_value(c).ok()),
            _ => None,
        },
        tool: match role {
            ChatRole::Tool => payload.and_then(|c| serde_json::from_value(c).ok()),
            _ => None,
        },
        usage: row
            .usage
            .clone()
            .and_then(|u| serde_json::from_value(u).ok()),
        at: row.at,
    }
}
