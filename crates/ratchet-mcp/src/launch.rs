//! How a harness is told to start the Ratchet server: the [`McpServer`] that goes into `SessionSpec::mcp`, which every adapter
//! turns into its CLI's own flags (Claude `--mcp-config`, Codex `-c mcp_servers.*`, Grok `session/new` / `config.toml`).

use std::collections::BTreeMap;
use std::path::Path;

use ratchet_core::ports::{McpServer, SessionSpec};

/// The server's name in every CLI. Tools show up as `mcp__ratchet__<tool>` in Claude Code.
pub const SERVER_NAME: &str = "ratchet";
/// Where `ratchetd mcp` finds the session token when `--session` is not given (for a client that would rather keep it out of
/// the process list).
pub const SESSION_ENV: &str = "RATCHET_MCP_SESSION";

/// The local server: `ratchetd mcp`, talking to the daemon on `socket` with `token`.
pub fn ratchetd_server(ratchetd: &str, token: &str, socket: Option<&Path>) -> McpServer {
    let mut env = BTreeMap::new();
    if let Some(s) = socket {
        env.insert("RATCHET_SOCKET".into(), s.display().to_string());
    }
    McpServer {
        name: SERVER_NAME.into(),
        command: ratchetd.into(),
        args: vec!["mcp".into(), "--session".into(), token.into()],
        env,
    }
}

/// Hand `server` to a session: it replaces an earlier server of the same name, so every adapter gets exactly one Ratchet server
/// in `mcp` and turns it into its CLI's flags.
pub fn attach(spec: &mut SessionSpec, server: McpServer) {
    spec.mcp.retain(|s| s.name != server.name);
    spec.mcp.push(server);
}

/// The cloud server: `ratchet-runner mcp`, which reads the runner's own `RATCHET_*` environment (optimization, agent, artifact
/// store). `env` is what it needs from there.
pub fn runner_server(runner: &str, env: BTreeMap<String, String>) -> McpServer {
    McpServer {
        name: SERVER_NAME.into(),
        command: runner.into(),
        args: vec!["mcp".into()],
        env,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_server_is_ratchetd_mcp_with_the_session_token() {
        let s = ratchetd_server("/bin/ratchetd", "tok", Some(Path::new("/s.sock")));
        assert_eq!(s.args, ["mcp", "--session", "tok"]);
        assert_eq!(s.env["RATCHET_SOCKET"], "/s.sock");
    }

    #[test]
    fn attaching_replaces_an_earlier_ratchet_server() {
        let mut spec = SessionSpec::default();
        attach(&mut spec, ratchetd_server("/a", "one", None));
        attach(&mut spec, ratchetd_server("/a", "two", None));
        assert_eq!(spec.mcp.len(), 1);
        assert_eq!(spec.mcp[0].args[2], "two");
    }
}
