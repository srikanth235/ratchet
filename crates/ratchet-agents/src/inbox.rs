//! In-process inbox: `push` from the engine side, `next` from the loop.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::domain::{RunnerInbox, RunnerMessage};

#[derive(Default)]
pub struct MemoryInbox {
    queue: Mutex<VecDeque<RunnerMessage>>,
    notify: Notify,
}

impl MemoryInbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, message: RunnerMessage) {
        self.queue.lock().unwrap().push_back(message);
        self.notify.notify_one();
    }

    /// Messages waiting to be read.
    pub fn pending(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

#[async_trait]
impl RunnerInbox for MemoryInbox {
    async fn next(&self, cancel: &CancellationToken) -> Option<RunnerMessage> {
        loop {
            if let Some(m) = self.queue.lock().unwrap().pop_front() {
                return Some(m);
            }
            if cancel.is_cancelled() {
                return None;
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = cancel.cancelled() => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn queued_messages_come_out_in_order_and_waiters_wake() {
        let inbox = Arc::new(MemoryInbox::new());
        inbox.push(RunnerMessage::Pause);
        inbox.push(RunnerMessage::Resume);
        assert_eq!(inbox.pending(), 2);
        let cancel = CancellationToken::new();
        assert_eq!(inbox.next(&cancel).await, Some(RunnerMessage::Pause));
        assert_eq!(inbox.next(&cancel).await, Some(RunnerMessage::Resume));
        let waiting = {
            let (inbox, cancel) = (inbox.clone(), cancel.clone());
            tokio::spawn(async move { inbox.next(&cancel).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        inbox.push(RunnerMessage::Stop);
        assert_eq!(waiting.await.unwrap(), Some(RunnerMessage::Stop));
    }

    #[tokio::test]
    async fn abort_returns_none_and_keeps_later_messages() {
        let inbox = Arc::new(MemoryInbox::new());
        let cancel = CancellationToken::new();
        let waiting = {
            let (inbox, cancel) = (inbox.clone(), cancel.clone());
            tokio::spawn(async move { inbox.next(&cancel).await })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel.cancel();
        assert_eq!(waiting.await.unwrap(), None);
        inbox.push(RunnerMessage::Stop);
        assert_eq!(
            inbox.next(&CancellationToken::new()).await,
            Some(RunnerMessage::Stop)
        );
        // an already-aborted token still drains what is queued, then reports None
        inbox.push(RunnerMessage::Pause);
        assert_eq!(inbox.next(&cancel).await, Some(RunnerMessage::Pause));
        assert_eq!(inbox.next(&cancel).await, None);
    }
}
