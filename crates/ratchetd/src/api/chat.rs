//! Chat (P4): a conversation with the user's own agent CLI that acts on Ratchet only through Ratchet's tools.
//!
//! A chat is made once (`POST /v1/chats`), talked to (`POST /v1/chats/{id}/messages`, which answers 202 at once) and followed on
//! its own server-sent-event stream (`GET /v1/chats/{id}/stream`). The stream is per chat: the app's one `/v1/events` stream
//! never carries chat text. Every reply names the harness and model that wrote it.
//!
//! Stream events, in the order a turn produces them: `message` (the user's text, echoed), then `delta`s and `thinking`, `tool`
//! steps, `message` (each finished reply), `card` (a draft the chat proposed) and `note` (a steering note and whether it was read),
//! `usage`, and last `done`. A turn that cannot finish ends with `error` and then `done`. `ready` is the first event of every
//! connection. Each event has an `id:` that counts up per chat; reconnect with `Last-Event-ID` to catch up.

use ratchet_core::enums::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use super::settings::{HarnessBilling, HarnessKind};

/// Longest message a chat takes, in characters.
pub const MAX_CHAT_MESSAGE_CHARS: usize = 8000;

impl HarnessKind {
    /// The agent kind the engine and ledger use for this harness.
    pub fn agent_kind(self) -> AgentKind {
        match self {
            Self::ClaudeCode => AgentKind::ClaudeCode,
            Self::Codex => AgentKind::Codex,
            Self::Grok => AgentKind::Grok,
        }
    }

    pub fn from_agent_kind(kind: AgentKind) -> Option<Self> {
        match kind {
            AgentKind::ClaudeCode => Some(Self::ClaudeCode),
            AgentKind::Codex => Some(Self::Codex),
            AgentKind::Grok => Some(Self::Grok),
            AgentKind::Mock => None,
        }
    }

    /// The id used in the ledger, the API and the CLI (`claude-code`, `codex`, `grok`).
    pub fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "claude-code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }

    /// The name people read.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
        }
    }
}

/// Body of `POST /v1/chats`. Every field is optional: an empty body starts a Home chat with no repo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateChatInput {
    /// `owner/name`: the chat reads this repo (a read-only checkout of its default branch) and may only draft for it.
    #[serde(default)]
    #[schema(nullable = false)]
    pub repo: Option<String>,
    /// Steering: the chat is bound to this optimization, sees its context and gets the `add_steering_note` tool.
    #[serde(default)]
    #[schema(nullable = false)]
    pub optimization: Option<String>,
    /// Default: the user's chat harness, else the first signed-in one (Claude Code, then Codex, then Grok).
    #[serde(default)]
    #[schema(nullable = false)]
    pub harness: Option<HarnessKind>,
    /// A model id or alias of the harness (`GET /v1/harnesses`). Default: the catalog's chat model.
    #[serde(default)]
    #[schema(nullable = false)]
    pub model: Option<String>,
}

/// Body of `POST /v1/chats/{id}/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SendChatMessageInput {
    /// 1 to 8000 characters.
    pub text: String,
}

/// Answer of `POST /v1/chats/{id}/messages`: the message was stored and the reply is on its way (follow the stream).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageAccepted {
    /// The user's message.
    pub seq: i64,
    /// 1 for the first turn of the chat, then 2, 3, ...; `done` carries the same number.
    pub turn: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
    /// A step the agent took with a tool (reading a file, drafting): the "Read acme/shop main" lines.
    Tool,
    /// A card the chat proposed (a draft optimization).
    Card,
    /// A steering note and whether an agent read it.
    Note,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Card => "card",
            Self::Note => "note",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "tool" => Self::Tool,
            "card" => Self::Card,
            "note" => Self::Note,
            _ => return None,
        })
    }
}

/// Whether the chat is answering right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatState {
    /// Waiting for the user's next message.
    Idle,
    /// A reply is being written; `interrupt` stops it.
    Replying,
}

/// Tokens and dollars of a turn or a reply. `costUsd` is absent when the CLI reports tokens only; on a subscription login it is 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsageVm {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub cost_usd: Option<f64>,
    /// How the tokens were paid for: the user's plan or an API key. Absent while unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub billing: Option<HarnessBilling>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatToolState {
    Running,
    Done,
    Failed,
}

