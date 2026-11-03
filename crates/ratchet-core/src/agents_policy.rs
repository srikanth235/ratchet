//! Which agent CLIs an optimization may use, in which order, and what to do when one stops working (docs/AGENT_HARNESS_PLAN.md
//! section 4.6).
//!
//! The list is ordered: the first harness runs; when it cannot, the next one takes over. An optimization never waits on a
//! person for this: a rate limit is waited out or routed around, an expired login switches harness (and tells the user once),
//! repeated crashes switch, and when every harness is unavailable the optimization pauses and resumes by itself.
//!
//! Two places set the policy, the later one winning: the optimization's settings (kept in the ledger) and an optional
//! `agents:` section in `ratchet.yaml`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::enums::AgentKind;

/// A rate limit that lifts within this many seconds is waited out; a longer or unknown one switches harness.
pub const DEFAULT_WAIT_LIMIT_S: u64 = 30 * 60;
/// Consecutive crashed or malformed turns on one harness before switching.
pub const DEFAULT_CRASH_STRIKES: u32 = 3;
/// While every harness is unavailable, look again this often.
pub const DEFAULT_RECHECK_S: u64 = 5 * 60;

/// The harness list and the fallback rules of one optimization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentsPolicy {
    /// Ordered, most preferred first, no repeats. `mock` is not a harness and never appears here.
    pub harnesses: Vec<AgentKind>,
    /// Wait for a rate limit that lifts within this many seconds; switch for a longer or unknown one.
    #[serde(default = "default_wait_limit")]
    pub wait_limit_s: u64,
    /// Consecutive crashes or malformed streams on one harness before switching.
    #[serde(default = "default_strikes")]
    pub crash_strikes: u32,
    /// Seconds between looks at the harnesses while all of them are unavailable.
    #[serde(default = "default_recheck")]
    pub recheck_s: u64,
}

fn default_wait_limit() -> u64 {
    DEFAULT_WAIT_LIMIT_S
}
fn default_strikes() -> u32 {
    DEFAULT_CRASH_STRIKES
}
fn default_recheck() -> u64 {
    DEFAULT_RECHECK_S
}

impl Default for AgentsPolicy {
    fn default() -> Self {
        Self {
            harnesses: Vec::new(),
            wait_limit_s: DEFAULT_WAIT_LIMIT_S,
            crash_strikes: DEFAULT_CRASH_STRIKES,
            recheck_s: DEFAULT_RECHECK_S,
        }
    }
}

impl AgentsPolicy {
    /// A policy with the default rules over `harnesses` (repeats and `mock` dropped, order kept).
    pub fn new(harnesses: impl IntoIterator<Item = AgentKind>) -> Self {
        Self {
            harnesses: dedup(harnesses),
            ..Self::default()
        }
    }

    /// The order a slot that starts on `first` falls back through: `first`, then the rest of the list in order. A `first`
    /// that is not on the list still leads, so an optimization created before it had a policy keeps running its own agent.
    pub fn chain_from(&self, first: AgentKind) -> Vec<AgentKind> {
        if first == AgentKind::Mock {
            return vec![first];
        }
        let mut chain = vec![first];
        chain.extend(self.harnesses.iter().copied().filter(|k| *k != first));
        chain
    }

    /// Why the policy cannot be used, in the user's words; `None` when it is fine.
    pub fn problem(&self) -> Option<String> {
        if self.harnesses.is_empty() {
            return Some("list at least one agent".into());
        }
        if self.harnesses.contains(&AgentKind::Mock) {
            return Some("mock is not an agent you can order".into());
        }
        if self.crash_strikes == 0 {
            return Some("crash strikes must be at least 1".into());
        }
        if self.recheck_s == 0 {
            return Some("the re-check interval must be at least 1 second".into());
        }
        None
    }
}

/// Keeps the first occurrence of each kind and drops `mock`.
pub fn dedup(kinds: impl IntoIterator<Item = AgentKind>) -> Vec<AgentKind> {
    let mut out: Vec<AgentKind> = Vec::new();
    for k in kinds {
        if k != AgentKind::Mock && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chain_leads_with_the_slots_own_harness() {
        let p = AgentsPolicy::new([AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Grok]);
        assert_eq!(
            p.chain_from(AgentKind::Codex),
            [AgentKind::Codex, AgentKind::ClaudeCode, AgentKind::Grok]
        );
        assert_eq!(
            p.chain_from(AgentKind::ClaudeCode),
            [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Grok]
        );
        assert_eq!(p.chain_from(AgentKind::Mock), [AgentKind::Mock]);
    }

    #[test]
    fn repeats_and_mock_are_dropped_and_problems_are_named() {
        let p = AgentsPolicy::new([
            AgentKind::Codex,
            AgentKind::Mock,
            AgentKind::Codex,
            AgentKind::Grok,
        ]);
        assert_eq!(p.harnesses, [AgentKind::Codex, AgentKind::Grok]);
        assert_eq!(p.problem(), None);
        assert!(AgentsPolicy::default().problem().is_some());
        let mut bad = p.clone();
        bad.crash_strikes = 0;
        assert!(bad.problem().unwrap().contains("strikes"));
    }

    #[test]
    fn missing_rules_take_their_defaults_on_the_wire() {
        let p: AgentsPolicy = serde_json::from_str(r#"{"harnesses":["codex"]}"#).unwrap();
        assert_eq!(p.wait_limit_s, 1800);
        assert_eq!(p.crash_strikes, 3);
        assert_eq!(p.recheck_s, 300);
    }
}
