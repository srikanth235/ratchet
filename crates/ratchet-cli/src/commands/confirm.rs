//! `--yes` for the commands that spend money or change things: `run`, `stop`, `env end` and `pr`.
//!
//! Without `--yes` they do nothing. They print what they would do (repo, agents, budget, where) and exit 2, so a
//! script or an agent that guessed a command line sees the effect before it happens. The preview is built from the
//! command line alone, so it works with the daemon down.

use clap::ValueEnum;
use serde_json::{Value, json};

use crate::cli::{AgentArg, Command, EnvCmd, RunArgs, WhereArg};
use crate::io::Io;

/// Exit code of a refused command: the command line is missing something, as with any usage error.
pub const EXIT_NEEDS_YES: i32 = 2;

/// What a command would do, as label/value lines plus the machine form.
pub struct Preview {
    pub command: &'static str,
    pub headline: String,
    pub lines: Vec<(&'static str, String)>,
    pub rerun: String,
}

/// The preview of `command`, or `None` when it needs no confirmation (or is too malformed to preview: the command
/// itself then reports the usage error).
pub fn preview(command: &Command) -> Option<Preview> {
    match command {
        Command::Run(a) => run_preview(a),
        Command::Stop { id, reason } => {
            let mut lines = vec![("optimization", id.clone())];
            if let Some(r) = reason {
                lines.push(("reason", r.clone()));
            }
            Some(Preview {
                command: "stop",
                headline: format!("Would stop {id}. Wins found so far are kept."),
                lines,
                rerun: format!("ratchet stop {id} --yes"),
            })
        }
        Command::Pr { id } => Some(Preview {
            command: "pr",
            headline: format!("Would open the pull request with the verified wins of {id}."),
            lines: vec![("optimization", id.clone())],
            rerun: format!("ratchet pr {id} --yes"),
        }),
        Command::Env {
            action: EnvCmd::End { sandbox },
        } => Some(Preview {
            command: "env end",
            headline: format!("Would tear down the environment {sandbox} now."),
            lines: vec![("environment", sandbox.clone())],
            rerun: format!("ratchet env end {sandbox} --yes"),
        }),
        _ => None,
    }
}

fn run_preview(a: &RunArgs) -> Option<Preview> {
    let target = match (&a.repo, &a.demo) {
        (Some(repo), _) => repo.clone(),
        (None, Some(d)) => format!(
            "{} (simulation)",
            d.to_possible_value()
                .map_or_else(|| "demo".to_owned(), |v| v.get_name().to_owned())
        ),
        (None, None) => return None,
    };
    let agents: Vec<&str> = a.agents.iter().map(|g| agent_name(*g)).collect();
    let place = match a.agents_where {
        Some(WhereArg::Local) => "local",
        Some(WhereArg::Cloud) => "cloud",
        None => "default for this repo",
    };
    let lines = vec![
        ("repo", target.clone()),
        ("branch", a.base_ref.clone()),
        ("agents", agents.join(", ")),
        ("budget", format!("up to ${:.2}", a.budget)),
        ("where", place.to_owned()),
        ("time limit", format!("{} s", a.time_limit)),
        (
            "starts",
            if a.no_start {
                "no (create only)"
            } else {
                "yes"
            }
            .to_owned(),
        ),
    ];
    Some(Preview {
        command: "run",
        headline: format!("Would create an optimization on {target} and run it."),
        lines,
        rerun: "re-run the same command with --yes".to_owned(),
    })
}

fn agent_name(a: AgentArg) -> &'static str {
    match a {
        AgentArg::ClaudeCode => "claude-code",
        AgentArg::Codex => "codex",
        AgentArg::Grok => "grok",
        AgentArg::Mock => "mock",
    }
}

/// Print the preview and the way forward; nothing was changed. Returns the exit code.
pub fn refuse(io: &dyn Io, json: bool, p: &Preview) -> i32 {
    if json {
        let details: Value = p
            .lines
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Value::String(v.clone())))
            .collect::<serde_json::Map<_, _>>()
            .into();
        let body = json!({
            "ok": false,
            "error": {
                "code": "confirmation_required",
                "message": format!("{} Re-run with --yes to do it.", p.headline),
            },
            "command": p.command,
            "preview": details,
            "changed": false,
        });
        io.out(&(serde_json::to_string(&body).unwrap_or_default() + "\n"));
    } else {
        let mut text = format!("{}\n", p.headline);
        let width = p.lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (k, v) in &p.lines {
            text.push_str(&format!("  {k:<width$}  {v}\n"));
        }
        text.push_str(&format!(
            "Nothing was changed; re-run with --yes ({}).\n",
            p.rerun
        ));
        io.err(&text);
    }
    EXIT_NEEDS_YES
}
