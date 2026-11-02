//! What the chat service needs from the machine it runs on: which harnesses can chat, a working directory per chat, and a live
//! session. The real engine drives the user's CLIs (`crate::real::chat`); the scripted one ([`super::scripted`]) needs nothing.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use ratchet_core::ports::{LiveSession, PortResult, SessionSpec, SharedEventSink};
use serde_json::Value;

use crate::api::{HarnessAuth, HarnessKind};

/// One harness as chat sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatHarness {
    pub kind: HarnessKind,
    /// Installed and signed in (or keyed): a chat can start on it.
    pub ready: bool,
    pub auth: HarnessAuth,
    /// What to do next when it is not ready ("Sign in to Codex: run `codex login`").
    pub hint: String,
    /// The model chat uses when the request names none (the catalog's chat default).
    pub default_model: Option<String>,
}

/// The harness layer behind the chat service.
#[async_trait]
pub trait ChatEngine: Send + Sync {
    /// Every harness, in the order chat prefers them (Claude Code, Codex, Grok).
    async fn harnesses(&self) -> Vec<ChatHarness>;

    /// The directory a chat's CLI runs in. With a repo: a read-only checkout of its default branch, one per chat. Without (a Home
    /// chat): an empty scratch directory, so the CLI has no files to read. Called once per chat; the directory stays until
    /// [`release_workdir`](Self::release_workdir).
    async fn prepare_workdir(&self, chat_id: &str, repo: Option<&str>) -> PortResult<PathBuf>;

    /// Remove what `prepare_workdir` made. Never fails.
    async fn release_workdir(&self, chat_id: &str);

    /// Called once when the service is made, with the way to call Ratchet's tools through the daemon. Only the scripted engine uses
    /// it (a real CLI reaches the tools through `ratchetd mcp`).
    fn attach_tools(&self, _caller: Arc<dyn ChatToolCaller>) {}

    /// Open a session on `kind`. `spec.resume` continues an earlier CLI session; the first message is not part of `open` (the
    /// service sends it, so `interrupt` works from the first moment).
    async fn open(
        &self,
        kind: HarnessKind,
        spec: SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>>;
}

/// What a session is bound to, as the MCP server knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolScope {
    pub repo: Option<String>,
    pub optimization: Option<String>,
}

/// How a harness that is not a real CLI (the scripted one) calls Ratchet's tools: through the daemon, with its session token,
/// exactly as `ratchetd mcp` does.
#[async_trait]
pub trait ChatToolCaller: Send + Sync {
    async fn call(&self, token: &str, tool: &str, arguments: Value) -> Result<Value, String>;
    async fn scope(&self, token: &str) -> Option<ToolScope>;
}

/// The ratchet MCP server's token in a spec: the argument after `--session`.
pub fn session_token(spec: &SessionSpec) -> Option<String> {
    let server = spec.mcp.iter().find(|s| s.name == "ratchet")?;
    let at = server.args.iter().position(|a| a == "--session")?;
    server.args.get(at + 1).cloned()
}

pub type SharedChatEngine = Arc<dyn ChatEngine>;
