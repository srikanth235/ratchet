//! The artifacts bucket as a message channel between the engine and a cloud runner: numbered objects under one prefix per agent
//! (`events/00000001.jsonl` runner to engine, `inbox/00000001.json` engine to runner), plus the stores that back it.
//!
//! Readers never skip an object they listed but could not read yet: `read_events` stops before an unreadable batch and
//! `ArtifactInbox::next` retries it after the poll interval, so a writer that has created an object but not filled it yet cannot
//! make a verdict or an event disappear.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{
    AgentEvent, ArtifactStore, CancellationToken, Clock, PolledEvents, PortError, PortResult,
    RunnerInbox, RunnerMessage,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::process::{RunProcessOptions, SharedSpawner, TokioSpawner, run_process};

/// Where one agent's runner state lives in the artifacts bucket (IAM allows exactly this prefix).
pub fn runner_prefix(optimization_id: &str, agent_id: &str) -> String {
    format!("artifacts/{optimization_id}/{agent_id}/")
}

pub fn event_key(prefix: &str, seq: u64) -> String {
    format!("{prefix}events/{seq:08}.jsonl")
}

pub fn inbox_key(prefix: &str, seq: u64) -> String {
    format!("{prefix}inbox/{seq:08}.json")
}

pub fn config_key(prefix: &str) -> String {
    format!("{prefix}config.json")
}

pub fn done_key(prefix: &str) -> String {
    format!("{prefix}done.json")
}

