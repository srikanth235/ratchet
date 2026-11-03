//! Harness fallback for unattended optimizations (docs/AGENT_HARNESS_PLAN.md section 4.6).
//!
//! The runner loop owns one agent slot. Its [`FallbackPlan`] lists the harnesses that slot may use, in order. [`FallbackState`]
//! is the pure part of the rules, so every case is a unit test:
//!
//! 1. `RateLimited` that lifts within the policy's wait limit (30 min): wait for it. A longer or unknown one: switch to the next
//!    harness at the next turn, carrying the brief and the history (CLI sessions do not cross CLIs).
//! 2. `AuthExpired`: switch now, and tell the user once.
//! 3. The policy's crash strikes (3) in a row on one harness, crashed or malformed stream alike: switch. A slot with no other
//!    harness keeps the old rule: the optimization's agent fails.
//! 4. Every harness unavailable: pause with the reason; look again every `recheck_s` (5 min) and carry on with the first harness
//!    that is back.
//!
//! The runner loop turns every decision into a status event in the run log. Nothing here ever waits on a person.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use ratchet_core::AgentsPolicy;
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::AgentAdapter;

/// A rate limit whose reset time the CLI did not give is treated as lasting this long.
pub const UNKNOWN_LIMIT_COOLDOWN_S: i64 = 60 * 60;

/// One harness of a slot's chain: the adapter that drives it and the model to ask it for (`None`: its default).
#[derive(Clone)]
pub struct HarnessSlot {
    pub kind: AgentKind,
    pub adapter: Arc<dyn AgentAdapter>,
    pub model: Option<String>,
}

impl HarnessSlot {
    pub fn new(kind: AgentKind, adapter: Arc<dyn AgentAdapter>) -> Self {
        Self {
            kind,
            adapter,
            model: None,
        }
    }
}

/// Is a harness usable right now (installed, signed in)? The local daemon answers from detection; without a probe the slot's
/// own adapter `preflight` decides.
#[async_trait]
pub trait AvailabilityProbe: Send + Sync {
    async fn check(&self, kind: AgentKind) -> Result<(), String>;
}

/// The harnesses one slot may use and the rules for moving between them.
#[derive(Clone)]
pub struct FallbackPlan {
    /// `chain[0]` is the slot's own harness (the loop's `adapter`).
    pub chain: Vec<HarnessSlot>,
    pub policy: AgentsPolicy,
    pub probe: Option<Arc<dyn AvailabilityProbe>>,
}

/// The name a person knows a harness by.
pub fn label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "Claude Code",
        AgentKind::Codex => "Codex",
        AgentKind::Grok => "Grok",
        AgentKind::Mock => "the mock agent",
    }
}

/// What a failed turn looked like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// `resets_at_ms` is epoch milliseconds when the CLI said when the limit lifts.
    RateLimited {
        resets_at_ms: Option<i64>,
    },
    AuthExpired,
    /// The process failed or the stream was malformed.
    Crash,
}

/// Why a harness is set aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    RateLimited,
    AuthExpired,
    Crashing,
    /// Detection says it is not installed or not signed in.
    Unavailable,
}

/// What to do about a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// A crash below the strike count: run the turn again on the same harness.
    Retry,
    /// A short rate limit: wait until `until_ms`, then run the turn again on the same harness.
    Wait { until_ms: i64 },
    /// Leave this harness for the next one that is available (the caller probes the candidates).
    Leave { cause: Cause },
    /// A lone harness crashed too often: the agent fails, as before fallback existed.
    GiveUp,
}

#[derive(Debug, Clone)]
struct Cooling {
    cause: Cause,
    /// `None`: until a probe says it is back.
    until_ms: Option<i64>,
    why: String,
}

/// Which harness of the chain is active and which are set aside.
pub struct FallbackState {
    kinds: Vec<AgentKind>,
    policy: AgentsPolicy,
    active: usize,
    strikes: u32,
    strikes_to_leave: u32,
    cooling: HashMap<usize, Cooling>,
}

