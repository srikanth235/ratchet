//! The event stream: `GET /v1/events` (server-sent events).
//!
//! The SSE `event:` field is the `type` below, `id:` is a per-boot sequence number (`u64`, from 1), `data:` is the
//! JSON of the whole [`ApiEvent`]. A client that reconnects sends `Last-Event-ID` and receives what it missed; if
//! the daemon no longer has it (or restarted), the first message is `resync` and the client refetches its
//! snapshots. The first message on every connection is `hello`.
//!
//! It is the union of `EngineEventVM` of the Electron app, plus `hello`, `resync` and the provisional
//! `environment-draft`, `connector` and `github`.

use ratchet_core::enums::{CandidateVerdict, OptimizationStatus};
use ratchet_core::ports::{NoticeLevel, SandboxPhase};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::connectors::ConnectorStatus;
use super::env_drafts::EnvironmentDraftStatus;
use super::vm::AgentEventVm;

/// One message of the stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ApiEvent {
    /// First message of a connection (no `id`).
    #[serde(rename_all = "camelCase")]
    Hello {
        /// Id of the newest event so far.
        last_event_id: u64,
        server_time: i64,
        version: String,
    },
    /// The client's `Last-Event-ID` is older than what the daemon kept (or from before a restart): refetch
    /// everything you show.
    Resync { reason: String },
    /// An optimization changed status.
    #[serde(rename_all = "camelCase")]
    Optimization {
        optimization_id: String,
        status: OptimizationStatus,
    },
    /// An agent logged something. `event` carries it so a log view can append without refetching.
    #[serde(rename_all = "camelCase")]
    AgentEvent {
        optimization_id: String,
        agent_id: String,
        #[schema(required = true)]
        event: Option<AgentEventVm>,
    },
    /// A candidate got its verdict.
    #[serde(rename_all = "camelCase")]
    Candidate {
        optimization_id: String,
        candidate_id: String,
        verdict: CandidateVerdict,
    },
    /// The ratchet moved: a verified win.
    #[serde(rename_all = "camelCase")]
    Ratchet {
        optimization_id: String,
        from: f64,
        to: f64,
    },
    #[serde(rename_all = "camelCase")]
    Spend {
        optimization_id: String,
        total_usd: f64,
    },
    #[serde(rename_all = "camelCase")]
    Notice {
        optimization_id: String,
        level: NoticeLevel,
        text: String,
    },
    /// True while any run is active (the app turns it into "prevent system sleep").
    #[serde(rename_all = "camelCase")]
    RunActive {
        active: bool,
        optimization_ids: Vec<String>,
    },
    /// The automatic environment changing phase.
    #[serde(rename_all = "camelCase")]
    Sandbox {
        sandbox_id: String,
        phase: SandboxPhase,
        #[schema(required = true)]
        region: Option<String>,
        text: String,
    },
    /// Provisional: an environment draft changed status.
    #[serde(rename_all = "camelCase")]
    EnvironmentDraft {
        draft_id: String,
        status: EnvironmentDraftStatus,
    },
    /// Provisional: a connector changed status (signed in, expired, removed).
    #[serde(rename_all = "camelCase")]
    Connector {
        connector_id: String,
        #[schema(required = true)]
        status: Option<ConnectorStatus>,
    },
    /// The GitHub sign-in changed (device flow finished, signed out).
    #[serde(rename_all = "camelCase")]
    Github { signed_in: bool },
}

impl ApiEvent {
    /// The SSE event name (the `type` tag).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Resync { .. } => "resync",
            Self::Optimization { .. } => "optimization",
            Self::AgentEvent { .. } => "agent-event",
            Self::Candidate { .. } => "candidate",
            Self::Ratchet { .. } => "ratchet",
            Self::Spend { .. } => "spend",
            Self::Notice { .. } => "notice",
            Self::RunActive { .. } => "run-active",
            Self::Sandbox { .. } => "sandbox",
            Self::EnvironmentDraft { .. } => "environment-draft",
            Self::Connector { .. } => "connector",
            Self::Github { .. } => "github",
        }
    }
}

/// An event with the id it was published under.
#[derive(Debug, Clone, PartialEq)]
pub struct Sequenced {
    pub id: u64,
    pub event: ApiEvent,
}

impl From<ratchet_core::ports::EngineEvent> for ApiEvent {
    /// The real engine's events, as the wire shows them.
    fn from(e: ratchet_core::ports::EngineEvent) -> Self {
        use ratchet_core::ports::EngineEvent as E;
        match e {
            E::Optimization {
                optimization_id,
                status,
            } => ApiEvent::Optimization {
                optimization_id: optimization_id.into_string(),
                status: status.parse().unwrap_or(OptimizationStatus::Draft),
            },
            E::AgentEvent {
                optimization_id,
                agent_id,
                ..
            } => ApiEvent::AgentEvent {
                optimization_id: optimization_id.into_string(),
                agent_id: agent_id.into_string(),
                event: None,
            },
            E::Candidate {
                optimization_id,
                candidate_id,
                verdict,
            } => ApiEvent::Candidate {
                optimization_id: optimization_id.into_string(),
                candidate_id: candidate_id.into_string(),
                verdict,
            },
            E::Ratchet {
                optimization_id,
                from,
                to,
            } => ApiEvent::Ratchet {
                optimization_id: optimization_id.into_string(),
                from,
                to,
            },
            E::Spend {
                optimization_id,
                total_usd,
            } => ApiEvent::Spend {
                optimization_id: optimization_id.into_string(),
                total_usd,
            },
            E::Notice {
                optimization_id,
                level,
                text,
            } => ApiEvent::Notice {
                optimization_id: optimization_id.into_string(),
                level,
                text,
            },
            E::RunActive {
                active,
                optimization_ids,
            } => ApiEvent::RunActive {
                active,
                optimization_ids: optimization_ids
                    .into_iter()
                    .map(|i| i.into_string())
                    .collect(),
            },
            E::Sandbox {
                sandbox_id,
                phase,
                region,
                text,
            } => ApiEvent::Sandbox {
                sandbox_id,
                phase,
                region,
                text,
            },
        }
    }
}
