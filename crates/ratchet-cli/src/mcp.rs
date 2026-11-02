//! `ratchet mcp`: the owner mode of Ratchet's MCP server.
//!
//! One tool per CLI command, generated from [`crate::table::command_table`] (the clap definition), so the tools are exactly the
//! commands. A call is turned back into an argument list and run by the same code as `ratchet <command> --json`, which talks to
//! the daemon with the token in `ratchetd.token`. Register it with one line: `claude mcp add ratchet -- ratchet mcp`.
//!
//! Destructive tools (`run`, `stop`, `pr`, `env_end`) change nothing until the call carries `confirm: true`; without it they
//! answer with what they would do. That mirrors `--yes`.

use std::sync::Arc;

use async_trait::async_trait;
use ratchet_mcp::{McpServer, Refusal, ServerInfo, Tool, ToolOutput, ToolProvider};
use serde_json::{Map, Value, json};

use crate::cli::Cli;
use crate::io::BufferIo;
use crate::table::{ArgKind, ArgSpec, CommandSpec, Mark, command_table};

/// Events and `run --wait` would otherwise hold a tool call open forever.
const EVENTS_DEFAULT_COUNT: &str = "20";
const EVENTS_DEFAULT_TIMEOUT: &str = "10s";

/// Runs one `ratchet` command line and returns `(exit code, stdout, stderr)`.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, argv: Vec<String>) -> (i32, String, String);
}

/// The real thing: [`crate::run`] with captured output.
pub struct InProcess;

#[async_trait]
impl CommandRunner for InProcess {
    async fn run(&self, argv: Vec<String>) -> (i32, String, String) {
        let io = BufferIo::new();
        let code = crate::run(argv, &io).await;
        (code, io.stdout(), io.stderr())
    }
}

pub struct OwnerTools {
    table: Vec<CommandSpec>,
    /// `--socket` / `--token-file` / `--token` given to `ratchet mcp`, passed on to every command.
    globals: Vec<String>,
    runner: Arc<dyn CommandRunner>,
}

impl OwnerTools {
    pub fn new(globals: Vec<String>, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            table: command_table(),
            globals,
            runner,
        }
    }

    fn spec(&self, tool: &str) -> Option<&CommandSpec> {
        self.table.iter().find(|c| c.tool_name() == tool)
    }
}

/// The tool for one command.
pub fn tool_of(c: &CommandSpec) -> Tool {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for a in &c.args {
        properties.insert(a.name.clone(), schema_of(a));
        if a.required {
            required.push(json!(a.name));
        }
    }
    if c.mark == Mark::Destructive {
        properties.insert(
            "confirm".into(),
            json!({
                "type": "boolean",
                "description": "Must be true to run this command. Without it the tool only says what it would do (like --yes on the command line)."
            }),
        );
    }
    let mut description = c.summary.clone();
    description.push_str(&format!(" (ratchet {})", c.key()));
    if c.mark == Mark::Destructive {
        description.push_str(" Needs confirm: true.");
    }
    Tool {
        name: c.tool_name(),
        description,
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        }),
        read_only: c.mark == Mark::ReadOnly,
        destructive: c.mark == Mark::Destructive,
        open_world: false,
        validate: None,
    }
}

fn schema_of(a: &ArgSpec) -> Value {
    let base = match a.kind {
        ArgKind::String if !a.choices.is_empty() => json!({"type": "string", "enum": a.choices}),
        ArgKind::String => json!({"type": "string"}),
        ArgKind::Integer => json!({"type": "integer"}),
        ArgKind::Number => json!({"type": "number"}),
        ArgKind::Boolean => json!({"type": "boolean"}),
    };
    let mut v = if a.repeated {
        json!({"type": "array", "items": base})
    } else {
        base
    };
    if !a.description.is_empty() {
        v["description"] = json!(a.description);
    }
    v
}

