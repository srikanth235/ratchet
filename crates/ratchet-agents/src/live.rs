//! Plumbing for live sessions: one CLI process held open, lines in and out, and a small JSON-RPC client on top of it for the
//! two protocols that need one (`codex app-server` and `grok agent stdio`, ACP).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use crate::process::LineSplitter;
use crate::process::{KillSignal, Output, ProcessExit, ProcessHandle, SpawnOptions, Spawner};

/// Writes lines to the child's stdin; cloneable so the reader callback can answer the CLI (permission requests, say).
#[derive(Clone)]
pub struct LineWriter {
    handle: Arc<Mutex<Box<dyn ProcessHandle>>>,
}

impl LineWriter {
    pub fn write_line(&self, line: &str) {
        let h = self.handle.lock().unwrap();
        h.write_stdin(&format!("{line}\n"));
    }

    pub fn write_json(&self, value: &Value) {
        self.write_line(&value.to_string());
    }

    fn end_stdin(&self) {
        self.handle.lock().unwrap().end_stdin();
    }

    fn kill(&self, signal: KillSignal) {
        self.handle.lock().unwrap().kill(signal);
    }
}

/// A child process that stays up between turns. A background task feeds every stdout line to `on_line` in order.
pub struct LiveProcess {
    writer: LineWriter,
    stderr: Arc<Mutex<String>>,
    exit: Arc<Mutex<Option<ProcessExit>>>,
    exited: watch::Receiver<bool>,
}

const STDERR_KEEP: usize = 4096;

impl LiveProcess {
    /// Start `command`. `on_line(writer, line)` runs on the reader task for each stdout line; `on_exit` runs once, after the
    /// output is drained.
    pub fn spawn(
        spawner: &dyn Spawner,
        command: &str,
        args: &[String],
        options: &SpawnOptions,
        mut on_line: impl FnMut(&LineWriter, &str) + Send + 'static,
        on_exit: impl FnOnce(&ProcessExit) + Send + 'static,
    ) -> Self {
        let mut child = spawner.spawn(command, args, options);
        let mut output = child.take_output();
        let exit_rx = child.take_exit();
        let writer = LineWriter {
            handle: Arc::new(Mutex::new(child)),
        };
        let stderr = Arc::new(Mutex::new(String::new()));
        let exit = Arc::new(Mutex::new(None));
        let (exited_tx, exited) = watch::channel(false);
        let task_writer = writer.clone();
        let task_stderr = stderr.clone();
        let task_exit = exit.clone();
        tokio::spawn(async move {
            let mut lines = LineSplitter::new();
            while let Some(item) = output.recv().await {
                match item {
                    Output::Stdout(chunk) => {
                        for line in lines.push(&chunk) {
                            on_line(&task_writer, &line);
                        }
                    }
                    Output::Stderr(chunk) => {
                        let mut tail = task_stderr.lock().unwrap();
                        tail.push_str(&chunk);
                        if tail.len() > STDERR_KEEP {
                            let mut cut = tail.len() - STDERR_KEEP;
                            while !tail.is_char_boundary(cut) {
                                cut += 1;
                            }
                            tail.drain(..cut);
                        }
                    }
                }
            }
            let status = exit_rx
                .await
                .unwrap_or_else(|_| ProcessExit::spawn_failed("process handle dropped"));
            for line in lines.flush() {
                on_line(&task_writer, &line);
            }
            *task_exit.lock().unwrap() = Some(status.clone());
            on_exit(&status);
            let _ = exited_tx.send(true);
        });
        Self {
            writer,
            stderr,
            exit,
            exited,
        }
    }

    pub fn writer(&self) -> LineWriter {
        self.writer.clone()
    }

    pub fn write_json(&self, value: &Value) {
        self.writer.write_json(value);
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().unwrap().trim().to_string()
    }

    /// How the process ended, once it has.
    pub fn exit(&self) -> Option<ProcessExit> {
        self.exit.lock().unwrap().clone()
    }

    pub fn is_running(&self) -> bool {
        self.exit().is_none()
    }

