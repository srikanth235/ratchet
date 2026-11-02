//! The Ratchet MCP server as a harness CLI meets it: attached to a session spec, rendered by each adapter into its own flags
//! or config, started from that rendering, and listing exactly the optimization tool set (the cloud runner serves it).

use std::collections::BTreeMap;
use std::path::Path;

use ratchet_agents::testing::rendered_mcp_servers;
use ratchet_agents::{
    ClaudeCodeAdapter, ClaudeCodeOptions, CodexAdapter, CodexOptions, runner_prefix,
};
use ratchet_core::ports::{Access, SessionSpec};
use ratchet_mcp::ToolSet;
use ratchet_mcp::launch::{SERVER_NAME, attach, runner_server};
use ratchet_mcp::testing::StdioClient;
use serde_json::{Value, json};

fn runner_env(dir: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("RATCHET_OPTIMIZATION_ID".to_owned(), "opt1".to_owned()),
        ("RATCHET_AGENT_ID".to_owned(), "agent1".to_owned()),
        ("RATCHET_ARTIFACT_DIR".to_owned(), dir.display().to_string()),
    ])
}

fn runner(dir: &Path) -> ratchet_core::ports::McpServer {
    runner_server(env!("CARGO_BIN_EXE_ratchet-runner"), runner_env(dir))
}

fn session_spec(dir: &Path) -> SessionSpec {
    let mut spec = SessionSpec {
        workdir: "/work".into(),
        prompt: "go".into(),
        access: Access::ReadOnly,
        ..SessionSpec::default()
    };
    attach(&mut spec, runner(dir));
    spec
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

#[test]
fn every_adapter_receives_the_ratchet_server_for_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let spec = session_spec(dir.path());
    assert_eq!(spec.mcp.len(), 1);
    assert_eq!(spec.mcp[0].name, SERVER_NAME);

    let claude = ClaudeCodeAdapter::new(ClaudeCodeOptions::default());
    let args = claude.build_session_args(&spec, None, Some(Path::new("/tmp/m.json")));
    assert_eq!(flag(&args, "--mcp-config"), Some("/tmp/m.json"));
    assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    assert!(
        flag(&args, "--allowedTools")
            .unwrap()
            .contains("mcp__ratchet")
    );

    let codex = CodexAdapter::new(CodexOptions::default());
    let args = codex.build_session_args(&spec, None);
    assert!(
        args.windows(2)
            .any(|w| w[0] == "-c" && w[1].starts_with("mcp_servers.ratchet.command=")),
        "{args:?}"
    );

    let config = ratchet_agents::grok::grok_config_toml(&spec.mcp);
    assert!(config.contains("[mcp_servers.ratchet]") && config.contains("mcps = false"));
}

#[tokio::test]
async fn each_harness_rendering_starts_a_server_that_lists_exactly_the_optimization_tools() {
    let dir = tempfile::tempdir().unwrap();
    let renderings = rendered_mcp_servers(&runner(dir.path()));
    assert_eq!(renderings.len(), 5);
    for (harness, server) in renderings {
        assert_eq!(
            server.command,
            env!("CARGO_BIN_EXE_ratchet-runner"),
            "{harness}"
        );
        let mut client = StdioClient::spawn(&server, &BTreeMap::new()).await.unwrap();
        let init = client.initialize().await;
        assert_eq!(init["result"]["serverInfo"]["name"], "ratchet", "{harness}");
        assert_eq!(
            client.tool_names().await,
            ToolSet::Optimization.names(),
            "{harness} lists exactly the optimization set"
        );
        // a chat tool is refused, not run
        for tool in [
            "create_optimization",
            "draft_optimization",
            "run_now",
            "add_steering_note",
        ] {
            let r = client.call(tool, json!({})).await;
            assert_eq!(r["error"]["code"], -32602, "{harness} {tool}: {r}");
        }
        assert!(
            client.finish().await,
            "{harness}: the server ends when the CLI closes stdin"
        );
    }
}

fn text(r: &Value) -> Value {
    serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn the_runner_serves_steering_notes_history_and_candidates_from_its_inbox_and_store() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = runner_prefix("opt1", "agent1");
    let put = |key: String, body: String| {
        let path = dir.path().join(key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    };
    put(
        format!("{prefix}inbox/00000001.json"),
        json!({"type": "user", "text": "try the cache"}).to_string(),
    );
    put(
        format!("{prefix}inbox/00000002.json"),
        json!({"type": "pause"}).to_string(),
    );
    put(
        format!("{prefix}inbox/00000003.json"),
        json!({"type": "user", "text": "and skip the sort"}).to_string(),
    );
    put(
        format!("{prefix}events/00000001.jsonl"),
        [
            json!({"at": 1, "type": "status", "text": "started"}),
            json!({"at": 2, "type": "verdict", "text": "accepted abc1234", "data": {"sha": "abc1234", "verdict": "accepted"}}),
            json!({"at": 3, "type": "verdict", "text": "rejected def5678", "data": {"sha": "def5678", "verdict": "rejected"}}),
        ]
        .map(|v| v.to_string())
        .join("\n")
            + "\n",
    );

    let mut client = StdioClient::spawn(&runner(dir.path()), &BTreeMap::new())
        .await
        .unwrap();
    client.initialize().await;

    // the notes since the last call: only user messages, once
    let first = text(&client.call("steering_notes", json!({})).await);
    let notes: Vec<&str> = first["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["text"].as_str().unwrap())
        .collect();
    assert_eq!(notes, ["try the cache", "and skip the sort"]);
    assert_eq!(
        text(&client.call("steering_notes", json!({})).await)["notes"],
        json!([])
    );
    put(
        format!("{prefix}inbox/00000004.json"),
        json!({"type": "user", "text": "one more"}).to_string(),
    );
    let later = text(&client.call("steering_notes", json!({})).await);
    assert_eq!(later["notes"][0]["text"], "one more");

    // history: the verdicts, newest last, limited
    let all = text(&client.call("history", json!({})).await);
    assert_eq!(all["verdicts"].as_array().unwrap().len(), 2);
    let one = text(&client.call("history", json!({"limit": 1})).await);
    assert_eq!(one["verdicts"][0]["data"]["sha"], "def5678");

    // submit_candidate: numbered objects in the store
    let r = client
        .call(
            "submit_candidate",
            json!({"sha": "abc1234", "note": "cache the parse"}),
        )
        .await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    client
        .call("submit_candidate", json!({"sha": "def5678abc"}))
        .await;
    let first =
        std::fs::read_to_string(dir.path().join(format!("{prefix}candidates/00000001.json")))
            .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&first).unwrap()["sha"],
        "abc1234"
    );
    assert!(
        dir.path()
            .join(format!("{prefix}candidates/00000002.json"))
            .exists()
    );
    let bad = client
        .call("submit_candidate", json!({"sha": "not a sha"}))
        .await;
    assert_eq!(bad["result"]["isError"], true);

    // connectors never reach a runner
    let q = client
        .call(
            "connector_query",
            json!({"connector": "datadog", "query": "p95"}),
        )
        .await;
    assert_eq!(q["result"]["isError"], true);
    assert!(
        q["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no connector")
    );
    assert!(client.finish().await);
}

#[tokio::test]
async fn the_runner_mcp_command_refuses_to_start_without_its_environment() {
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_ratchet-runner"))
        .arg("mcp")
        .env_clear()
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("RATCHET_OPTIMIZATION_ID"));
    assert!(out.stdout.is_empty(), "stdout carries JSON-RPC only");
}
