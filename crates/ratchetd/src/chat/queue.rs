//! The hand-off between a harness's synchronous event sink and the chat's async pump.
//!
//! The sink must never block and never lose a turn's structure, so the queue is unbounded for everything that matters
//! (messages, tool steps, usage, errors, the end of the turn) and bounded for the streaming text: neighbouring `Delta`s (and
//! `Thinking`s) are merged into one entry while the pump is busy, and past a size cap further ones are dropped. A dropped delta
//! costs nothing but the typing effect, because the finished `Message` carries the whole text.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ratchet_core::ports::{AgentEvent, AgentEventType, PortError};
use tokio::sync::Notify;

/// A merged delta stops growing here; the next delta starts a new entry.
const MERGE_BYTES: usize = 8 * 1024;
/// Streaming text waiting for the pump, at most this much.
const TEXT_CAP_BYTES: usize = 256 * 1024;

pub enum Item {
    Agent(AgentEvent),
    End(TurnEnd),
}

/// How a turn ended, pushed after `send` returned so it lands behind every event of the turn.
pub struct TurnEnd {
    pub result: Result<(), PortError>,
    /// The CLI session id now (for resume).
    pub session_id: Option<String>,
    /// The turn never reached the CLI: the session could not be opened.
    pub not_started: Option<crate::chat::cards::Failure>,
}

#[derive(Default)]
struct Inner {
    items: VecDeque<Item>,
    text_bytes: usize,
}

#[derive(Default)]
pub struct EventQueue {
    inner: Mutex<Inner>,
    notify: Notify,
    closed: AtomicBool,
}

fn is_text(t: AgentEventType) -> bool {
    matches!(t, AgentEventType::Delta | AgentEventType::Thinking)
}

impl EventQueue {
    pub fn push_agent(&self, ev: AgentEvent) {
        {
            let mut q = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if is_text(ev.event_type) {
                let add = ev.text.as_ref().map_or(0, String::len);
                if add == 0 {
                    return;
                }
                if q.text_bytes + add > TEXT_CAP_BYTES {
                    return;
                }
                q.text_bytes += add;
                let merged = match q.items.back_mut() {
                    Some(Item::Agent(last))
                        if last.event_type == ev.event_type
                            && last.text.as_ref().map_or(0, String::len) + add <= MERGE_BYTES =>
                    {
                        last.text
                            .get_or_insert_with(String::new)
                            .push_str(ev.text.as_deref().unwrap_or(""));
                        true
                    }
                    _ => false,
                };
                if merged {
                    drop(q);
                    self.notify.notify_one();
                    return;
                }
            }
            q.items.push_back(Item::Agent(ev));
        }
        self.notify.notify_one();
    }

    pub fn push_end(&self, end: TurnEnd) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .items
            .push_back(Item::End(end));
        self.notify.notify_one();
    }

    /// No more items will come: `next_batch` returns `None` once the queue is empty.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// Everything queued, in order; waits when there is nothing. `None` when closed and drained.
    pub async fn next_batch(&self) -> Option<Vec<Item>> {
        loop {
            {
                let mut q = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if !q.items.is_empty() {
                    q.text_bytes = 0;
                    return Some(q.items.drain(..).collect());
                }
                if self.closed.load(Ordering::SeqCst) {
                    return None;
                }
            }
            self.notify.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(t: &str) -> AgentEvent {
        AgentEvent::new(0, AgentEventType::Delta).with_text(t)
    }

    #[tokio::test]
    async fn neighbouring_deltas_merge_and_order_is_kept() {
        let q = EventQueue::default();
        q.push_agent(delta("a"));
        q.push_agent(delta("b"));
        q.push_agent(AgentEvent::new(0, AgentEventType::Tool).with_text("t"));
        q.push_agent(delta("c"));
        q.push_end(TurnEnd {
            result: Ok(()),
            session_id: None,
            not_started: None,
        });
        let batch = q.next_batch().await.unwrap();
        let texts: Vec<String> = batch
            .iter()
            .map(|i| match i {
                Item::Agent(e) => {
                    format!("{:?}:{}", e.event_type, e.text.clone().unwrap_or_default())
                }
                Item::End(_) => "end".into(),
            })
            .collect();
        assert_eq!(texts, ["Delta:ab", "Tool:t", "Delta:c", "end"]);
    }

    #[tokio::test]
    async fn streaming_text_is_capped_but_structure_never_is() {
        let q = EventQueue::default();
        let chunk = "x".repeat(MERGE_BYTES);
        for _ in 0..(TEXT_CAP_BYTES / MERGE_BYTES + 10) {
            q.push_agent(delta(&chunk));
        }
        q.push_agent(AgentEvent::new(0, AgentEventType::Message).with_text("whole"));
        let batch = q.next_batch().await.unwrap();
        let bytes: usize = batch
            .iter()
            .filter_map(|i| match i {
                Item::Agent(e) if e.event_type == AgentEventType::Delta => {
                    e.text.as_ref().map(String::len)
                }
                _ => None,
            })
            .sum();
        assert!(bytes <= TEXT_CAP_BYTES, "{bytes}");
        assert!(
            matches!(batch.last(), Some(Item::Agent(e)) if e.event_type == AgentEventType::Message)
        );
    }

    #[tokio::test]
    async fn a_closed_queue_drains_then_ends() {
        let q = EventQueue::default();
        q.push_agent(delta("a"));
        q.close();
        assert!(q.next_batch().await.is_some());
        assert!(q.next_batch().await.is_none());
    }
}
