#![allow(dead_code)]

pub use ratchet_core::enums::{AgentKind, AgentStatus, OptimizationStatus};
pub use ratchet_core::ids::OptimizationId;
pub use ratchet_core::ports::{AgentEvent, AgentEventType, EngineEvent, RunnerMessage};
pub use ratchet_engine::testing::*;
pub use ratchet_engine::{CreateOptimizationInput, EngineErrorCode};

pub use AgentKind::{ClaudeCode, Codex};

pub const CC: &str = "ratchet/opt-test/claude-code";
pub const CX: &str = "ratchet/opt-test/codex";

pub fn oid(id: &str) -> OptimizationId {
    OptimizationId::new(id)
}

/// `createOptimization` input with the smallest valid values.
pub fn small_input() -> CreateOptimizationInput {
    CreateOptimizationInput {
        repo_url: "x".into(),
        base_ref: "main".into(),
        agents: vec![Codex],
        budget_usd: 1.0,
        time_limit_seconds: 60,
        plateau_evals: 1,
        ..Default::default()
    }
}

pub fn begin_opts() -> BeginOptions {
    BeginOptions::default()
}

pub async fn begun(h: &Harness) -> Begun {
    h.begin(BeginOptions::default()).await.expect("begin")
}

pub fn done_event() -> AgentEvent {
    status_event(serde_json::json!({ "state": "done" }))
}

pub fn msg_event(text: &str) -> AgentEvent {
    ev(AgentEventType::Message).with_text(text)
}

pub async fn candidates(h: &Harness, id: &str) -> Vec<ratchet_core::rows::CandidateWithResults> {
    h.ledger.candidates_with_results(&oid(id)).await.unwrap()
}

pub async fn all_decided(h: &Harness, id: &str, n: usize) -> bool {
    let c = candidates(h, id).await;
    c.len() == n
        && c.iter()
            .all(|c| c.candidate.verdict != ratchet_core::enums::CandidateVerdict::Pending)
}

pub fn v(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect()
}

pub fn types(events: &[EngineEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            EngineEvent::Optimization { .. } => "optimization",
            EngineEvent::AgentEvent { .. } => "agent-event",
            EngineEvent::Candidate { .. } => "candidate",
            EngineEvent::Ratchet { .. } => "ratchet",
            EngineEvent::Spend { .. } => "spend",
            EngineEvent::Notice { .. } => "notice",
            EngineEvent::RunActive { .. } => "run-active",
            EngineEvent::Sandbox { .. } => "sandbox",
        })
        .collect()
}
