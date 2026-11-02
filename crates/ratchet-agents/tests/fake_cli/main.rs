//! A fake agent CLI for tests: replays a fixture on stdout and, for the long-lived protocols, reads stdin and answers per turn.
//!
//! ```text
//! ratchet-fake-cli replay             <fixture> [--exit-code N] [--stderr TEXT] [--delay-ms N]
//! ratchet-fake-cli claude-stream-json <fixture>   # `claude -p --input-format stream-json --output-format stream-json`
//! ratchet-fake-cli codex-app-server   <fixture>   # `codex app-server` (JSONL JSON-RPC, no "jsonrpc" key)
//! ratchet-fake-cli grok-acp           <fixture>   # `grok agent stdio` (ACP, JSON-RPC 2.0)
//! ```
//!
//! `replay` prints the whole fixture, then exits (one-shot CLIs: `claude -p`, `codex exec --json`, `grok -p`).
//!
//! The long-lived modes print nothing until stdin delivers a message, then print the next *batch* of fixture lines:
//!
//! * `claude-stream-json`: each stdin line that is a JSON object with `"type":"user"` releases lines up to and including the next
//!   `"type":"result"` line. The `system/init` line is released with the first turn, as the real CLI does.
//! * `codex-app-server`: each request (a line with `id` and `method`) releases the next line containing the placeholder `"$ID"`
//!   (the response) plus the notification lines that follow it, up to the next `"$ID"` line. `turn/start` therefore gets its
//!   response and the whole turn's notifications. Notifications from the client (`initialized`) release nothing.
//! * `grok-acp`: each request releases lines up to and including the next `"$ID"` line. A `session/prompt` therefore gets its
//!   `session/update` chunks first and its response last, as ACP does.
//!
//! `"$ID"` in a fixture is replaced by the raw JSON id of the request being answered. Stdin EOF ends the process with exit 0.
//!
//! A fixture line `{"$hold":true}` cuts a batch short: the lines before it are printed, the turn stays open, and the lines after
//! it wait for the client to interrupt (`control_request` for Claude, `turn/interrupt` for Codex, `session/cancel` for Grok).
//! The interrupt releases the next batch as usual; for ACP, `$ID` then stands for the id of the held `session/prompt`.
//! Everything read from stdin is appended to the file named by `RATCHET_FAKE_CLI_LOG`, when set, for assertions.

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(mode), Some(fixture)) = (args.first(), args.get(1)) else {
        eprintln!(
            "usage: ratchet-fake-cli <replay|claude-stream-json|codex-app-server|grok-acp> <fixture> [options]"
        );
        std::process::exit(2);
    };
    let text = match std::fs::read_to_string(fixture) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read fixture {fixture}: {e}");
            std::process::exit(2);
        }
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    match mode.as_str() {
        "replay" => replay(&lines, &args[2..]),
        "claude-stream-json" | "codex-app-server" | "grok-acp" => serve(mode, &lines),
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn replay(lines: &[&str], args: &[String]) {
    let delay = opt(args, "--delay-ms")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let mut out = std::io::stdout().lock();
    for line in lines {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
    }
    if let Some(stderr) = opt(args, "--stderr") {
        eprintln!("{stderr}");
    }
    let code = opt(args, "--exit-code")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    std::process::exit(code);
}

fn is_hold(line: &str) -> bool {
    line.contains("\"$hold\"")
}

fn is_response(line: &str) -> bool {
    line.contains("\"$ID\"")
}

fn is_result(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .is_some_and(|v| v.get("type").and_then(|t| t.as_str()) == Some("result"))
}

fn serve(mode: &str, lines: &[&str]) {
    let log_path = std::env::var("RATCHET_FAKE_CLI_LOG").ok();
    let mut log = log_path.and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    let mut cursor = 0usize;
    // raw id of the request whose batch was cut at a hold
    let mut held: Option<String> = None;
    let mut out = std::io::stdout().lock();
    for input in std::io::stdin().lock().lines() {
        let Ok(input) = input else { break };
        if input.trim().is_empty() {
            continue;
        }
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{input}");
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&input) else {
            continue;
        };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let mut id = msg
            .get("id")
            .map_or_else(|| "null".to_string(), |v| v.to_string());
        let end = match mode {
            "claude-stream-json" => {
                let kind = msg.get("type").and_then(|t| t.as_str());
                let interrupt = kind == Some("control_request") && held.is_some();
                if kind != Some("user") && !interrupt {
                    continue;
                }
                lines[cursor..]
                    .iter()
                    .position(|l| is_result(l))
                    .map_or(lines.len(), |i| cursor + i + 1)
            }
            _ => {
                // JSON-RPC: requests (id + method) are answered; ACP's `session/cancel` is a notification that releases the
                // held prompt's response.
                let cancel = mode == "grok-acp" && method == "session/cancel" && held.is_some();
                if cancel {
                    id = held.clone().unwrap_or_default();
                } else if msg.get("id").is_none() || method.is_empty() {
                    continue;
                }
                let Some(first) = lines[cursor..].iter().position(|l| is_response(l)) else {
                    continue;
                };
                let first = cursor + first;
                if mode == "codex-app-server" {
                    lines[first + 1..]
                        .iter()
                        .position(|l| is_response(l))
                        .map_or(lines.len(), |i| first + 1 + i)
                } else {
                    first + 1
                }
            }
        };
        let hold = lines[cursor..end].iter().position(|l| is_hold(l));
        let (batch, next) = match hold {
            Some(h) => (&lines[cursor..cursor + h], cursor + h + 1),
            None => (&lines[cursor..end], end),
        };
        held = hold.map(|_| id.clone());
        cursor = next;
        for line in batch {
            let _ = writeln!(out, "{}", line.replace("\"$ID\"", &id));
        }
        let _ = out.flush();
    }
}
