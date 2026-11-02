//! Test doubles that are part of the public API because simulation and other crates use them too.

use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};

use crate::process::{KillSignal, Output, ProcessExit, ProcessHandle, SpawnOptions, Spawner};

#[derive(Debug, Clone, Default)]
pub struct FakeProcessSpec {
    /// Full stdout text, delivered in `chunk_size` pieces (default: one chunk per line) after stdin closes.
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub chunk_size: Option<usize>,
    /// Never exits on its own; exits with the signal when killed (simulates a long-running agent turn).
    pub hang: bool,
    /// Spawn fails (ENOENT-style) with this message.
    pub spawn_error: Option<String>,
}

impl FakeProcessSpec {
    pub fn stdout(text: impl Into<String>) -> Self {
        Self {
            stdout: text.into(),
            ..Self::default()
        }
    }
    pub fn exit(code: i32) -> Self {
        Self {
            exit_code: code,
            ..Self::default()
        }
    }
    pub fn hang() -> Self {
        Self {
            hang: true,
            ..Self::default()
        }
    }
    pub fn spawn_error(message: impl Into<String>) -> Self {
        Self {
            spawn_error: Some(message.into()),
            ..Self::default()
        }
    }
    pub fn with_stderr(mut self, text: impl Into<String>) -> Self {
        self.stderr = text.into();
        self
    }
    pub fn with_stdout(mut self, text: impl Into<String>) -> Self {
        self.stdout = text.into();
        self
    }
    pub fn with_exit(mut self, code: i32) -> Self {
        self.exit_code = code;
        self
    }
    pub fn with_chunk_size(mut self, n: usize) -> Self {
        self.chunk_size = Some(n);
        self
    }
    pub fn hanging(mut self) -> Self {
        self.hang = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct FakeCall {
    pub command: String,
    pub args: Vec<String>,
    pub options: SpawnOptions,
    pub stdin: String,
    pub signals: Vec<KillSignal>,
}

type Plan = dyn Fn(&FakeCall, usize) -> FakeProcessSpec + Send + Sync;

/// A `Spawner` that replays canned output; for adapter tests and simulation.
#[derive(Clone)]
pub struct FakeSpawner {
    calls: Arc<Mutex<Vec<FakeCall>>>,
    plan: Arc<Plan>,
}

impl FakeSpawner {
    pub fn new(spec: FakeProcessSpec) -> Self {
        Self::with_plan(move |_, _| spec.clone())
    }

    pub fn with_plan(
        plan: impl Fn(&FakeCall, usize) -> FakeProcessSpec + Send + Sync + 'static,
    ) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            plan: Arc::new(plan),
        }
    }

    /// Snapshot of the calls so far (including stdin written and signals received).
    pub fn calls(&self) -> Vec<FakeCall> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call(&self, index: usize) -> FakeCall {
        self.calls.lock().unwrap()[index].clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

/// The sending halves of a fake process: its output and its exit.
type Channels = (mpsc::UnboundedSender<Output>, oneshot::Sender<ProcessExit>);

struct FakeProcess {
    index: usize,
    calls: Arc<Mutex<Vec<FakeCall>>>,
    spec: FakeProcessSpec,
    output: Option<mpsc::UnboundedReceiver<Output>>,
    exit: Option<oneshot::Receiver<ProcessExit>>,
    play: Mutex<Option<Channels>>,
    /// Shared with the playback task so a kill can finish a hanging process.
    finish: Arc<Mutex<Option<Channels>>>,
}

impl Spawner for FakeSpawner {
    fn spawn(
        &self,
        command: &str,
        args: &[String],
        options: &SpawnOptions,
    ) -> Box<dyn ProcessHandle> {
        let call = FakeCall {
            command: command.to_string(),
            args: args.to_vec(),
            options: options.clone(),
            stdin: String::new(),
            signals: Vec::new(),
        };
        let (index, spec) = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(call.clone());
            let index = calls.len() - 1;
            (index, (self.plan)(&call, index))
        };
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (exit_tx, exit_rx) = oneshot::channel();
        Box::new(FakeProcess {
            index,
            calls: self.calls.clone(),
            spec,
            output: Some(out_rx),
            exit: Some(exit_rx),
            play: Mutex::new(Some((out_tx, exit_tx))),
            finish: Arc::new(Mutex::new(None)),
        })
    }
}

impl FakeProcess {
    fn finish_with(
        finish: &Mutex<Option<(mpsc::UnboundedSender<Output>, oneshot::Sender<ProcessExit>)>>,
        exit: ProcessExit,
    ) {
        if let Some((out, exit_tx)) = finish.lock().unwrap().take() {
            drop(out);
            let _ = exit_tx.send(exit);
        }
    }
}

impl ProcessHandle for FakeProcess {
    fn take_output(&mut self) -> mpsc::UnboundedReceiver<Output> {
        self.output.take().expect("output already taken")
    }

    fn take_exit(&mut self) -> oneshot::Receiver<ProcessExit> {
        self.exit.take().expect("exit already taken")
    }

    fn write_stdin(&self, data: &str) {
        self.calls.lock().unwrap()[self.index].stdin.push_str(data);
    }

