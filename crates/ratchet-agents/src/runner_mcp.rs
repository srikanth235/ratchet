//! `ratchet-runner mcp`: the optimization tool set served from the runner itself, against its inbox and artifact store, so a cloud
//! agent needs no laptop and no daemon.
//!
//! * `steering_notes` reads the `User` messages in the agent's inbox (`inbox/NNNNNNNN.json`) that arrived since the last call.
//!   It only reads; the runner loop keeps its own cursor and still consumes the same messages.
//! * `history` reads the verdicts the runner reported (`events/NNNNNNNN.jsonl`).
//! * `submit_candidate` writes `candidates/NNNNNNNN.json` (`sha`, `note`, `at`) for the engine to pick up.
//! * `connector_query` is refused: connectors are credentials on the user's machine and never reach a runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{AgentEventType, ArtifactStore, Clock, RunnerMessage};
use ratchet_mcp::args::{HistoryArgs, SubmitCandidateArgs};
use ratchet_mcp::{ToolBackend, ToolError};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::artifacts::{last_seq, read_events, seq_of};

pub struct RunnerToolBackend {
    store: Arc<dyn ArtifactStore>,
    prefix: String,
    clock: Arc<dyn Clock>,
    /// Highest inbox sequence number `steering_notes` has returned.
    notes_seen: Mutex<u64>,
}

impl RunnerToolBackend {
    /// `prefix` is `artifacts::runner_prefix(optimization, agent)`.
    pub fn new(store: Arc<dyn ArtifactStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into(),
            clock: Arc::new(SystemClock),
            notes_seen: Mutex::new(0),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    async fn steering_notes(&self) -> Result<Value, ToolError> {
        let dir = format!("{}inbox/", self.prefix);
        let after = *self.notes_seen.lock().unwrap_or_else(|e| e.into_inner());
        let mut keys: Vec<(u64, String)> = self
            .store
            .list(&dir)
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
            .into_iter()
            .filter_map(|k| seq_of(&k).map(|s| (s, k)))
            .filter(|(s, _)| *s > after)
            .collect();
        keys.sort();
        let mut notes = Vec::new();
        let mut seen = after;
        for (seq, key) in keys {
            // an object listed but not readable yet is left for the next call
            let Some(body) = self
                .store
                .get(&key)
                .await
                .map_err(|e| ToolError::new(e.to_string()))?
                .filter(|b| !b.is_empty())
            else {
                break;
            };
            seen = seq;
            if let Ok(RunnerMessage::User { text }) = serde_json::from_str::<RunnerMessage>(&body) {
                notes.push(json!({"seq": seq, "text": text}));
            }
        }
        *self.notes_seen.lock().unwrap_or_else(|e| e.into_inner()) = seen;
        Ok(json!({ "notes": notes }))
    }

    async fn history(&self, limit: u32) -> Result<Value, ToolError> {
        let polled = read_events(self.store.as_ref(), &self.prefix, None)
            .await
            .map_err(|e| ToolError::new(e.to_string()))?;
        let mut verdicts: Vec<Value> = polled
            .events
            .into_iter()
            .filter(|e| e.event_type == AgentEventType::Verdict)
            .map(|e| json!({"at": e.at, "text": e.text, "data": e.data}))
            .collect();
        let skip = verdicts.len().saturating_sub(limit.clamp(1, 200) as usize);
        verdicts.drain(..skip);
        Ok(json!({ "verdicts": verdicts }))
    }

    async fn submit_candidate(&self, a: SubmitCandidateArgs) -> Result<Value, ToolError> {
        let sha = a.sha.trim();
        if sha.len() < 7 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ToolError::new(
                "sha must be a commit sha (7 to 40 hex digits)",
            ));
        }
        let dir = format!("{}candidates/", self.prefix);
        let seq = last_seq(self.store.as_ref(), &dir)
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
            + 1;
        let body = json!({"sha": sha, "note": a.note, "at": self.clock.now()});
        self.store
            .put_str(&format!("{dir}{seq:08}.json"), &body.to_string())
            .await
            .map_err(|e| ToolError::new(e.to_string()))?;
        Ok(json!({"submitted": sha, "seq": seq}))
    }
}

fn args<T: DeserializeOwned>(v: Value) -> Result<T, ToolError> {
    serde_json::from_value(v).map_err(|e| ToolError::new(format!("bad arguments: {e}")))
}

#[async_trait]
impl ToolBackend for RunnerToolBackend {
    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, ToolError> {
        match tool {
            "steering_notes" => self.steering_notes().await,
            "history" => {
                let a: HistoryArgs = args(arguments)?;
                self.history(a.limit.unwrap_or(20)).await
            }
            "submit_candidate" => self.submit_candidate(args(arguments)?).await,
            "connector_query" => Err(ToolError::new(
                "no connector is attached to this optimization; cloud runners never hold the user's connector credentials",
            )),
            other => Err(ToolError::new(format!("unknown tool \"{other}\""))),
        }
    }
}
