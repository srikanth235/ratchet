//! Where the Rust port is deliberately more robust than the TypeScript original: a store error is a retry, never the end of a reader.

mod common;

use ratchet_core::ports::{
    AgentEvent, AgentEventType, ArtifactStore, CancellationToken, Clock, PortError, PortResult,
    RunnerInbox, RunnerMessage,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::MemoryStore;

use ratchet_agents::{ArtifactEventSink, ArtifactInbox, EventSinkOptions, event_key, inbox_key};

/// Counts sleeps and aborts after 20 of them.
struct CountingClock {
    sleeps: Mutex<u32>,
    cancel: CancellationToken,
}

#[async_trait]
impl Clock for CountingClock {
    fn now(&self) -> i64 {
        0
    }
    async fn sleep(&self, _ms: u64, _cancel: Option<&CancellationToken>) -> PortResult<()> {
        let mut n = self.sleeps.lock().unwrap();
        *n += 1;
        if *n > 20 {
            self.cancel.cancel();
        }
        Ok(())
    }
}

/// A store whose first `get` calls fail, like a transient S3 error; the first `list` can fail too.
struct Flaky {
    inner: MemoryStore,
    get_failures: Mutex<u32>,
    list_failures: Mutex<u32>,
}

impl Flaky {
    fn new(get_failures: u32, list_failures: u32) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::default(),
            get_failures: Mutex::new(get_failures),
            list_failures: Mutex::new(list_failures),
        })
    }
}

fn take_failure(counter: &Mutex<u32>) -> bool {
    let mut left = counter.lock().unwrap();
    let fail = *left > 0;
    if fail {
        *left -= 1;
    }
    fail
}

#[async_trait]
impl ArtifactStore for Flaky {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        self.inner.put(key, body).await
    }
    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        if take_failure(&self.get_failures) {
            return Err(PortError::failed("S3 503"));
        }
        self.inner.get(key).await
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        if take_failure(&self.list_failures) {
            return Err(PortError::failed("S3 503"));
        }
        self.inner.list(prefix).await
    }
}

#[tokio::test]
async fn the_inbox_retries_after_a_store_error_instead_of_giving_up() {
    let store = Flaky::new(2, 1);
    store
        .put(&inbox_key("p/", 1), br#"{"type":"stop"}"#)
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let clock = Arc::new(CountingClock {
        sleeps: Mutex::new(0),
        cancel: cancel.clone(),
    });
    let inbox = ArtifactInbox::new(store, "p/")
        .with_clock(clock.clone())
        .with_poll_interval(Duration::from_millis(1));
    assert_eq!(
        inbox.next(Some(&cancel)).await.unwrap(),
        Some(RunnerMessage::Stop)
    );
    assert_eq!(*clock.sleeps.lock().unwrap(), 3); // one failed list and two failed gets, each retried after the poll interval
}

#[tokio::test]
async fn the_inbox_skips_a_malformed_message_and_delivers_the_next() {
    let store = Arc::new(MemoryStore::default());
    store.put(&inbox_key("p/", 1), b"{not json").await.unwrap();
    store
        .put(&inbox_key("p/", 2), br#"{"type":"pause"}"#)
        .await
        .unwrap();
    let inbox = ArtifactInbox::new(store, "p/").with_poll_interval(Duration::from_millis(1));
    assert_eq!(inbox.next(None).await.unwrap(), Some(RunnerMessage::Pause));
}

#[tokio::test]
async fn the_event_sink_never_writes_over_batches_it_could_not_list() {
    // a restarted sink whose first list fails keeps its events and writes nothing, rather than starting again at batch 1
    let store = Flaky::new(0, 1);
    store
        .put(&event_key("p/", 5), b"{\"type\":\"status\"}\n")
        .await
        .unwrap();
    let sink = ArtifactEventSink::new(
        store.clone(),
        "p/",
        EventSinkOptions {
            flush_interval: Some(Duration::from_secs(60)),
            max_buffered: None,
        },
    );
    sink.emit(AgentEvent::new(1, AgentEventType::Status).with_text("x"));
    sink.flush().await;
    assert_eq!(sink.errors().len(), 1);
    assert_eq!(
        store.list("p/events/").await.unwrap(),
        vec![event_key("p/", 5)]
    );
    sink.close().await; // the list works now
    assert_eq!(
        store.list("p/events/").await.unwrap(),
        vec![event_key("p/", 5), event_key("p/", 6)]
    );
}
