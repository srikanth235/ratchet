//! Child processes: the `Spawner` port (real: tokio, in its own process group; fake: `testing::FakeSpawner`), line splitting of
//! chunked output, and `run_process`, which feeds stdout lines to a callback and kills the whole group on abort.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct SpawnOptions {
    pub cwd: PathBuf,
    /// The complete environment of the child (nothing is inherited).
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSignal {
    Term,
    Kill,
}

impl KillSignal {
    pub fn name(self) -> &'static str {
        match self {
            KillSignal::Term => "SIGTERM",
            KillSignal::Kill => "SIGKILL",
        }
    }
}

impl fmt::Display for KillSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How a process ended. `spawn_error` is set when it could not be started at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessExit {
    pub code: Option<i32>,
    /// Name of the terminating signal (`SIGTERM`, ...).
    pub signal: Option<String>,
    pub spawn_error: Option<String>,
}

impl ProcessExit {
    pub fn code(code: i32) -> Self {
        Self { code: Some(code), ..Self::default() }
    }
    pub fn spawn_failed(message: impl Into<String>) -> Self {
        Self { spawn_error: Some(message.into()), ..Self::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Stdout(String),
    Stderr(String),
}

/// The slice of a child process the adapters use; real or fake. Output arrives on a channel that closes once the process has
/// exited and its stdio is drained; the exit status follows on the second channel.
pub trait ProcessHandle: Send {
    /// Take the output stream (call once).
    fn take_output(&mut self) -> mpsc::UnboundedReceiver<Output>;
    /// Take the exit notification (call once).
    fn take_exit(&mut self) -> oneshot::Receiver<ProcessExit>;
    fn write_stdin(&self, data: &str);
    fn end_stdin(&self);
    /// Signal the whole process group (the CLI and every tool it started).
    fn kill(&self, signal: KillSignal);
}

pub trait Spawner: Send + Sync {
    fn spawn(&self, command: &str, args: &[String], options: &SpawnOptions) -> Box<dyn ProcessHandle>;
}

pub type SharedSpawner = Arc<dyn Spawner>;

// ---------------------------------------------------------------- real spawner

/// Spawns real processes in a new process group so abort can kill the CLI and everything under it.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSpawner;

enum StdinMsg {
    Data(String),
    End,
}

struct TokioProcess {
    output: Option<mpsc::UnboundedReceiver<Output>>,
    exit: Option<oneshot::Receiver<ProcessExit>>,
    stdin: mpsc::UnboundedSender<StdinMsg>,
    pid: Option<u32>,
}

fn signal_number(signal: KillSignal) -> i32 {
    match signal {
        KillSignal::Term => libc::SIGTERM,
        KillSignal::Kill => libc::SIGKILL,
    }
}

fn signal_name(number: i32) -> String {
    match number {
        libc::SIGHUP => "SIGHUP".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGQUIT => "SIGQUIT".into(),
        libc::SIGABRT => "SIGABRT".into(),
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        libc::SIGPIPE => "SIGPIPE".into(),
        libc::SIGALRM => "SIGALRM".into(),
        libc::SIGTERM => "SIGTERM".into(),
        n => format!("SIG{n}"),
    }
}

/// Node-style spawn error text, e.g. `spawn claude ENOENT`.
fn spawn_error_text(command: &str, err: &std::io::Error) -> String {
    let code = match err.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        _ => return format!("spawn {command} failed: {err}"),
    };
    format!("spawn {command} {code}")
}

/// Streams a pipe as UTF-8 text chunks; a multi-byte character split across reads is held back until it is complete.
async fn pump_pipe<R: AsyncReadExt + Unpin>(mut reader: R, tx: mpsc::UnboundedSender<Output>, wrap: fn(String) -> Output) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        let valid = match std::str::from_utf8(&carry) {
            Ok(_) => carry.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => carry.len(),
        };
        if valid > 0 {
            let text = String::from_utf8_lossy(&carry[..valid]).into_owned();
            carry.drain(..valid);
            if tx.send(wrap(text)).is_err() {
                break;
            }
        }
    }
    if !carry.is_empty() {
        let _ = tx.send(wrap(String::from_utf8_lossy(&carry).into_owned()));
    }
}