    /// Closing stdin starts the playback, like a real CLI that only answers once it has read its prompt.
    fn end_stdin(&self) {
        let Some((out_tx, exit_tx)) = self.play.lock().unwrap().take() else {
            return;
        };
        if let Some(message) = &self.spec.spawn_error {
            let _ = exit_tx.send(ProcessExit::spawn_failed(message.clone()));
            return;
        }
        let spec = self.spec.clone();
        let text = &spec.stdout;
        let pieces: Vec<String> = match spec.chunk_size {
            Some(n) if n > 0 => {
                let chars: Vec<char> = text.chars().collect();
                chars.chunks(n).map(|c| c.iter().collect()).collect()
            }
            _ => text.split_inclusive('\n').map(str::to_string).collect(),
        };
        *self.finish.lock().unwrap() = Some((out_tx, exit_tx));
        let finish = self.finish.clone();
        tokio::spawn(async move {
            // setImmediate: let the caller finish wiring up (and possibly abort) first
            tokio::task::yield_now().await;
            {
                let guard = finish.lock().unwrap();
                if let Some((out, _)) = guard.as_ref() {
                    for piece in pieces {
                        let _ = out.send(Output::Stdout(piece));
                    }
                    if !spec.stderr.is_empty() {
                        let _ = out.send(Output::Stderr(spec.stderr.clone()));
                    }
                }
            }
            if !spec.hang {
                FakeProcess::finish_with(&finish, ProcessExit::code(spec.exit_code));
            }
        });
    }

    fn kill(&self, signal: KillSignal) {
        self.calls.lock().unwrap()[self.index].signals.push(signal);
        if self.spec.hang {
            let exit = ProcessExit {
                signal: Some(signal.name().to_string()),
                ..ProcessExit::default()
            };
            // Before playback started the senders are still in `play`; either way the process ends now.
            if let Some((out, exit_tx)) = self.play.lock().unwrap().take() {
                drop(out);
                let _ = exit_tx.send(exit);
            } else {
                FakeProcess::finish_with(&self.finish, exit);
            }
        }
    }
}

/// The MCP server `server` as each harness hands it to its CLI, read back: what Claude Code's `--mcp-config`, Codex's `-c`
/// overrides and overlay `config.toml`, and Grok's `config.toml` and ACP `session/new` actually start. A test launches these to
/// prove a CLI would get a working Ratchet server.
pub fn rendered_mcp_servers(
    server: &ratchet_core::ports::McpServer,
) -> Vec<(&'static str, ratchet_core::ports::McpServer)> {
    use ratchet_core::ports::McpServer;
    use serde_json::Value;
    use std::collections::BTreeMap;

    let name = server.name.clone();
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::new();

    // Claude Code: --mcp-config JSON
    let claude = crate::claude_code::claude_mcp_config(std::slice::from_ref(server));
    let c = &claude["mcpServers"][&name];
    out.push((
        "claude-code",
        McpServer {
            name: name.clone(),
            command: c["command"].as_str().unwrap_or_default().to_owned(),
            args: strings(&c["args"]),
            env: c["env"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                        .collect()
                })
                .unwrap_or_default(),
        },
    ));

    // Codex: `-c mcp_servers.<name>.command="..."` and friends (TOML values written as JSON-compatible text)
    let mut codex = McpServer {
        name: name.clone(),
        ..McpServer::default()
    };
    let prefix = format!("mcp_servers.{name}.");
    for o in crate::codex::codex_mcp_overrides(std::slice::from_ref(server)) {
        let Some((key, value)) = o.split_once('=') else {
            continue;
        };
        let Some(key) = key.strip_prefix(&prefix) else {
            continue;
        };
        let value: Value = serde_json::from_str(value).unwrap_or(Value::Null);
        match key {
            "command" => codex.command = value.as_str().unwrap_or_default().to_owned(),
            "args" => codex.args = strings(&value),
            k => {
                if let Some(env) = k.strip_prefix("env.") {
                    codex.env.insert(
                        env.to_owned(),
                        value.as_str().unwrap_or_default().to_owned(),
                    );
                }
            }
        }
    }
    out.push(("codex -c", codex));

    // the TOML tables of a Codex overlay and a Grok per-session config
    let toml_server = |text: &str| -> McpServer {
        let mut s = McpServer {
            name: name.clone(),
            ..McpServer::default()
        };
        let mut in_env = false;
        let mut in_server = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_server = line == format!("[mcp_servers.{name}]")
                    || line == format!("[mcp_servers.{name}.env]");
                in_env = line == format!("[mcp_servers.{name}.env]");
                continue;
            }
            if !in_server {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (
                k.trim(),
                serde_json::from_str::<Value>(v.trim()).unwrap_or(Value::Null),
            );
            if in_env {
                s.env
                    .insert(k.to_owned(), v.as_str().unwrap_or_default().to_owned());
            } else if k == "command" {
                s.command = v.as_str().unwrap_or_default().to_owned();
            } else if k == "args" {
                s.args = strings(&v);
            }
        }
        s
    };
    out.push((
        "codex config.toml",
        toml_server(&crate::codex::codex_mcp_toml(std::slice::from_ref(server))),
    ));
    out.push((
        "grok config.toml",
        toml_server(&crate::grok::grok_config_toml(std::slice::from_ref(server))),
    ));

    // Grok ACP: session/new { mcpServers }
    let acp = crate::grok_acp::acp_mcp_servers(std::slice::from_ref(server));
    let a = &acp[0];
    let env: BTreeMap<String, String> = a["env"]
        .as_array()
        .map(|e| {
            e.iter()
                .map(|p| {
                    (
                        p["name"].as_str().unwrap_or_default().to_owned(),
                        p["value"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.push((
        "grok acp",
        McpServer {
            name: name.clone(),
            command: a["command"].as_str().unwrap_or_default().to_owned(),
            args: strings(&a["args"]),
            env,
        },
    ));
    out
}
