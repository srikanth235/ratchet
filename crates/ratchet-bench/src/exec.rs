//! Running a shell command in its own process group (port of `exec.ts`).

use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use ratchet_core::ports::CancellationToken;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// Which pipe a chunk of output came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Called with every chunk of output (stdout and stderr), e.g. to feed a log.
pub type OnData = Arc<dyn Fn(&str, Stream) + Send + Sync>;

#[derive(Clone)]
pub struct ProcessOptions {
    pub cwd: PathBuf,
    /// Merged over this process's environment.
    pub env: Vec<(String, String)>,
    /// Kill the whole process group after this many ms.
    pub timeout_ms: Option<u64>,
    /// Kill the whole process group when cancelled.
    pub cancel: Option<CancellationToken>,
    pub on_data: Option<OnData>,
    /// Keep at most this many trailing characters of stdout in `ProcessResult::stdout`. Default 8 Mi.
    pub max_capture_chars: usize,
}

impl ProcessOptions {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        ProcessOptions {
            cwd: cwd.into(),
            env: Vec::new(),
            timeout_ms: None,
            cancel: None,
            on_data: None,
            max_capture_chars: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessResult {
    pub exit_code: Option<i32>,
    /// The signal that terminated the process, if any.
    pub signal: Option<i32>,
    /// The (tail of the) stdout; stderr is only delivered through `on_data`.
    pub stdout: String,
    pub timed_out: bool,
    pub aborted: bool,
    pub duration_ms: f64,
    /// Set when the process could not be spawned at all.
    pub spawn_error: Option<String>,
}

/// `NodeJS.Signals` name of a signal number (`SIGKILL`), as the log line prints it.
pub fn signal_name(sig: i32) -> String {
    Signal::try_from(sig).map_or_else(|_| format!("SIG{sig}"), |s| s.as_str().to_owned())
}

/// Decodes UTF-8 across chunk boundaries (a multi-byte character split between two reads is not mangled).
#[derive(Default)]
struct Utf8Chunker {
    pending: Vec<u8>,
}

impl Utf8Chunker {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(&String::from_utf8_lossy(&self.pending[..valid]));
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{fffd}');
                            self.pending.drain(..valid + n);
                        }
                        None => {
                            self.pending.drain(..valid);
                            return out; // an incomplete character at the end: wait for the next chunk
                        }
                    }
                }
            }
        }
    }

    fn finish(&mut self) -> String {
        let s = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        s
    }
}

struct Shared {
    stdout: Mutex<String>,
    cap: usize,
    on_data: Option<OnData>,
}

async fn pump<R: AsyncRead + Unpin>(mut r: R, stream: Stream, shared: Arc<Shared>) {
    let mut chunker = Utf8Chunker::default();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let text = chunker.push(&buf[..n]);
        deliver(&shared, stream, &text);
    }
    let rest = chunker.finish();
    deliver(&shared, stream, &rest);
}

fn deliver(shared: &Shared, stream: Stream, text: &str) {
    if text.is_empty() {
        return;
    }
    if stream == Stream::Stdout
        && let Ok(mut out) = shared.stdout.lock()
    {
        out.push_str(text);
        if out.len() > shared.cap * 2 {
            let chars = out.chars().count();
            if chars > shared.cap * 2 {
                let keep: String = out.chars().skip(chars - shared.cap).collect();
                *out = keep;
            }
        }
    }
    if let Some(f) = &shared.on_data {
        f(text, stream);
    }
}

fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid
        && let Ok(raw) = i32::try_from(pid)
    {
        let _ = killpg(Pid::from_raw(raw), Signal::SIGKILL); // group already gone: fine
    }
}

/// Run `command` with `bash -c` in its own process group, so a timeout or abort kills the shell AND everything it spawned
/// (bench server, load generator, ...). Never fails. After the shell exits the group is killed again to clear stragglers
/// that would otherwise outlive the step.
pub async fn run_process(command: &str, opts: &ProcessOptions) -> ProcessResult {
    let t0 = Instant::now();
    let mut res = ProcessResult::default();
    if opts.cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
        res.aborted = true;
        return res;
    }
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(&opts.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0); // own process group
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            res.spawn_error = Some(e.to_string());
            return res;
        }
    };
    let pid = child.id();
    let shared = Arc::new(Shared {
        stdout: Mutex::new(String::new()),
        cap: opts.max_capture_chars,
        on_data: opts.on_data.clone(),
    });
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(tokio::spawn(pump(out, Stream::Stdout, shared.clone())));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(tokio::spawn(pump(err, Stream::Stderr, shared.clone())));
    }

    let timeout = async {
        match opts.timeout_ms {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(timeout);
    let cancelled = async {
        match &opts.cancel {
            Some(c) => c.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(cancelled);

    let mut timed_out = false;
    let mut aborted = false;
    let status = loop {
        tokio::select! {
            s = child.wait() => break s,
            () = &mut timeout, if !timed_out => {
                timed_out = true;
                kill_group(pid);
            }
            () = &mut cancelled, if !aborted => {
                aborted = true;
                kill_group(pid);
            }
        }
    };
    kill_group(pid); // stragglers (orphaned servers) must not outlive the step
    // The pipes close once the group is gone; do not wait forever for a grandchild that escaped it.
    let drain = async {
        for r in &mut readers {
            let _ = r.await;
        }
    };
    if tokio::time::timeout(Duration::from_millis(1000), drain).await.is_err() {
        for r in &readers {
            r.abort();
        }
    }

    match status {
        Ok(s) => {
            res.exit_code = s.code();
            res.signal = s.signal();
        }
        Err(e) => res.spawn_error = Some(e.to_string()),
    }
    let mut stdout = shared.stdout.lock().map(|s| s.clone()).unwrap_or_default();
    let n = stdout.chars().count();
    if n > opts.max_capture_chars {
        stdout = stdout.chars().skip(n - opts.max_capture_chars).collect();
    }
    res.stdout = stdout;
    res.timed_out = timed_out;
    res.aborted = aborted;
    res.duration_ms = t0.elapsed().as_secs_f64() * 1000.0;
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunker_joins_split_characters() {
        let mut c = Utf8Chunker::default();
        let bytes = "aé€😀b".as_bytes().to_vec();
        let mut out = String::new();
        for b in &bytes {
            out.push_str(&c.push(std::slice::from_ref(b)));
        }
        out.push_str(&c.finish());
        assert_eq!(out, "aé€😀b");
    }

    #[test]
    fn signal_names() {
        assert_eq!(signal_name(9), "SIGKILL");
        assert_eq!(signal_name(15), "SIGTERM");
    }
}
