//! The stdio server end to end over in-memory pipes: handshake, the two tool sets, refusals, token-free backends.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_mcp::{McpServer, ServerInfo, SessionTools, ToolBackend, ToolError, ToolSet};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, Value)>>,
    fail: bool,
}

#[async_trait]
impl ToolBackend for Recorder {
    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, ToolError> {
        self.calls.lock().unwrap().push((tool.into(), arguments));
        if self.fail {
            return Err(ToolError::new("no such optimization"));
        }
        Ok(json!({"ok": true}))
    }
}

struct Client {
    to: DuplexStream,
    from: BufReader<DuplexStream>,
    next: i64,
}

impl Client {
    fn start(set: ToolSet, backend: Arc<Recorder>) -> Self {
        let (to, server_in) = tokio::io::duplex(1 << 16);
        let (server_out, from) = tokio::io::duplex(1 << 16);
        let server = McpServer::new(
            ServerInfo::ratchet(None),
            Arc::new(SessionTools::new(set, backend)),
        );
        tokio::spawn(async move { server.serve(server_in, server_out).await });
        Self {
            to,
            from: BufReader::new(from),
            next: 1,
        }
    }

    async fn raw(&mut self, line: &str) {
        self.to
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn read(&mut self) -> Value {
        let mut line = String::new();
        self.from.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line:?}"))
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.raw(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
        )
        .await;
        let reply = self.read().await;
        assert_eq!(reply["id"], id);
        reply
    }

    async fn initialize(&mut self, version: &str) -> Value {
        let r = self
            .request(
                "initialize",
                json!({"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
            )
            .await;
        self.raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        r
    }

    async fn tool_names(&mut self) -> Vec<String> {
        let r = self.request("tools/list", json!({})).await;
        r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    }
}

#[tokio::test]
async fn initialize_negotiates_the_protocol_version() {
    for (asked, answered) in [
        ("2025-06-18", "2025-06-18"),
        ("2025-03-26", "2025-03-26"),
        ("2024-11-05", "2024-11-05"),
        ("2030-01-01", "2025-06-18"),
    ] {
        let mut c = Client::start(ToolSet::Chat, Arc::default());
        let r = c.initialize(asked).await;
        assert_eq!(r["result"]["protocolVersion"], answered, "asked {asked}");
        assert_eq!(r["result"]["serverInfo"]["name"], "ratchet");
        assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], false);
    }
}

#[tokio::test]
async fn requests_before_initialize_are_refused_but_ping_works() {
    let mut c = Client::start(ToolSet::Chat, Arc::default());
    let r = c.request("tools/list", json!({})).await;
    assert_eq!(r["error"]["code"], -32002);
    assert_eq!(c.request("ping", json!({})).await["result"], json!({}));
}

#[tokio::test]
async fn each_session_lists_exactly_its_own_tool_set() {
    let mut chat = Client::start(ToolSet::Chat, Arc::default());
    chat.initialize("2025-06-18").await;
    assert_eq!(chat.tool_names().await, ToolSet::Chat.names());

    let mut opt = Client::start(ToolSet::Optimization, Arc::default());
    opt.initialize("2025-06-18").await;
    assert_eq!(
        opt.tool_names().await,
        [
            "steering_notes",
            "history",
            "connector_query",
            "submit_candidate"
        ]
    );
}

#[tokio::test]
async fn listed_tools_carry_schemas_and_safety_annotations() {
    let mut c = Client::start(ToolSet::Chat, Arc::default());
    c.initialize("2025-06-18").await;
    let r = c.request("tools/list", json!({})).await;
    let tools = r["result"]["tools"].as_array().unwrap();
    let find = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap();
    assert_eq!(
        find("list_optimizations")["annotations"]["readOnlyHint"],
        true
    );
    assert_eq!(
        find("create_optimization")["annotations"]["readOnlyHint"],
        false
    );
    assert_eq!(find("draft_optimization")["inputSchema"]["type"], "object");
    assert_eq!(
        find("draft_optimization")["inputSchema"]["required"],
        json!(["repo", "goal"])
    );
}

#[tokio::test]
async fn a_call_in_the_set_reaches_the_backend() {
    let backend = Arc::new(Recorder::default());
    let mut c = Client::start(ToolSet::Chat, backend.clone());
    c.initialize("2025-06-18").await;
    let r = c
        .request(
            "tools/call",
            json!({"name": "pause", "arguments": {"optimization": "o1"}}),
        )
        .await;
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(r["result"]["structuredContent"], json!({"ok": true}));
    assert_eq!(
        backend.calls.lock().unwrap().as_slice(),
        [("pause".to_owned(), json!({"optimization": "o1"}))]
    );
}

#[tokio::test]
async fn a_call_outside_the_set_is_refused_and_never_reaches_the_backend() {
    let backend = Arc::new(Recorder::default());
    let mut opt = Client::start(ToolSet::Optimization, backend.clone());
    opt.initialize("2025-06-18").await;
    for tool in [
        "create_optimization",
        "draft_optimization",
        "run_now",
        "add_steering_note",
        "list_repos",
    ] {
        let r = opt
            .request("tools/call", json!({"name": tool, "arguments": {}}))
            .await;
        assert_eq!(r["error"]["code"], -32602, "{tool}");
        assert!(
            r["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not available in this session"),
            "{tool}: {r}"
        );
    }
    let mut chat = Client::start(ToolSet::Chat, backend.clone());
    chat.initialize("2025-06-18").await;
    for tool in [
        "steering_notes",
        "history",
        "connector_query",
        "submit_candidate",
        "no_such_tool",
    ] {
        let r = chat
            .request("tools/call", json!({"name": tool, "arguments": {}}))
            .await;
        assert_eq!(r["error"]["code"], -32602, "{tool}");
    }
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn bad_arguments_and_backend_failures_are_tool_errors_the_model_can_read() {
    let backend = Arc::new(Recorder {
        fail: true,
        ..Recorder::default()
    });
    let mut c = Client::start(ToolSet::Chat, backend.clone());
    c.initialize("2025-06-18").await;
    let bad = c
        .request(
            "tools/call",
            json!({"name": "add_steering_note", "arguments": {"optimization": "o"}}),
        )
        .await;
    assert_eq!(bad["result"]["isError"], true);
    assert!(
        bad["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("bad arguments")
    );
    assert!(
        backend.calls.lock().unwrap().is_empty(),
        "invalid arguments never get forwarded"
    );
    let failed = c
        .request(
            "tools/call",
            json!({"name": "get_optimization", "arguments": {"optimization": "x"}}),
        )
        .await;
    assert_eq!(failed["result"]["isError"], true);
    assert_eq!(
        failed["result"]["content"][0]["text"],
        "no such optimization"
    );
}

#[tokio::test]
async fn junk_unknown_methods_and_notifications() {
    let mut c = Client::start(ToolSet::Chat, Arc::default());
    c.raw("not json").await;
    assert_eq!(c.read().await["error"]["code"], -32700);
    c.initialize("2025-06-18").await;
    assert_eq!(
        c.request("resources/list", json!({})).await["error"]["code"],
        -32601
    );
    // a notification gets no reply: the next reply is for the next request
    c.raw(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#)
        .await;
    assert_eq!(c.request("ping", json!({})).await["result"], json!({}));
}
