//! The event hub: numbers every event, keeps a window of the latest ones for `Last-Event-ID` resume, and fans
//! them out to the SSE connections.
//!
//! Publishing and subscribing both take the same lock, so a connection sees every event exactly once: the replay
//! of what it missed, then the live ones, with no gap and no duplicate between them.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::api::{ApiEvent, Sequenced};

/// How many events a reconnecting client can still catch up on.
pub const DEFAULT_WINDOW: usize = 1024;

struct Buf {
    events: VecDeque<Sequenced>,
    last_id: u64,
}

struct Shared {
    buf: Mutex<Buf>,
    tx: broadcast::Sender<Sequenced>,
    window: usize,
}

#[derive(Clone)]
pub struct EventHub(Arc<Shared>);

/// What a new SSE connection gets.
pub struct Subscription {
    /// Id of the newest event at the time of subscribing.
    pub last_id: u64,
    /// Set when the client's `Last-Event-ID` cannot be honoured: it must refetch what it shows.
    pub resync: Option<String>,
    /// The events the client missed, oldest first.
    pub replay: Vec<Sequenced>,
    pub live: broadcast::Receiver<Sequenced>,
}

impl EventHub {
    pub fn new(window: usize) -> Self {
        let (tx, _) = broadcast::channel(window.max(16));
        Self(Arc::new(Shared {
            buf: Mutex::new(Buf {
                events: VecDeque::new(),
                last_id: 0,
            }),
            tx,
            window: window.max(1),
        }))
    }

    fn buf(&self) -> std::sync::MutexGuard<'_, Buf> {
        self.0.buf.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Number `event`, keep it, and send it to every connection. Returns its id.
    pub fn publish(&self, event: ApiEvent) -> u64 {
        let mut b = self.buf();
        b.last_id += 1;
        let s = Sequenced {
            id: b.last_id,
            event,
        };
        b.events.push_back(s.clone());
        while b.events.len() > self.0.window {
            b.events.pop_front();
        }
        let _ = self.0.tx.send(s.clone());
        s.id
    }

    pub fn last_id(&self) -> u64 {
        self.buf().last_id
    }

    /// Subscribe from `last_event_id` (the header of a reconnecting client; `None` for a fresh one).
    pub fn subscribe(&self, last_event_id: Option<u64>) -> Subscription {
        let b = self.buf();
        let live = self.0.tx.subscribe();
        let last_id = b.last_id;
        let mut resync = None;
        let mut replay = Vec::new();
        if let Some(after) = last_event_id {
            if after > last_id {
                resync = Some("the daemon restarted since your last event".to_owned());
            } else if after < last_id {
                let oldest = b.events.front().map(|e| e.id);
                match oldest {
                    Some(o) if o <= after + 1 => {
                        replay = b.events.iter().filter(|e| e.id > after).cloned().collect();
                    }
                    _ => {
                        resync = Some(format!(
                            "events after {after} are no longer kept (the daemon keeps the last {})",
                            self.0.window
                        ));
                    }
                }
            }
        }
        Subscription {
            last_id,
            resync,
            replay,
            live,
        }
    }

    /// Feed the hub from a service's event channel until the channel closes.
    pub fn bridge(&self, mut rx: broadcast::Receiver<ApiEvent>) -> JoinHandle<()> {
        let hub = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        hub.publish(e);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("event bridge lagged by {n} events");
                        hub.publish(ApiEvent::Resync {
                            reason: format!("the daemon dropped {n} events"),
                        });
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(n: u32) -> ApiEvent {
        ApiEvent::Spend {
            optimization_id: "o".into(),
            total_usd: f64::from(n),
        }
    }

    #[test]
    fn ids_count_from_one() {
        let h = EventHub::new(8);
        assert_eq!(h.last_id(), 0);
        assert_eq!(h.publish(ev(1)), 1);
        assert_eq!(h.publish(ev(2)), 2);
        assert_eq!(h.last_id(), 2);
    }

    #[test]
    fn resume_replays_what_was_missed() {
        let h = EventHub::new(8);
        for i in 1..=5 {
            h.publish(ev(i));
        }
        let s = h.subscribe(Some(2));
        assert!(s.resync.is_none());
        assert_eq!(s.replay.iter().map(|e| e.id).collect::<Vec<_>>(), [3, 4, 5]);
        let fresh = h.subscribe(None);
        assert!(fresh.replay.is_empty() && fresh.resync.is_none());
        assert_eq!(fresh.last_id, 5);
        let caught_up = h.subscribe(Some(5));
        assert!(caught_up.replay.is_empty() && caught_up.resync.is_none());
    }

    #[test]
    fn a_gap_or_a_restart_means_resync() {
        let h = EventHub::new(3);
        for i in 1..=10 {
            h.publish(ev(i));
        }
        // window holds 8..=10
        assert!(h.subscribe(Some(3)).resync.is_some());
        assert!(h.subscribe(Some(7)).resync.is_none());
        assert_eq!(h.subscribe(Some(7)).replay.len(), 3);
        // a client from before a restart is ahead of the new numbering
        assert!(h.subscribe(Some(99)).resync.is_some());
        // an empty hub and a client that has seen something: restarted
        assert!(EventHub::new(3).subscribe(Some(1)).resync.is_some());
    }

    #[tokio::test]
    async fn live_events_follow_the_replay_without_gaps() {
        let h = EventHub::new(8);
        h.publish(ev(1));
        let mut s = h.subscribe(Some(0));
        assert_eq!(s.replay.len(), 1);
        h.publish(ev(2));
        let live = s.live.recv().await.unwrap();
        assert_eq!(live.id, 2);
    }

    #[tokio::test]
    async fn the_bridge_numbers_a_services_events() {
        let (tx, rx) = broadcast::channel(8);
        let h = EventHub::new(8);
        let j = h.bridge(rx);
        tx.send(ev(1)).unwrap();
        tx.send(ev(2)).unwrap();
        drop(tx);
        j.await.unwrap();
        assert_eq!(h.last_id(), 2);
    }
}
