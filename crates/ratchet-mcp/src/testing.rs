//! A tiny MCP client for tests: start a server command the way a harness CLI would and talk JSON-RPC to it.

use std::collections::BTreeMap;
use std::process::Stdio;

use ratchet_core::ports::McpServer;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub struct StdioClient {
    child: Child,
    stdin: ChildStdin,
    out: tokio::io::Lines<BufReader<ChildStdout>>,
    next: i64,
}

impl StdioClient {
    /// Start `server.command` with its args and env (on top of an empty environment plus `base_env`).
    pub async fn spawn(
        server: &McpServer,
        base_env: &BTreeMap<String, String>,
    ) -> std::io::Result<Self> {
        let mut child = Command::new(&server.command)
            .args(&server.args)
            .env_clear()
            .envs(base_env)
            .envs(&server.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let out = BufReader::new(child.stdout.take().expect("piped")).lines();
        Ok(Self {
            child,
            stdin,
            out,
            next: 1,
        })
    }

    /// One request and its reply (the server answers in order).
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
            .to_string()
            + "\n";
        self.stdin.write_all(line.as_bytes()).await.expect("write");
        let reply = self
            .out
            .next_line()
            .await
            .expect("read")
            .expect("the server closed its output");
        let v: Value =
            serde_json::from_str(&reply).unwrap_or_else(|e| panic!("not JSON ({e}): {reply:?}"));
        assert_eq!(v["id"], id, "{v}");
        v
    }

    /// `initialize` + `notifications/initialized`.
    pub async fn initialize(&mut self) -> Value {
        let r = self
            .request(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
            )
            .await;
        self.stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .expect("write");
        r
    }

    pub async fn tool_names(&mut self) -> Vec<String> {
        let r = self.request("tools/list", json!({})).await;
        r["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("no tool list: {r}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    pub async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await
    }

    /// Close stdin and wait for the server to end.
    pub async fn finish(mut self) -> bool {
        drop(self.stdin);
        self.child
            .wait()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }
}