impl FallbackState {
    /// `strikes_to_leave` overrides the policy's crash strikes (the loop's own tuning knob wins).
    pub fn new(kinds: Vec<AgentKind>, policy: AgentsPolicy, strikes_to_leave: Option<u32>) -> Self {
        let strikes_to_leave = strikes_to_leave.unwrap_or(policy.crash_strikes).max(1);
        Self {
            kinds,
            policy,
            active: 0,
            strikes: 0,
            strikes_to_leave,
            cooling: HashMap::new(),
        }
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn active_kind(&self) -> AgentKind {
        self.kinds[self.active]
    }

    pub fn kind(&self, idx: usize) -> AgentKind {
        self.kinds[idx]
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn policy(&self) -> &AgentsPolicy {
        &self.policy
    }

    pub fn strikes(&self) -> u32 {
        self.strikes
    }

    /// A turn went through: the crash count starts over.
    pub fn on_success(&mut self) {
        self.strikes = 0;
    }

    /// Decide what a failed turn means. Does not change the active harness; the caller applies [`Next::Leave`] with
    /// [`FallbackState::leave`] and [`FallbackState::activate`].
    pub fn on_failure(&mut self, failure: Failure, now_ms: i64) -> Next {
        match failure {
            Failure::RateLimited { resets_at_ms } => {
                let limit_ms = (self.policy.wait_limit_s as i64).saturating_mul(1000);
                match resets_at_ms {
                    Some(at) if at - now_ms <= limit_ms => Next::Wait {
                        until_ms: at.max(now_ms),
                    },
                    _ => Next::Leave {
                        cause: Cause::RateLimited,
                    },
                }
            }
            Failure::AuthExpired => Next::Leave {
                cause: Cause::AuthExpired,
            },
            Failure::Crash => {
                self.strikes += 1;
                if self.strikes < self.strikes_to_leave {
                    Next::Retry
                } else if self.kinds.len() > 1 {
                    Next::Leave {
                        cause: Cause::Crashing,
                    }
                } else {
                    Next::GiveUp
                }
            }
        }
    }

    /// Set the active harness aside for `cause`. `resets_at_ms` is the CLI's reset time for a rate limit.
    pub fn leave(&mut self, cause: Cause, resets_at_ms: Option<i64>, now_ms: i64, why: &str) {
        let until_ms = match cause {
            Cause::RateLimited => {
                Some(resets_at_ms.unwrap_or(now_ms + UNKNOWN_LIMIT_COOLDOWN_S * 1000))
            }
            Cause::Crashing => {
                Some(now_ms + (self.policy.wait_limit_s as i64).saturating_mul(1000))
            }
            Cause::AuthExpired | Cause::Unavailable => None,
        };
        self.cool(self.active, cause, until_ms, why);
    }

    /// Set harness `idx` aside (`until_ms: None` means until a probe passes).
    pub fn cool(&mut self, idx: usize, cause: Cause, until_ms: Option<i64>, why: &str) {
        self.cooling.insert(
            idx,
            Cooling {
                cause,
                until_ms,
                why: why.to_owned(),
            },
        );
    }

    /// Make `idx` the active harness and forget why it was set aside.
    pub fn activate(&mut self, idx: usize) {
        self.active = idx;
        self.strikes = 0;
        self.cooling.remove(&idx);
    }

    /// Harnesses worth trying, in the order to try them. After a failure that is the ones after the active one, wrapping round
    /// (`after_active`); while paused it is the whole chain in list order. A harness set aside until a known time is left out
    /// until then; one waiting for a probe is included (the caller probes it).
    pub fn candidates(&mut self, now_ms: i64, after_active: bool) -> Vec<usize> {
        self.cooling
            .retain(|_, c| c.until_ms.is_none_or(|u| u > now_ms));
        let n = self.kinds.len();
        let order: Vec<usize> = if after_active {
            (1..n).map(|i| (self.active + i) % n).collect()
        } else {
            (0..n).collect()
        };
        order
            .into_iter()
            .filter(|i| self.cooling.get(i).is_none_or(|c| c.until_ms.is_none()))
            .collect()
    }

    /// One line per harness that is set aside, for the pause reason: "Claude Code: rate limited until 14:05; Codex: signed out".
    pub fn describe(&self, now_ms: i64) -> String {
        let mut parts = Vec::new();
        for (i, kind) in self.kinds.iter().enumerate() {
            let Some(c) = self.cooling.get(&i) else {
                continue;
            };
            let what = match c.cause {
                Cause::RateLimited => match c.until_ms {
                    Some(u) if u > now_ms => format!(
                        "rate limited, back in about {}",
                        minutes_text((u - now_ms) / 1000)
                    ),
                    _ => "rate limited".to_owned(),
                },
                Cause::AuthExpired => "sign-in expired".to_owned(),
                Cause::Crashing => "keeps failing".to_owned(),
                Cause::Unavailable => c.why.clone(),
            };
            parts.push(format!("{}: {what}", label(*kind)));
        }
        parts.join("; ")
    }
}

/// "5 minutes", "1 minute", "2 hours".
pub fn minutes_text(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60;
    match m {
        0 | 1 => "1 minute".to_owned(),
        2..=119 => format!("{m} minutes"),
        _ => format!("{} hours", (m + 59) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn state(kinds: &[AgentKind]) -> FallbackState {
        FallbackState::new(
            kinds.to_vec(),
            AgentsPolicy::new(kinds.iter().copied()),
            None,
        )
    }

    #[test]
    fn a_short_rate_limit_is_waited_out() {
        let mut s = state(&[AgentKind::ClaudeCode, AgentKind::Codex]);
        let at = NOW + 10 * 60_000;
        assert_eq!(
            s.on_failure(
                Failure::RateLimited {
                    resets_at_ms: Some(at)
                },
                NOW
            ),
            Next::Wait { until_ms: at }
        );
        // exactly the limit still waits
        let at = NOW + 30 * 60_000;
        assert_eq!(
            s.on_failure(
                Failure::RateLimited {
                    resets_at_ms: Some(at)
                },
                NOW
            ),
            Next::Wait { until_ms: at }
        );
        // a reset time already past is not a wait at all
        assert_eq!(
            s.on_failure(
                Failure::RateLimited {
                    resets_at_ms: Some(NOW - 5)
                },
                NOW
            ),
            Next::Wait { until_ms: NOW }
        );
    }

    #[test]
    fn a_long_or_unknown_rate_limit_leaves_the_harness() {
        let mut s = state(&[AgentKind::ClaudeCode, AgentKind::Codex]);
        let at = NOW + 31 * 60_000;
        assert_eq!(
            s.on_failure(
                Failure::RateLimited {
                    resets_at_ms: Some(at)
                },
                NOW
            ),
            Next::Leave {
                cause: Cause::RateLimited
            }
        );
        assert_eq!(
            s.on_failure(Failure::RateLimited { resets_at_ms: None }, NOW),
            Next::Leave {
                cause: Cause::RateLimited
            }
        );
        s.leave(Cause::RateLimited, Some(at), NOW, "");
        assert_eq!(s.candidates(NOW, true), [1]);
        // the limited harness is back on the list once its reset time has passed
        s.activate(1);
        assert_eq!(s.candidates(at + 1, true), [0]);
    }

    #[test]
    fn an_unknown_limit_cools_for_an_hour() {
        let mut s = state(&[AgentKind::ClaudeCode, AgentKind::Codex]);
        s.leave(Cause::RateLimited, None, NOW, "");
        s.activate(1);
        assert!(s.candidates(NOW + 59 * 60_000, true).is_empty());
        assert_eq!(s.candidates(NOW + 61 * 60_000, true), [0]);
    }

    #[test]
    fn auth_expired_leaves_at_once_and_waits_for_a_probe() {
        let mut s = state(&[AgentKind::Codex, AgentKind::ClaudeCode]);
        assert_eq!(
            s.on_failure(Failure::AuthExpired, NOW),
            Next::Leave {
                cause: Cause::AuthExpired
            }
        );
        s.leave(Cause::AuthExpired, None, NOW, "");
        s.activate(1);
        // never expires by time: it is offered again, for the caller to probe
        assert_eq!(s.candidates(NOW + 10_000_000_000, true), [0]);
        assert!(s.describe(NOW).contains("Codex: sign-in expired"));
    }

    #[test]
    fn three_crashes_in_a_row_leave_and_a_success_resets_the_count() {
        let mut s = state(&[AgentKind::ClaudeCode, AgentKind::Codex]);
        assert_eq!(s.on_failure(Failure::Crash, NOW), Next::Retry);
        assert_eq!(s.on_failure(Failure::Crash, NOW), Next::Retry);
        s.on_success();
        assert_eq!(s.on_failure(Failure::Crash, NOW), Next::Retry);
        assert_eq!(s.on_failure(Failure::Crash, NOW), Next::Retry);
        assert_eq!(
            s.on_failure(Failure::Crash, NOW),
            Next::Leave {
                cause: Cause::Crashing
            }
        );
    }

    #[test]
    fn a_lone_harness_that_keeps_crashing_gives_up() {
        let mut s = state(&[AgentKind::ClaudeCode]);
        assert_eq!(s.on_failure(Failure::Crash, NOW), Next::Retry);
        assert_eq!(s.on_failure(Failure::Crash, NOW), Next::Retry);
        assert_eq!(s.on_failure(Failure::Crash, NOW), Next::GiveUp);
    }

    #[test]
    fn the_loops_own_strike_count_wins() {
        let mut s = FallbackState::new(
            vec![AgentKind::ClaudeCode, AgentKind::Codex],
            AgentsPolicy::new([AgentKind::ClaudeCode, AgentKind::Codex]),
            Some(1),
        );
        assert_eq!(
            s.on_failure(Failure::Crash, NOW),
            Next::Leave {
                cause: Cause::Crashing
            }
        );
    }

    #[test]
    fn candidates_follow_the_list_and_wrap() {
        let mut s = state(&[AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Grok]);
        s.activate(1);
        assert_eq!(s.candidates(NOW, true), [2, 0]);
        assert_eq!(s.candidates(NOW, false), [0, 1, 2]);
        s.cool(2, Cause::Unavailable, None, "Grok is signed out");
        assert!(s.describe(NOW).contains("Grok: Grok is signed out"));
    }

    #[test]
    fn minutes_read_naturally() {
        assert_eq!(minutes_text(10), "1 minute");
        assert_eq!(minutes_text(300), "5 minutes");
        assert_eq!(minutes_text(7200), "2 hours");
    }
}
