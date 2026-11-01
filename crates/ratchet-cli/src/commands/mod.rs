//! The commands. `validate`, `init`, `baseline` and `templates` work offline on core alone; the rest are thin
//! clients of the daemon's socket API (see [`api`]).

pub mod api;
pub mod baseline;
pub mod init;
pub mod validate;

use serde::Serialize;

use crate::cli::{Cli, Command};
use crate::client::{Client, ClientError, Connection};
use crate::io::Io;

/// Print `value` as pretty JSON on stdout.
pub fn emit_json<T: Serialize + ?Sized>(io: &dyn Io, value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => io.out(&(s + "\n")),
        Err(e) => io.err(&format!("ratchet: cannot encode the output: {e}\n")),
    }
}

/// Report a failure: `ratchet: message` on stderr, or with `--json` an `{"error": ...}` object on stderr.
pub fn report_error(io: &dyn Io, json: bool, e: &ClientError) -> i32 {
    if json {
        let body = match e {
            ClientError::Api { error, .. } => serde_json::json!({"error": error}),
            other => serde_json::json!({"error": {
                "code": match other {
                    ClientError::Connect { .. } => "unreachable",
                    ClientError::NoToken(_) => "no_token",
                    _ => "protocol",
                },
                "message": other.to_string(),
                "fields": [],
            }}),
        };
        io.err(&(serde_json::to_string(&body).unwrap_or_default() + "\n"));
    } else {
        io.err(&format!("ratchet: {e}\n"));
        if let ClientError::Api { error, .. } = e {
            for f in &error.fields {
                io.err(&format!("  - {}: {}\n", f.path, f.message));
            }
        }
    }
    1
}

/// A fixed-width text table.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i + 1 == cols {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<w$}  ", w = widths[i]));
            }
        }
        s.trim_end().to_owned() + "\n"
    };
    let mut out = line(headers.to_vec());
    for r in rows {
        out.push_str(&line(r.iter().map(String::as_str).collect()));
    }
    out
}

/// Percent-encode a query value.
pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Run a parsed command line. Returns the process exit code.
pub async fn dispatch(cli: Cli, io: &dyn Io) -> i32 {
    let json = cli.json;
    match &cli.command {
        Command::Validate { path } => return validate::run_validate(path, json, io),
        Command::Init(args) => return init::run_init(args, json, io),
        Command::Baseline(args) => return baseline::run_baseline(args, json, io).await,
        Command::Templates => return init::list_all(json, io),
        _ => {}
    }
    let conn = Connection {
        socket: cli.socket.clone(),
        token_file: cli.token_file.clone(),
        token: cli.token.clone(),
    };
    let client = match Client::resolve(&conn) {
        Ok(c) => c,
        Err(e) => return report_error(io, json, &e),
    };
    match api::run(&client, cli.command, json, io).await {
        Ok(code) => code,
        Err(e) => report_error(io, json, &e),
    }
}
