//! Common plumbing for the CLI-backed adapters: spawn, feed lines to a parser, classify the outcome.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::clock::SystemClock;
use crate::domain::{AgentSpec, Clock};
use crate::process::{RunProcessOptions, RunProcessResult, SharedSpawner, TokioSpawner, run_process};

pub use crate::domain::{AgentError, AgentSession};

#[derive(Clone, Default)]
pub struct CliAgentOptions {
    /// Path or name of the CLI binary.
    pub binary: Option<String>,
    /// Base environment (default: the process environment); `AgentSpec::env` is layered on top.
    pub env: Option<HashMap<String, String>>,
    pub spawner: Option<SharedSpawner>,
    pub clock: Option<Arc<dyn Clock>>,
    /// Extra CLI args appended before the prompt.
    pub extra_args: Vec<String>,
    /// Grace between SIGTERM and SIGKILL on abort.
    pub kill_grace: Option<Duration>,
}

/// `text` cut to `max` characters with a note of how much was dropped.
pub fn truncate(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count > max {
        let head: String = text.chars().take(max).collect();
        format!("{head}… [+{} chars]", count - max)
    } else {
        text.to_string()
    }
}

pub fn as_record(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()
}

/// A finite JSON number, else 0 (token counts are whole numbers).
pub fn num(v: Option<&Value>) -> u64 {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n > 0.0 => n as u64,
        _ => 0,
    }
}

pub fn str_of<'a>(v: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The base environment with the spec's layered on top.
pub fn build_env(base: &HashMap<String, String>, spec: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut env = base.clone();
    if let Some(spec) = spec {
        env.extend(spec.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    env
}

pub fn process_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// The part of a CLI adapter that does not depend on the CLI: binary, environment, spawner, clock.
pub struct CliAgent {
    pub binary: String,
    pub base_env: HashMap<String, String>,
    pub spawner: SharedSpawner,
    pub clock: Arc<dyn Clock>,
    pub extra_args: Vec<String>,
    pub kill_grace: Option<Duration>,
}

impl CliAgent {
    pub fn new(default_binary: &str, options: CliAgentOptions) -> Self {
        Self {
            binary: options.binary.unwrap_or_else(|| default_binary.to_string()),
            base_env: options.env.unwrap_or_else(process_env),
            spawner: options.spawner.unwrap_or_else(|| Arc::new(TokioSpawner)),
            clock: options.clock.unwrap_or_else(|| Arc::new(SystemClock)),
            extra_args: options.extra_args,
            kill_grace: options.kill_grace,
        }
    }

    pub async fn run(
        &self,
        args: &[String],
        spec: &AgentSpec,
        stdin: Option<String>,
        on_line: &mut (dyn FnMut(&str) + Send),
        cancel: &CancellationToken,
    ) -> RunProcessResult {
        let mut options = RunProcessOptions::new(spec.workdir.clone(), build_env(&self.base_env, spec.env.as_ref()));
        options.stdin = stdin;
        options.cancel = Some(cancel);
        if let Some(grace) = self.kill_grace {
            options.kill_grace = grace;
        }
        options.on_stdout_line = Some(on_line);
        run_process(self.spawner.as_ref(), &self.binary, args, options).await
    }

    /// `<binary> --version` succeeds. Returns the problem, if any.
    pub async fn probe_binary(&self) -> Option<String> {
        let mut out = String::new();
        let mut collect = |line: &str| {
            out.push_str(line);
            out.push('\n');
        };
        let mut options = RunProcessOptions::new(std::env::current_dir().unwrap_or_else(|_| ".".into()), self.base_env.clone());
        options.kill_grace = Duration::from_millis(1000);
        options.on_stdout_line = Some(&mut collect);
        let res = run_process(self.spawner.as_ref(), &self.binary, &["--version".to_string()], options).await;
        if let Some(err) = res.spawn_error() {
            return Some(format!("cannot run '{}': {}", self.binary, err));
        }
        if res.code() != Some(0) {
            let code = res.code().map_or("null".to_string(), |c| c.to_string());
            let detail = if res.stderr_tail.is_empty() { out.trim().to_string() } else { res.stderr_tail.clone() };
            return Some(format!("'{} --version' exited with {code}: {detail}", self.binary));
        }
        None
    }
}