/// One step of the agent: a line of the reply's "what it did" list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatToolVm {
    /// The CLI's id of the call; the `running` and `done` events of one call share it.
    pub id: String,
    /// The tool, e.g. `draft_optimization` (`mcp__ratchet__` removed) or `Read`.
    pub name: String,
    pub state: ChatToolState,
    /// One line to show, e.g. "Read RATCHET.md". Short; never a credential.
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatCardKind {
    /// A draft optimization (`draft_optimization`): the summary card with Create and Change buttons.
    Draft,
    /// A steering note.
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatCardStatus {
    /// A draft the user has not created yet.
    Draft,
    /// The optimization was created (and started): `optimization` says which.
    Created,
    /// Creating failed: see `error`.
    Failed,
    /// A newer draft replaced this one.
    Replaced,
    /// A steering note waiting for an agent's next turn.
    Queued,
    /// A steering note an agent has taken up.
    Read,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CardOptionVm {
    /// What to send back: the value of the row after the change.
    #[schema(value_type = Object)]
    pub value: Value,
    pub label: String,
}

/// One line of a card: "Measures", "Agents", "Each run", "Schedule", "Output", "Config". The UI shows `label` and `value` and,
/// when `editable`, a Change button whose popover is built from `options` (or a free input of the type of `raw`). A change is
/// sent as a chat message ("Change the budget to $20"): the chat drafts again and the card updates in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CardRowVm {
    /// Stable name: `goal`, `repo`, `measure`, `agents`, `limits`, `schedule`, `output`, `config`.
    pub key: String,
    pub label: String,
    /// The line as shown.
    pub value: String,
    pub editable: bool,
    /// The row's value as data (a number, a list, an object), for an editor.
    #[schema(value_type = Object)]
    pub raw: Value,
    /// Offered choices; empty for a free value.
    #[serde(default)]
    pub options: Vec<CardOptionVm>,
}

/// Something went wrong with a card, with the ways out ("Use Smaller Setup", "Try Again").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CardErrorVm {
    pub title: String,
    pub message: String,
}

/// The card the chat shows for a draft optimization (or a steering note). The same `id` arriving again is an update: replace the
/// card you have (`card` events carry `replaces` when a new draft takes the place of an older one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatCardVm {
    /// The draft id (`draft_...`) or note id (`note_...`).
    pub id: String,
    pub kind: ChatCardKind,
    pub status: ChatCardStatus,
    /// "Checkout p95" for a draft; the note's text for a note.
    pub title: String,
    /// "up to $50 and 2 h per run" for a draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub repo: Option<String>,
    #[serde(default)]
    pub rows: Vec<CardRowVm>,
    /// Set once the draft became an optimization, or on a note: the optimization it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub optimization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<CardErrorVm>,
    /// A note: the agent that read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub read_by: Option<String>,
    /// A note: when it was read (epoch ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub read_at: Option<i64>,
}

/// One message of a chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageVm {
    /// 1, 2, 3, ... per chat: the order of the conversation.
    pub seq: i64,
    pub role: ChatRole,
    /// The text; for a tool step its one-line summary.
    pub text: String,
    /// Who wrote an assistant reply ("Written by Claude Code · Opus").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub harness: Option<HarnessKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub card: Option<ChatCardVm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub tool: Option<ChatToolVm>,
    /// What the turn cost; set on the last reply of a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub usage: Option<ChatUsageVm>,
    /// Epoch ms.
    pub at: i64,
}

/// The reply being written when the client asks: text not yet part of a `message`, so a client that joins late does not miss it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingReplyVm {
    pub text: String,
}

/// The first-use notice: what leaves this machine when you chat. Show it once, dismissible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataNoticeVm {
    pub harness: HarnessKind,
    /// "First time: your messages, the repo name and the files it reads go to Claude Code, signed in on this Mac. Ratchet sends nothing else."
    pub text: String,
}

