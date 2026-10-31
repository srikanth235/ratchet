mod common;

use ratchet_core::ports::{
    ArtifactStore, CancellationToken, Clock, PortResult, RunnerInbox, RunnerMessage,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use ratchet_agents::{ArtifactInbox, event_key, inbox_key, read_events};

/// A store where a listed object reads as empty the first time: a writer that has created the file but not filled it.
#[derive(Default)]
struct RacyStore {
    files: Mutex<std::collections::BTreeMap<String, String>>,
    empty_once: Mutex<std::collections::HashSet<String>>,
}

#[async_trait]
impl ArtifactStore for RacyStore {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        self.files
            .lock()
            .unwrap()
            .insert(key.to_string(), String::from_utf8_lossy(body).into_owned());
        Ok(())
    }
    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        if self.empty_once.lock().unwrap().remove(key) {
            return Ok(Some(String::new()));
        }
        Ok(self.files.lock().unwrap().get(key).cloned())
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

#[tokio::test]
async fn read_events_stops_before_an_empty_batch_and_returns_it_on_the_next_poll() {
    let store = RacyStore::default();
    store
        .put(
            &event_key("p/", 1),
            b"{\"type\":\"status\",\"text\":\"one\"}\n",
        )
        .await
        .unwrap();
    store
        .put(
            &event_key("p/", 2),
            b"{\"type\":\"commit\",\"text\":\"two\"}\n",
        )
        .await
        .unwrap();
    store.empty_once.lock().unwrap().insert(event_key("p/", 2));
    let first = read_events(&store, "p/", None).await.unwrap();
    assert_eq!(common::texts(&first.events), vec!["one"]);
    assert_eq!(first.cursor, "1");
    let second = read_events(&store, "p/", Some(&first.cursor))
        .await
        .unwrap();
    assert_eq!(common::texts(&second.events), vec!["two"]); // the commit was not lost
}

/// Counts sleeps and aborts after 20 of them, so a reader that dropped the message would not poll forever.
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

#[tokio::test]
async fn the_inbox_retries_a_message_that_read_as_empty_instead_of_dropping_it() {
    let store = Arc::new(RacyStore::default());
    let verdict =
        r#"{"type":"verdict","sha":"abc","verdict":"verified","reasons":[],"currentBest":1}"#;
    store
        .put(&inbox_key("p/", 1), verdict.as_bytes())
        .await
        .unwrap();
    store.empty_once.lock().unwrap().insert(inbox_key("p/", 1));
    let cancel = CancellationToken::new();
    let clock = Arc::new(CountingClock {
        sleeps: Mutex::new(0),
        cancel: cancel.clone(),
    });
    let inbox = ArtifactInbox::new(store, "p/")
        .with_clock(clock.clone())
        .with_poll_interval(Duration::from_millis(1));
    let message = inbox.next(Some(&cancel)).await.unwrap();
    assert!(
        matches!(message, Some(RunnerMessage::Verdict { ref sha, .. }) if sha == "abc"),
        "{message:?}"
    );
    assert_eq!(*clock.sleeps.lock().unwrap(), 1); // exactly one retry
}