impl Spawner for TokioSpawner {
    fn spawn(&self, command: &str, args: &[String], options: &SpawnOptions) -> Box<dyn ProcessHandle> {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (exit_tx, exit_rx) = oneshot::channel();
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<StdinMsg>();

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .current_dir(&options.cwd)
            .env_clear()
            .envs(&options.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = exit_tx.send(ProcessExit::spawn_failed(spawn_error_text(command, &err)));
                return Box::new(TokioProcess { output: Some(out_rx), exit: Some(exit_rx), stdin: stdin_tx, pid: None });
            }
        };
        let pid = child.id();
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // EPIPE when the child exits before reading its prompt is ignored.
        tokio::spawn(async move {
            while let Some(msg) = stdin_rx.recv().await {
                match msg {
                    StdinMsg::Data(data) => {
                        if let Some(s) = stdin.as_mut() {
                            if s.write_all(data.as_bytes()).await.is_err() {
                                stdin = None;
                            }
                        }
                    }
                    StdinMsg::End => {
                        if let Some(mut s) = stdin.take() {
                            let _ = s.shutdown().await;
                        }
                    }
                }
            }
            drop(stdin);
        });

        tokio::spawn(async move {
            let out_task = stdout.map(|r| tokio::spawn(pump_pipe(r, out_tx.clone(), Output::Stdout)));
            let err_task = stderr.map(|r| tokio::spawn(pump_pipe(r, out_tx.clone(), Output::Stderr)));
            let status = child.wait().await;
            // like node's `close`: the exit is reported once stdio is drained
            if let Some(t) = out_task {
                let _ = t.await;
            }
            if let Some(t) = err_task {
                let _ = t.await;
            }
            drop(out_tx);
            let exit = match status {
                Ok(status) => {
                    use std::os::unix::process::ExitStatusExt;
                    ProcessExit { code: status.code(), signal: status.signal().map(signal_name), spawn_error: None }
                }
                Err(err) => ProcessExit::spawn_failed(err.to_string()),
            };
            let _ = exit_tx.send(exit);
        });

        Box::new(TokioProcess { output: Some(out_rx), exit: Some(exit_rx), stdin: stdin_tx, pid })
    }
}

impl ProcessHandle for TokioProcess {
    fn take_output(&mut self) -> mpsc::UnboundedReceiver<Output> {
        self.output.take().expect("output already taken")
    }
    fn take_exit(&mut self) -> oneshot::Receiver<ProcessExit> {
        self.exit.take().expect("exit already taken")
    }
    fn write_stdin(&self, data: &str) {
        let _ = self.stdin.send(StdinMsg::Data(data.to_string()));
    }
    fn end_stdin(&self) {
        let _ = self.stdin.send(StdinMsg::End);
    }
    fn kill(&self, signal: KillSignal) {
        let Some(pid) = self.pid else { return };
        let sig = signal_number(signal);
        // SAFETY: plain `kill(2)` calls with a pid we spawned; a negative pid addresses its process group.
        unsafe {
            if libc::kill(-(pid as i32), sig) != 0 {
                let _ = libc::kill(pid as i32, sig); // not a group leader (or already gone)
            }
        }
    }
}

// ---------------------------------------------------------------- line splitting

/// Splits a chunked stream into lines; a trailing partial line is held until its newline (or `flush`).
#[derive(Debug, Default)]
pub struct LineSplitter {
    rest: String,
}

impl LineSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.rest.push_str(chunk);
        let mut lines: Vec<&str> = self.rest.split('\n').collect();
        let tail = lines.pop().unwrap_or("").to_string();
        let out = lines
            .into_iter()
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        self.rest = tail;
        out
    }

    pub fn flush(&mut self) -> Vec<String> {
        let last = std::mem::take(&mut self.rest);
        let last = last.trim();
        if last.is_empty() { Vec::new() } else { vec![last.to_string()] }
    }
}

// ---------------------------------------------------------------- run_process