/// A chat, with its messages (`GET`) or without (`POST`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatVm {
    pub id: String,
    #[schema(required = true)]
    pub repo: Option<String>,
    #[schema(required = true)]
    pub optimization: Option<String>,
    pub harness: HarnessKind,
    /// The model the chat asked for (the catalog default when the request named none); `null` when the CLI chooses.
    #[schema(required = true)]
    pub model: Option<String>,
    pub state: ChatState,
    /// A live CLI session is open (it closes after 10 minutes without a message and reopens with the next one).
    pub live: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// Set while this is the first chat with this harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub data_notice: Option<DataNoticeVm>,
    /// Oldest first. Only on `GET`, and at most `limit` of the messages after `after`.
    #[serde(default)]
    pub messages: Vec<ChatMessageVm>,
    /// More messages follow the last one here (ask again with `after`).
    #[serde(default)]
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub pending: Option<PendingReplyVm>,
    /// Id of the newest stream event so far, for `Last-Event-ID`.
    pub last_event_id: u64,
}

/// Query of `GET /v1/chats/{id}`.
#[derive(Debug, Clone, Default, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ChatQuery {
    /// Only messages after this `seq` (default all).
    pub after: Option<i64>,
    /// 1 to 1000; default 200.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatErrorCode {
    /// The plan's quota ran out. `retry.resetsAt` says when it lifts, when the CLI knows.
    RateLimited,
    /// The login or key was rejected: sign in again.
    AuthExpired,
    /// The CLI is not installed or not signed in, so no reply can be written.
    Unavailable,
    /// The CLI crashed or sent something unreadable.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ChatFix {
    /// "Try Again": send the same message once more.
    Retry,
    /// "Sign In": open Customize, Agents.
    SignIn,
    /// "Use Codex": start the chat again on another harness (`switchTo`).
    SwitchHarness,
}

/// When and whether trying again helps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatRetryVm {
    /// Epoch ms when the quota lifts; absent when the CLI did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub resets_at: Option<i64>,
    /// Sending the same message again can work right now.
    pub can_retry_now: bool,
}

/// What went wrong and what to press. `title` and `message` are written for people ("Claude Code reached its usage limit").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatErrorVm {
    pub code: ChatErrorCode,
    pub harness: HarnessKind,
    pub title: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub retry: Option<ChatRetryVm>,
    /// The buttons to offer, best first.
    #[serde(default)]
    pub fixes: Vec<ChatFix>,
    /// For `switch-harness`: a signed-in harness other than this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub switch_to: Option<HarnessKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatOutcome {
    Completed,
    /// The user pressed Stop.
    Interrupted,
    Failed,
}

/// Events of `GET /v1/chats/{id}/stream`. The SSE `event:` is the `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ChatStreamEvent {
    /// First event of a connection (no `id`). `resync` is true when the events since `Last-Event-ID` are gone: refetch the chat.
    Ready {
        chat_id: String,
        harness: HarnessKind,
        #[schema(required = true)]
        model: Option<String>,
        state: ChatState,
        /// The newest message `seq`.
        last_seq: i64,
        last_event_id: u64,
        resync: bool,
        /// The partial reply written so far, if one is in progress.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schema(nullable = false)]
        pending: Option<PendingReplyVm>,
    },
    /// A whole message: the user's (echoed so every client agrees), or a finished reply with its harness and model.
    Message { message: ChatMessageVm },
    /// Part of the reply being written. Append; the finished `message` repeats the whole text.
    Delta { text: String },
    /// Part of the reasoning summary (collapsed in the UI). Not stored.
    Thinking { text: String },
    /// A step with a tool: `running`, then `done` or `failed` with the same `tool.id`.
    Tool { tool: ChatToolVm },
    /// A draft card, new (`replaces` is the id of the draft it supersedes) or updated (same `card.id`).
    Card {
        seq: i64,
        card: ChatCardVm,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schema(nullable = false)]
        replaces: Option<String>,
    },
    /// A steering note: `queued` when sent, `read` once an agent took it up.
    Note { seq: i64, card: ChatCardVm },
    /// Tokens of the turn so far (incremental).
    Usage { usage: ChatUsageVm },
    /// The turn is over. Totals are for the turn.
    Done {
        turn: u64,
        outcome: ChatOutcome,
        last_seq: i64,
        usage: ChatUsageVm,
    },
    /// The turn failed; `done` follows with `outcome: failed`.
    Error { error: ChatErrorVm },
}

impl ChatStreamEvent {
    /// The SSE event name (the `type` tag).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::Message { .. } => "message",
            Self::Delta { .. } => "delta",
            Self::Thinking { .. } => "thinking",
            Self::Tool { .. } => "tool",
            Self::Card { .. } => "card",
            Self::Note { .. } => "note",
            Self::Usage { .. } => "usage",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
        }
    }
}
