//! Error classification shared by the parsers: is a failure a rate limit (and when does it lift) or an expired login?
//!
//! Structured fields win where a CLI has them (Claude's `rate_limit_event`, `error` and `api_error_status`; Codex app-server's
//! `codexErrorInfo`). Text is the fallback for the CLIs that only print prose (Codex `exec`, Grok). See `docs/harness-matrix.md`
//! section 3 for the shapes.

use ratchet_core::ports::{AgentEvent, AgentEventType};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Out of quota or throttled. `resets_at` is epoch seconds when the CLI says when it lifts.
    RateLimited { resets_at: Option<i64> },
    /// The login or key was rejected.
    AuthExpired,
}

/// `needle` (a number such as `429`) occurs in `haystack` not glued to other digits.
fn has_number(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(i, _)| {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + needle.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_digit()) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// Classify by the words a CLI prints. `now_secs` anchors a relative "retry after Ns" into `resets_at`.
pub fn classify_text(text: &str, now_secs: i64) -> Option<FailureKind> {
    let t = text.to_ascii_lowercase();
    let rate = ["usage limit", "rate limit", "rate_limit", "ratelimit"]
        .iter()
        .any(|k| t.contains(k))
        || t.contains("too many requests")
        || t.contains("hit your limit")
        || t.contains("quota")
        || has_number(&t, "429");
    if rate {
        return Some(FailureKind::RateLimited {
            resets_at: retry_after_secs(&t).map(|s| now_secs + s),
        });
    }
    let auth = [
        "authentication_failed",
        "authentication failed",
        "unauthorized",
        "invalid api key",
        "invalid x-api-key",
        "please run /login",
        "not logged in",
        "grok login",
        "log out and sign in",
        "sign in again",
        "token could not be refreshed",
    ]
    .iter()
    .any(|k| t.contains(k))
        || has_number(&t, "401");
    auth.then_some(FailureKind::AuthExpired)
}

/// "retry after 30s" / "retry after 30 seconds" / "retry-after: 30".
fn retry_after_secs(lower: &str) -> Option<i64> {
    let i = lower
        .find("retry after")
        .or_else(|| lower.find("retry-after"))?;
    let rest = lower[i..].trim_start_matches(|c: char| !c.is_ascii_digit());
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Classify by an HTTP status.
pub fn classify_status(status: i64) -> Option<FailureKind> {
    match status {
        429 => Some(FailureKind::RateLimited { resets_at: None }),
        401 => Some(FailureKind::AuthExpired),
        _ => None,
    }
}

/// The event for a failure: `RateLimited` (with `data.resetsAt` when known) or `AuthExpired`, carrying the CLI's text.
pub fn failure_event(at: i64, kind: FailureKind, text: &str) -> AgentEvent {
    match kind {
        FailureKind::RateLimited { resets_at } => AgentEvent::new(at, AgentEventType::RateLimited)
            .with_text(text)
            .with_data(json!({"resetsAt": resets_at.map_or(Value::Null, Value::from)})),
        FailureKind::AuthExpired => {
            AgentEvent::new(at, AgentEventType::AuthExpired).with_text(text)
        }
    }
}

/// Emits at most one classified event per turn.
#[derive(Debug, Default)]
pub struct FailureOnce {
    emitted: Option<FailureKind>,
}

impl FailureOnce {
    pub fn reset(&mut self) {
        self.emitted = None;
    }

    pub fn kind(&self) -> Option<FailureKind> {
        self.emitted
    }

    /// The event for `kind`, unless one was already emitted this turn. A later signal that adds a reset time to a rate limit is
    /// not re-emitted (the first event is what fallback logic reads), so parsers call this with the best-informed signal first.
    pub fn emit(&mut self, at: i64, kind: FailureKind, text: &str) -> Option<AgentEvent> {
        if self.emitted.is_some() {
            return None;
        }
        self.emitted = Some(kind);
        Some(failure_event(at, kind, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_classification() {
        assert_eq!(
            classify_text("You've hit your usage limit. try again at Nov 4th", 0),
            Some(FailureKind::RateLimited { resets_at: None })
        );
        assert_eq!(
            classify_text(
                "429 Too Many Requests: rate limit exceeded, retry after 30s",
                100
            ),
            Some(FailureKind::RateLimited {
                resets_at: Some(130)
            })
        );
        assert_eq!(
            classify_text("unexpected status 401 Unauthorized", 0),
            Some(FailureKind::AuthExpired)
        );
        assert_eq!(
            classify_text("Authentication failed. Run `grok login` first", 0),
            Some(FailureKind::AuthExpired)
        );
        assert_eq!(classify_text("exit 4290 files", 0), None);
        assert_eq!(classify_text("Reconnecting... 1/5", 0), None);
        assert_eq!(
            classify_status(429),
            Some(FailureKind::RateLimited { resets_at: None })
        );
        assert_eq!(classify_status(500), None);
    }
}