/// Arguments of a tool call to a `ratchet` argument list (after `ratchet --json <globals>`). Pure, so it is tested without a
/// daemon.
pub fn argv_for(c: &CommandSpec, arguments: &Map<String, Value>) -> Result<Vec<String>, String> {
    for k in arguments.keys() {
        if k != "confirm" && !c.args.iter().any(|a| &a.name == k) {
            return Err(format!(
                "unknown argument \"{k}\"; {} takes: {}",
                c.tool_name(),
                c.args
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let mut argv: Vec<String> = c.path.clone();
    let mut positionals: Vec<String> = Vec::new();
    for a in &c.args {
        let Some(v) = arguments.get(&a.name).filter(|v| !v.is_null()) else {
            if a.required {
                return Err(format!("missing \"{}\" ({})", a.name, a.description));
            }
            continue;
        };
        let values: Vec<&Value> = match v {
            Value::Array(items) if a.repeated => items.iter().collect(),
            other => vec![other],
        };
        for v in values {
            let text = match (a.kind, v) {
                (ArgKind::Boolean, Value::Bool(true)) => None,
                (ArgKind::Boolean, Value::Bool(false)) => continue,
                (ArgKind::String, Value::String(s)) => Some(s.clone()),
                (ArgKind::Integer, Value::Number(n)) if n.is_i64() || n.is_u64() => {
                    Some(n.to_string())
                }
                (ArgKind::Number, Value::Number(n)) => Some(n.to_string()),
                _ => {
                    return Err(format!(
                        "\"{}\" has the wrong type (expected {:?})",
                        a.name, a.kind
                    ));
                }
            };
            match (&a.flag, text) {
                (Some(flag), None) => argv.push(format!("--{flag}")),
                (Some(flag), Some(t)) => argv.push(format!("--{flag}={t}")),
                (None, Some(t)) => positionals.push(t),
                (None, None) => return Err(format!("\"{}\" must be text", a.name)),
            }
        }
    }
    if c.path == ["events"] {
        if !arguments.contains_key("count") {
            argv.push(format!("--count={EVENTS_DEFAULT_COUNT}"));
        }
        if !arguments.contains_key("timeout") {
            argv.push(format!("--timeout={EVENTS_DEFAULT_TIMEOUT}"));
        }
    }
    if !positionals.is_empty() {
        argv.push("--".into());
        argv.extend(positionals);
    }
    Ok(argv)
}

#[async_trait]
impl ToolProvider for OwnerTools {
    fn tools(&self) -> Vec<Tool> {
        self.table.iter().map(tool_of).collect()
    }

    async fn call(&self, name: &str, arguments: Value) -> Result<ToolOutput, Refusal> {
        let Some(spec) = self.spec(name) else {
            return Err(Refusal(format!("unknown tool \"{name}\"")));
        };
        let Value::Object(arguments) = arguments else {
            return Ok(ToolOutput::error("arguments must be an object"));
        };
        let args = match argv_for(spec, &arguments) {
            Ok(a) => a,
            Err(e) => return Ok(ToolOutput::error(e)),
        };
        let mut argv = vec!["ratchet".to_owned(), "--json".to_owned()];
        argv.extend(self.globals.iter().cloned());
        argv.extend(args.iter().cloned());
        if spec.mark == Mark::Destructive {
            if arguments.get("confirm") != Some(&Value::Bool(true)) {
                let mut out = ToolOutput::json(json!({
                    "confirmed": false,
                    "changed": false,
                    "wouldRun": format!("ratchet {}", args.join(" ")),
                    "hint": "Nothing was done. Call this tool again with confirm: true to run it.",
                }));
                out.is_error = false;
                return Ok(out);
            }
            if spec.has_yes {
                argv.push("--yes".into());
            }
        }
        let (code, stdout, stderr) = self.runner.run(argv).await;
        Ok(finish(code, &stdout, &stderr))
    }
}

fn finish(code: i32, stdout: &str, stderr: &str) -> ToolOutput {
    if code == 0 {
        return match serde_json::from_str::<Value>(stdout.trim()) {
            Ok(v) => ToolOutput::json(v),
            Err(_) => ToolOutput::text(stdout.trim_end()),
        };
    }
    let detail = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    ToolOutput::error(format!(
        "ratchet exited with code {code}: {}",
        detail.trim()
    ))
}

/// `ratchet mcp`: serve on stdio until the client closes it.
pub async fn serve(cli: &Cli) -> i32 {
    let mut globals = Vec::new();
    if let Some(s) = &cli.socket {
        globals.push(format!("--socket={}", s.display()));
    }
    if let Some(f) = &cli.token_file {
        globals.push(format!("--token-file={}", f.display()));
    }
    // commands run in this process, so the token never shows in a process list
    if let Some(t) = &cli.token {
        globals.push(format!("--token={t}"));
    }
    let provider = Arc::new(OwnerTools::new(globals, Arc::new(InProcess)));
    let server = McpServer::new(
        ServerInfo::ratchet(Some(
            "Every tool is one `ratchet` command. Destructive tools (run, stop, pr, env_end) preview until confirm is true.",
        )),
        provider,
    );
    match server.serve(tokio::io::stdin(), tokio::io::stdout()).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ratchet: the MCP stream failed: {e}");
            1
        }
    }
}