/// `artifacts/o/a/events/00000012.jsonl` -> `Some(12)` (`None` for anything else).
pub fn seq_of(key: &str) -> Option<u64> {
    let (head, ext) = key.rsplit_once('.')?;
    if ext.is_empty() || !ext.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let (_, digits) = head.rsplit_once('/')?;
    if digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Numbered {
    key: String,
    seq: u64,
}

async fn list_seqs(store: &dyn ArtifactStore, dir_prefix: &str) -> PortResult<Vec<Numbered>> {
    let mut items: Vec<Numbered> = store
        .list(dir_prefix)
        .await?
        .into_iter()
        .filter_map(|key| seq_of(&key).map(|seq| Numbered { key, seq }))
        .collect();
    items.sort_by_key(|n| n.seq);
    Ok(items)
}

/// Highest sequence number under `dir_prefix` (0 when empty).
pub async fn last_seq(store: &dyn ArtifactStore, dir_prefix: &str) -> PortResult<u64> {
    Ok(list_seqs(store, dir_prefix)
        .await?
        .last()
        .map_or(0, |n| n.seq))
}

// ---------------------------------------------------------------- inbox (engine -> runner)

struct InboxState {
    last: u64,
    buffered: Vec<Numbered>,
}

/// Read engine -> runner messages, numbered objects `inbox/00000001.json`, in order, exactly once.
pub struct ArtifactInbox {
    store: Arc<dyn ArtifactStore>,
    prefix: String,
    clock: Arc<dyn Clock>,
    poll_interval: Duration,
    state: AsyncMutex<InboxState>,
}

impl ArtifactInbox {
    pub fn new(store: Arc<dyn ArtifactStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into(),
            clock: Arc::new(SystemClock),
            poll_interval: Duration::from_millis(2000),
            state: AsyncMutex::new(InboxState {
                last: 0,
                buffered: Vec::new(),
            }),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Skip messages up to this sequence (for a runner restarted mid-run).
    pub fn starting_after(self, seq: u64) -> Self {
        self.state.try_lock().expect("not shared yet").last = seq;
        self
    }
}

#[async_trait]
impl RunnerInbox for ArtifactInbox {
    async fn next(&self, cancel: Option<&CancellationToken>) -> PortResult<Option<RunnerMessage>> {
        let never = CancellationToken::new();
        let cancel = cancel.unwrap_or(&never);
        let mut st = self.state.lock().await;
        while !cancel.is_cancelled() {
            if st.buffered.is_empty() {
                match list_seqs(self.store.as_ref(), &format!("{}inbox/", self.prefix)).await {
                    Ok(all) => {
                        let last = st.last;
                        st.buffered = all.into_iter().filter(|k| k.seq > last).collect();
                    }
                    Err(err) => tracing::warn!(%err, "listing the inbox failed; retrying"),
                }
            }
            if let Some(item) = st.buffered.first().cloned() {
                let body = match self.store.get(&item.key).await {
                    Ok(body) => body.filter(|b| !b.is_empty()),
                    Err(err) => {
                        tracing::warn!(%err, key = %item.key, "reading an inbox message failed; retrying");
                        None
                    }
                };
                let Some(body) = body else {
                    // Listed but not readable yet: keep it and retry after the poll interval, rather than dropping a verdict.
                    st.buffered.clear();
                    let _ = self
                        .clock
                        .sleep(self.poll_interval.as_millis() as u64, Some(cancel))
                        .await;
                    continue;
                };
                st.buffered.remove(0);
                st.last = item.seq;
                // a malformed message is skipped rather than wedging the runner
                if let Ok(message) = serde_json::from_str::<RunnerMessage>(&body) {
                    return Ok(Some(message));
                }
                continue;
            }
            let _ = self
                .clock
                .sleep(self.poll_interval.as_millis() as u64, Some(cancel))
                .await;
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------- events (runner -> engine)

#[derive(Debug, Clone, Copy, Default)]
pub struct EventSinkOptions {
    /// Default 2 s.
    pub flush_interval: Option<Duration>,
    /// Flush at once when this many events are buffered. Default 50.
    pub max_buffered: Option<usize>,
}

struct SinkInner {
    store: Arc<dyn ArtifactStore>,
    prefix: String,
    max_buffered: usize,
    buffer: Mutex<Vec<AgentEvent>>,
    /// Highest batch number written; `None` until the first flush has looked at the store (a restarted runner continues the numbering).
    seq: AsyncMutex<Option<u64>>,
    errors: Mutex<Vec<String>>,
    stop: CancellationToken,
}

/// Runner -> engine events: buffered, written as numbered JSONL objects (`events/00000001.jsonl`). Must be created inside a tokio
/// runtime (it flushes on a timer).
#[derive(Clone)]
pub struct ArtifactEventSink {
    inner: Arc<SinkInner>,
}

impl ArtifactEventSink {
    pub fn new(
        store: Arc<dyn ArtifactStore>,
        prefix: impl Into<String>,
        options: EventSinkOptions,
    ) -> Self {
        let inner = Arc::new(SinkInner {
            store,
            prefix: prefix.into(),
            max_buffered: options.max_buffered.unwrap_or(50),
            buffer: Mutex::new(Vec::new()),
            seq: AsyncMutex::new(None),
            errors: Mutex::new(Vec::new()),
            stop: CancellationToken::new(),
        });
        let interval = options
            .flush_interval
            .unwrap_or(Duration::from_millis(2000));
        let timer = inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = timer.stop.cancelled() => break,
                }
                Self::flush_inner(&timer).await;
            }
        });
        Self { inner }
    }

    pub fn emit(&self, event: AgentEvent) {
        let full = {
            let mut buffer = self.inner.buffer.lock().unwrap();
            buffer.push(event);
            buffer.len() >= self.inner.max_buffered
        };
        if full {
            let inner = self.inner.clone();
            tokio::spawn(async move { Self::flush_inner(&inner).await });
        }
    }

    /// An `emit` function to hand to the runner loop.
    pub fn emitter(&self) -> ratchet_core::ports::SharedEventSink {
        let sink = self.clone();
        Arc::new(move |e| sink.emit(e))
    }

    /// Write what is buffered (after any flush already in progress). A failed write keeps the events for the next flush.
    pub async fn flush(&self) {
        Self::flush_inner(&self.inner).await
    }

    async fn flush_inner(inner: &SinkInner) {
        let mut seq = inner.seq.lock().await; // serializes flushes
        if inner.buffer.lock().unwrap().is_empty() {
            return;
        }
        let current = match *seq {
            Some(n) => n,
            None => match last_seq(inner.store.as_ref(), &format!("{}events/", inner.prefix)).await
            {
                Ok(n) => n,
                Err(err) => {
                    inner.errors.lock().unwrap().push(err.to_string());
                    return;
                }
            },
        };
        let batch: Vec<AgentEvent> = std::mem::take(&mut *inner.buffer.lock().unwrap());
        let mut body = batch
            .iter()
            .map(|e| serde_json::to_string(e).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        body.push('\n');
        match inner
            .store
            .put(&event_key(&inner.prefix, current + 1), body.as_bytes())
            .await
        {
            Ok(()) => *seq = Some(current + 1),
            Err(err) => {
                *seq = Some(current);
                let mut buffer = inner.buffer.lock().unwrap();
                let newer = std::mem::take(&mut *buffer);
                *buffer = batch;
                buffer.extend(newer); // retry on the next flush, in order
                inner.errors.lock().unwrap().push(err.to_string());
            }
        }
    }

    /// Errors from failed writes so far.
    pub fn errors(&self) -> Vec<String> {
        self.inner.errors.lock().unwrap().clone()
    }

    pub async fn close(&self) {
        self.inner.stop.cancel();
        self.flush().await;
    }
}

/// Parse every event line from the batches after `cursor`. Returns the new cursor (highest seq read). Stops before a batch that is
/// listed but not readable yet, so the next poll retries it instead of skipping it for good.
pub async fn read_events(
    store: &dyn ArtifactStore,
    prefix: &str,
    cursor: Option<&str>,
) -> PortResult<PolledEvents> {
    let after: u64 = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
    let batches: Vec<Numbered> = list_seqs(store, &format!("{prefix}events/"))
        .await?
        .into_iter()
        .filter(|k| k.seq > after)
        .collect();
    let mut events = Vec::new();
    let mut last = after;
    for Numbered { key, seq } in batches {
        let body = store.get(&key).await?;
        // Not readable yet (or empty): stop before it so the next poll retries it, rather than skipping it for good.
        let Some(body) = body.filter(|b| !b.is_empty()) else {
            break;
        };
        for line in body.split('\n') {
            if line.trim().is_empty() {
                continue;
            }
            // skip a torn line
            if let Ok(event) = serde_json::from_str::<AgentEvent>(line) {
                events.push(event);
            }
        }
        last = seq;
    }
    Ok(PolledEvents {
        events,
        cursor: last.to_string(),
    })
}

// ---------------------------------------------------------------- stores

/// S3 through the `aws` CLI (present on the runner image; credentials come from the instance profile). The real S3 store lives in
/// `ratchet-sandbox` and is used by the engine; this one lets the runner stay dependency-free.
pub struct AwsCliArtifactStore {
    bucket: String,
    spawner: SharedSpawner,
    env: HashMap<String, String>,
    region: Option<String>,
}

struct AwsOutput {
    code: Option<i32>,
    out: String,
    err: String,
}

impl AwsCliArtifactStore {
    pub fn new(
        bucket: impl Into<String>,
        spawner: Option<SharedSpawner>,
        env: Option<HashMap<String, String>>,
        region: Option<String>,
    ) -> Self {
        Self {
            bucket: bucket.into(),
            spawner: spawner.unwrap_or_else(|| Arc::new(TokioSpawner)),
            env: env.unwrap_or_else(|| std::env::vars().collect()),
            region,
        }
    }

    async fn aws(&self, args: &[&str], stdin: Option<String>) -> AwsOutput {
        let mut all: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        if let Some(region) = &self.region {
            all.extend(["--region".to_string(), region.clone()]);
        }
        let mut out = String::new();
        let mut collect = |l: &str| {
            out.push_str(l);
            out.push('\n');
        };
        let mut options = RunProcessOptions::new(
            std::env::current_dir().unwrap_or_else(|_| ".".into()),
            self.env.clone(),
        );
        options.stdin = stdin;
        options.on_stdout_line = Some(&mut collect);
        let res = run_process(self.spawner.as_ref(), "aws", &all, options).await;
        AwsOutput {
            code: res.code(),
            out,
            err: res.stderr_tail,
        }
    }
}

#[async_trait]
impl ArtifactStore for AwsCliArtifactStore {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        let text = String::from_utf8_lossy(body).into_owned();
        let target = format!("s3://{}/{key}", self.bucket);
        let r = self
            .aws(
                &["s3", "cp", "-", &target, "--only-show-errors"],
                Some(text),
            )
            .await;
        if r.code != Some(0) {
            return Err(PortError::failed(format!(
                "aws s3 cp failed for {key}: {}",
                r.err
            )));
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        let source = format!("s3://{}/{key}", self.bucket);
        let r = self
            .aws(&["s3", "cp", &source, "-", "--only-show-errors"], None)
            .await;
        if r.code == Some(0) {
            return Ok(Some(r.out)); // line-oriented: blank lines are not preserved (fine for JSON / JSONL)
        }
        if ["NoSuchKey", "Not Found", "404"]
            .iter()
            .any(|m| r.err.contains(m))
        {
            return Ok(None);
        }
        Err(PortError::failed(format!(
            "aws s3 cp failed for {key}: {}",
            r.err
        )))
    }

    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        let r = self
            .aws(
                &[
                    "s3api",
                    "list-objects-v2",
                    "--bucket",
                    &self.bucket,
                    "--prefix",
                    prefix,
                    "--query",
                    "Contents[].Key",
                    "--output",
                    "json",
                ],
                None,
            )
            .await;
        if r.code != Some(0) {
            return Err(PortError::failed(format!(
                "aws s3api list-objects-v2 failed: {}",
                r.err
            )));
        }
        let text = r.out.trim();
        let parsed: Option<Vec<String>> =
            serde_json::from_str(if text.is_empty() { "null" } else { text })
                .map_err(|e| PortError::failed(e.to_string()))?;
        Ok(parsed.unwrap_or_default())
    }
}
