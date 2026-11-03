//! Capability tokens of MCP sessions: issued when a harness session starts, revoked when it ends.
//!
//! A session token is not the owner's bearer token. It opens `/v1/mcp/*` only, and only the tool set (and repo or optimization)
//! it was issued for. Every other route answers it with 401.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use ratchet_mcp::ToolSet;

use crate::auth::generate_token;

/// What a token is bound to. `None` means "any" for that dimension.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// A chat about one repo may only draft optimizations for it.
    pub repo: Option<String>,
    /// A chat bound to an optimization (steering), or an optimization agent's own optimization.
    pub optimization: Option<String>,
    /// The agent an optimization session belongs to (its steering notes).
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// Not secret; used to revoke and in logs.
    pub id: String,
    pub set: ToolSet,
    pub scope: Scope,
}

/// What `issue` hands to the caller; the token is shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedSession {
    pub id: String,
    pub token: String,
    pub set: ToolSet,
}

#[derive(Default)]
pub struct Sessions {
    by_token: Mutex<HashMap<String, SessionInfo>>,
}

impl Sessions {
    fn map(&self) -> MutexGuard<'_, HashMap<String, SessionInfo>> {
        self.by_token.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn issue(&self, set: ToolSet, scope: Scope) -> IssuedSession {
        let token = generate_token();
        let id = format!("mcps_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        self.map().insert(
            token.clone(),
            SessionInfo {
                id: id.clone(),
                set,
                scope,
            },
        );
        IssuedSession { id, token, set }
    }

    /// The session a token opens, if it is still valid.
    pub fn resolve(&self, token: &str) -> Option<SessionInfo> {
        // The tokens are 256 random bits, so a map lookup leaks nothing useful; still compare in constant time on a hit.
        let map = self.map();
        let (_, info) = map
            .iter()
            .find(|(t, _)| crate::auth::constant_time_eq(t.as_bytes(), token.as_bytes()))?;
        Some(info.clone())
    }

    /// End one session by its id. True when it existed.
    pub fn revoke(&self, id: &str) -> bool {
        let mut map = self.map();
        let before = map.len();
        map.retain(|_, s| s.id != id);
        map.len() != before
    }

    pub fn revoke_all(&self) {
        self.map().clear();
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_resolve_to_their_set_until_revoked() {
        let s = Sessions::default();
        let chat = s.issue(ToolSet::Chat, Scope::default());
        let opt = s.issue(
            ToolSet::Optimization,
            Scope {
                optimization: Some("o1".into()),
                ..Scope::default()
            },
        );
        assert_ne!(chat.token, opt.token);
        assert_eq!(s.resolve(&chat.token).unwrap().set, ToolSet::Chat);
        assert_eq!(
            s.resolve(&opt.token).unwrap().scope.optimization.as_deref(),
            Some("o1")
        );
        assert!(s.resolve("nope").is_none());
        assert!(s.revoke(&chat.id));
        assert!(s.resolve(&chat.token).is_none());
        assert!(!s.revoke(&chat.id));
        assert!(s.resolve(&opt.token).is_some());
        s.revoke_all();
        assert!(s.is_empty());
    }
}