    /// Wait up to `limit` for the process to be gone.
    pub async fn wait_exit(&self, limit: Duration) -> bool {
        let mut rx = self.exited.clone();
        tokio::time::timeout(limit, async move {
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .is_ok()
    }

    /// Close stdin, give the CLI `grace` to leave, then SIGTERM, then SIGKILL (the whole process group).
    pub async fn close(&self, grace: Duration) {
        self.writer.end_stdin();
        if self.wait_exit(grace).await {
            return;
        }
        self.writer.kill(KillSignal::Term);
        if self.wait_exit(grace).await {
            return;
        }
        self.writer.kill(KillSignal::Kill);
        self.wait_exit(grace).await;
    }

    pub fn kill(&self, signal: KillSignal) {
        self.writer.kill(signal);
    }
}

// ---------------------------------------------------------------- JSON-RPC

/// One line from a JSON-RPC peer.
#[derive(Debug, Clone, PartialEq)]
pub enum RpcMessage {
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<Value>,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// The peer asks us something (a permission prompt, say). It needs an answer with the same `id`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

/// Parse one line. `None` for anything that is not a JSON-RPC message (counted as malformed by the caller).
pub fn parse_rpc(line: &str) -> Option<RpcMessage> {
    let v: Value = serde_json::from_str(line).ok()?;
    let o = v.as_object()?;
    let method = o.get("method").and_then(Value::as_str);
    let id = o.get("id").filter(|i| !i.is_null());
    match (method, id) {
        (Some(m), Some(id)) => Some(RpcMessage::Request {
            id: id.clone(),
            method: m.to_string(),
            params: o.get("params").cloned().unwrap_or(Value::Null),
        }),
        (Some(m), None) => Some(RpcMessage::Notification {
            method: m.to_string(),
            params: o.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, Some(id)) if o.contains_key("result") || o.contains_key("error") => {
            Some(RpcMessage::Response {
                id: id.clone(),
                result: o.get("result").cloned(),
                error: o.get("error").filter(|e| !e.is_null()).cloned(),
            })
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RpcError {
    /// The peer answered with an `error` object.
    Rpc(Value),
    /// The process went away before answering.
    Closed,
    Timeout,
}

impl RpcError {
    /// The peer's error message, or a description of what went wrong.
    pub fn message(&self) -> String {
        match self {
            RpcError::Rpc(e) => e
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| e.to_string(), str::to_string),
            RpcError::Closed => "the CLI exited before answering".to_string(),
            RpcError::Timeout => "the CLI did not answer in time".to_string(),
        }
    }
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, Value>>>>>;

/// A JSON-RPC client over a [`LiveProcess`]: requests wait for their response; notifications and the peer's requests go to the
/// callback given to [`RpcClient::spawn`], in order, on the reader task.
pub struct RpcClient {
    process: LiveProcess,
    pending: Pending,
    next_id: AtomicU64,
    /// Write `"jsonrpc":"2.0"` (ACP does; Codex's app-server omits it).
    tag: bool,
}

impl RpcClient {
    pub fn spawn(
        spawner: &dyn Spawner,
        command: &str,
        args: &[String],
        options: &SpawnOptions,
        tag: bool,
        mut on_message: impl FnMut(&LineWriter, RpcMessage) + Send + 'static,
        on_exit: impl FnOnce(&ProcessExit) + Send + 'static,
    ) -> Self {
        let pending: Pending = Arc::default();
        let p = pending.clone();
        let p_exit = pending.clone();
        let process = LiveProcess::spawn(
            spawner,
            command,
            args,
            options,
            move |writer, line| match parse_rpc(line) {
                Some(RpcMessage::Response { id, result, error }) => {
                    let key = id.to_string();
                    if let Some(tx) = p.lock().unwrap().remove(&key) {
                        let _ = tx.send(match error {
                            Some(e) => Err(e),
                            None => Ok(result.unwrap_or(Value::Null)),
                        });
                    }
                }
                Some(other) => on_message(writer, other),
                None => {}
            },
            move |exit| {
                // dropping the senders wakes every waiting request with `Closed`
                p_exit.lock().unwrap().clear();
                on_exit(exit);
            },
        );
        Self {
            process,
            pending,
            next_id: AtomicU64::new(1),
            tag,
        }
    }

    pub fn process(&self) -> &LiveProcess {
        &self.process
    }

    fn envelope(&self, mut body: Value) -> Value {
        if self.tag
            && let Some(o) = body.as_object_mut()
        {
            o.insert("jsonrpc".into(), json!("2.0"));
        }
        body
    }

    pub async fn request(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.to_string(), tx);
        if !self.process.is_running() {
            self.pending.lock().unwrap().remove(&id.to_string());
            return Err(RpcError::Closed);
        }
        self.process
            .write_json(&self.envelope(json!({"id": id, "method": method, "params": params})));
        match tokio::time::timeout(limit, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(error))) => Err(RpcError::Rpc(error)),
            Ok(Err(_)) => Err(RpcError::Closed),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id.to_string());
                Err(RpcError::Timeout)
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.process
            .write_json(&self.envelope(json!({"method": method, "params": params})));
    }
}

/// An answer to a request of the peer's, as `on_message` writes it.
pub fn rpc_reply(tag: bool, id: &Value, result: Value) -> Value {
    let mut v = json!({"id": id, "result": result});
    if tag {
        v["jsonrpc"] = json!("2.0");
    }
    v
}
