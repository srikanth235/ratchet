//! Shared helpers of the ledger tests.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use ratchet_core::enums::{AgentKind, Direction};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::{CancellationToken, Clock, PortResult};
use ratchet_core::rows::{Agent, BenchStats, Optimization};
use ratchet_ledger::{
    AddAgentInput, CreateOptimizationInput, Ledger, LedgerOptions, RecordBaselineInput,
};

/// `let t = 1000; clock = () => ++t;` of the TypeScript tests.
pub struct TickClock(AtomicI64);

impl TickClock {
    pub fn new(start: i64) -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(start)))
    }
}

#[async_trait]
impl Clock for TickClock {
    fn now(&self) -> i64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    async fn sleep(&self, _ms: u64, _cancel: Option<&CancellationToken>) -> PortResult<()> {
        Ok(())
    }
}

/// An in-memory ledger whose clock ticks 1001, 1002, ...
pub async fn ledger() -> Ledger {
    Ledger::open_memory(LedgerOptions {
        clock: Some(TickClock::new(1_000)),
        max_connections: None,
    })
    .await
    .unwrap()
}

pub fn opt_input() -> CreateOptimizationInput {
    CreateOptimizationInput::new(
        "acme/search",
        "main",
        "http_req_duration.p95",
        Direction::Minimize,
        0.03,
        100.0,
        3600,
        "version: 0\n",
    )
}

pub async fn mk_opt(l: &Ledger) -> Optimization {
    l.create_optimization(opt_input()).await.unwrap()
}

pub async fn mk_baseline(l: &Ledger, id: &OptimizationId, value: f64) {
    l.record_baseline(RecordBaselineInput {
        optimization_id: id.clone(),
        samples: vec![value - 1.0, value, value + 1.0],
        noise_floor: 0.02,
        stats: BenchStats::new(value)
            .with("mean", value)
            .with("stddev", 1.0),
        value: None,
        recorded_at: None,
    })
    .await
    .unwrap();
}

pub async fn mk_candidate(
    l: &Ledger,
    opt: &OptimizationId,
    agent: &AgentId,
    sha: &str,
) -> ratchet_core::rows::Candidate {
    let mut input = ratchet_ledger::SubmitCandidateInput::new(opt.clone(), agent.clone(), sha);
    input.message = Some(format!("msg {sha}"));
    l.submit_candidate(input).await.unwrap()
}

pub struct Setup {
    pub opt: Optimization,
    pub claude: Agent,
    pub codex: Agent,
}

pub async fn setup_with(l: &Ledger, input: CreateOptimizationInput) -> Setup {
    let opt = l.create_optimization(input).await.unwrap();
    mk_baseline(l, &opt.id, 100.0).await;
    let claude = l
        .add_agent(AddAgentInput::new(
            opt.id.clone(),
            AgentKind::ClaudeCode,
            format!("ratchet/{}/claude", opt.id),
        ))
        .await
        .unwrap();
    let codex = l
        .add_agent(AddAgentInput::new(
            opt.id.clone(),
            AgentKind::Codex,
            format!("ratchet/{}/codex", opt.id),
        ))
        .await
        .unwrap();
    Setup { opt, claude, codex }
}

pub async fn setup(l: &Ledger) -> Setup {
    setup_with(l, opt_input()).await
}

/// The error text of a failed ledger call.
pub fn msg<T: std::fmt::Debug>(r: Result<T, ratchet_core::LedgerError>) -> String {
    r.expect_err("expected an error").to_string()
}
