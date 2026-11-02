//! The optimization's agents: which agent CLIs it may use, in which order, and the rules for moving between them.
//!
//! Behind the Agents row of the optimization settings ("Claude Code → Codex → Grok"). The first agent runs; when it cannot, the
//! next takes over, and every switch is an entry in the run log (a `status` event whose `data.harness.event` is `switch`).
//! Nothing here waits on a person: a rate limit is waited out or routed around, an expired sign-in switches agent and tells the
//! user once, repeated crashes switch, and when every agent is unavailable the optimization pauses and resumes by itself.

use ratchet_core::AgentsPolicy;
use ratchet_core::enums::AgentKind;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Where the policy in force comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AgentsPolicySource {
    /// The optimization's settings; `PUT /v1/optimizations/{id}/agents` changes them.
    Settings,
    /// An `agents:` section in `ratchet.yaml` overrides the settings; change it in the repo.
    Config,
}

/// `GET /v1/optimizations/{id}/agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentsPolicyVm {
    /// Ordered, most preferred first.
    pub harnesses: Vec<AgentKind>,
    /// A rate limit that lifts within this many seconds is waited out; a longer or unknown one switches agent.
    pub wait_limit_s: u64,
    /// Consecutive crashed or malformed turns on one agent before switching.
    pub crash_strikes: u32,
    /// While every agent is unavailable, how often to look again, in seconds.
    pub recheck_s: u64,
    pub source: AgentsPolicySource,
    /// "Claude Code → Codex → Grok", the order the row shows.
    pub summary: String,
}

impl AgentsPolicyVm {
    pub fn new(policy: &AgentsPolicy, source: AgentsPolicySource) -> Self {
        Self {
            harnesses: policy.harnesses.clone(),
            wait_limit_s: policy.wait_limit_s,
            crash_strikes: policy.crash_strikes,
            recheck_s: policy.recheck_s,
            source,
            summary: summary_of(&policy.harnesses),
        }
    }
}

/// `PUT /v1/optimizations/{id}/agents`. A rule left out keeps its default (30 minutes, 3 strikes, 5 minutes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetAgentsPolicyInput {
    pub harnesses: Vec<AgentKind>,
    pub wait_limit_s: Option<u64>,
    pub crash_strikes: Option<u32>,
    pub recheck_s: Option<u64>,
}

impl SetAgentsPolicyInput {
    pub fn policy(&self) -> AgentsPolicy {
        let mut p = AgentsPolicy::new(self.harnesses.iter().copied());
        if let Some(v) = self.wait_limit_s {
            p.wait_limit_s = v;
        }
        if let Some(v) = self.crash_strikes {
            p.crash_strikes = v;
        }
        if let Some(v) = self.recheck_s {
            p.recheck_s = v;
        }
        p
    }
}

fn name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "Claude Code",
        AgentKind::Codex => "Codex",
        AgentKind::Grok => "Grok",
        AgentKind::Mock => "Mock",
    }
}

/// "Claude Code → Codex → Grok".
pub fn summary_of(harnesses: &[AgentKind]) -> String {
    harnesses
        .iter()
        .map(|k| name(*k))
        .collect::<Vec<_>>()
        .join(" \u{2192} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_reads_in_order() {
        assert_eq!(
            summary_of(&[AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Grok]),
            "Claude Code \u{2192} Codex \u{2192} Grok"
        );
    }

    #[test]
    fn an_input_keeps_defaults_for_the_rules_it_leaves_out() {
        let p = SetAgentsPolicyInput {
            harnesses: vec![AgentKind::Codex, AgentKind::Codex],
            wait_limit_s: None,
            crash_strikes: Some(2),
            recheck_s: None,
        }
        .policy();
        assert_eq!(p.harnesses, [AgentKind::Codex]);
        assert_eq!(
            (p.wait_limit_s, p.crash_strikes, p.recheck_s),
            (1800, 2, 300)
        );
    }
}