pub struct RunProcessOptions<'a> {
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub stdin: Option<String>,
    pub cancel: Option<&'a CancellationToken>,
    /// Grace between SIGTERM and SIGKILL on abort. Default 5 s.
    pub kill_grace: Duration,
    pub on_stdout_line: Option<&'a mut (dyn FnMut(&str) + Send)>,
}

impl RunProcessOptions<'_> {
    pub fn new(cwd: impl Into<PathBuf>, env: HashMap<String, String>) -> Self {
        Self { cwd: cwd.into(), env, stdin: None, cancel: None, kill_grace: Duration::from_millis(5000), on_stdout_line: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProcessResult {
    pub exit: ProcessExit,
    pub aborted: bool,
    /// Last ~4 KB of stderr, for error messages.
    pub stderr_tail: String,
}

impl RunProcessResult {
    pub fn code(&self) -> Option<i32> {
        self.exit.code
    }
    pub fn signal(&self) -> Option<&str> {
        self.exit.signal.as_deref()
    }
    pub fn spawn_error(&self) -> Option<&str> {
        self.exit.spawn_error.as_deref()
    }
}

const STDERR_TAIL: usize = 4096;

fn keep_tail(text: &mut String, max: usize) {
    if text.len() > max {
        let mut cut = text.len() - max;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text.drain(..cut);
    }
}

pub async fn run_process(spawner: &dyn Spawner, command: &str, args: &[String], options: RunProcessOptions<'_>) -> RunProcessResult {
    let RunProcessOptions { cwd, env, stdin, cancel, kill_grace, mut on_stdout_line } = options;
    let mut child = spawner.spawn(command, args, &SpawnOptions { cwd, env });
    let mut output = child.take_output();
    let exit = child.take_exit();
    let mut lines = LineSplitter::new();
    let mut stderr_tail = String::new();
    if let Some(data) = stdin {
        child.write_stdin(&data);
    }
    child.end_stdin();

    let never = CancellationToken::new();
    let cancel = cancel.unwrap_or(&never);
    let mut aborted = false;
    let mut kill_at: Option<tokio::time::Instant> = None;
    let begin_abort = |child: &dyn ProcessHandle, aborted: &mut bool, kill_at: &mut Option<tokio::time::Instant>| {
        *aborted = true;
        child.kill(KillSignal::Term);
        *kill_at = Some(tokio::time::Instant::now() + kill_grace);
    };
    if cancel.is_cancelled() {
        begin_abort(child.as_ref(), &mut aborted, &mut kill_at);
    }

    loop {
        let deadline = kill_at;
        let kill_wait = async move {
            match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled(), if !aborted => begin_abort(child.as_ref(), &mut aborted, &mut kill_at),
            _ = kill_wait, if kill_at.is_some() => {
                child.kill(KillSignal::Kill);
                kill_at = None;
            }
            item = output.recv() => match item {
                Some(Output::Stdout(chunk)) => {
                    for line in lines.push(&chunk) {
                        if let Some(cb) = on_stdout_line.as_mut() {
                            cb(&line);
                        }
                    }
                }
                Some(Output::Stderr(chunk)) => {
                    stderr_tail.push_str(&chunk);
                    keep_tail(&mut stderr_tail, STDERR_TAIL);
                }
                None => break,
            },
        }
    }
    let exit = exit.await.unwrap_or_else(|_| ProcessExit::spawn_failed("process handle dropped"));
    for line in lines.flush() {
        if let Some(cb) = on_stdout_line.as_mut() {
            cb(&line);
        }
    }
    RunProcessResult { exit, aborted, stderr_tail: stderr_tail.trim().to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_splitter_handles_crlf_blank_lines_and_partial_tails() {
        let mut s = LineSplitter::new();
        assert_eq!(s.push("a\r\n\nb"), vec!["a"]);
        assert_eq!(s.push("c\nd"), vec!["bc"]);
        assert_eq!(s.flush(), vec!["d"]);
        assert!(s.flush().is_empty());
    }

    #[test]
    fn stderr_tail_keeps_the_end_on_a_char_boundary() {
        let mut t = "é".repeat(3000);
        keep_tail(&mut t, STDERR_TAIL);
        assert!(t.len() <= STDERR_TAIL && t.chars().all(|c| c == 'é'));
    }
}
