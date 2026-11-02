//! `ratchet mcp` (owner mode): the tool list is the CLI's command table, destructive tools need `confirm`, and a call runs the
//! same code as the command line.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clap::CommandFactory;
use ratchet_cli::cli::Cli;
use ratchet_cli::mcp::{CommandRunner, InProcess, OwnerTools, argv_for};
use ratchet_cli::table::{HIDDEN_ARGS, MARKS, command_table, mark_of};
use ratchet_mcp::{ToolOutput, ToolProvider};
use serde_json::{Map, Value, json};

use crate::common::Env;

/// Every leaf command of the clap definition, walked here independently of `command_table`.
fn leaves(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        let path = if prefix.is_empty() {
            sub.get_name().to_owned()
        } else {
            format!("{prefix} {}", sub.get_name())
        };
        if sub
            .get_subcommands()
            .filter(|s| s.get_name() != "help")
            .count()
            == 0
        {
            out.push(path);
        } else {
            leaves(sub, &path, out);
        }
    }
}

fn cli_commands() -> Vec<String> {
    let mut v = Vec::new();
    leaves(&Cli::command(), "", &mut v);
    v.retain(|c| c != "mcp");
    v
}

fn owner(runner: Arc<dyn CommandRunner>) -> OwnerTools {
    OwnerTools::new(vec![], runner)
}

#[test]
fn the_owner_tool_list_matches_the_cli_command_table() {
    let tools = owner(Arc::new(InProcess)).tools();
    let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

    let mut from_cli: Vec<String> = cli_commands().iter().map(|c| c.replace(' ', "_")).collect();
    let mut sorted_names = names.clone();
    from_cli.sort();
    sorted_names.sort();
    assert_eq!(
        sorted_names, from_cli,
        "one tool per CLI command, no more, no fewer"
    );

    // and the table itself is in the CLI's definition order
    let table: Vec<String> = command_table()
        .iter()
        .map(|c| c.key().replace(' ', "_"))
        .collect();
    assert_eq!(names, table);

    // the commands an agent expects are there
    for want in [
        "validate",
        "init",
        "baseline",
        "templates",
        "status",
        "list",
        "show",
        "run",
        "stop",
        "report",
        "pr",
        "candidates",
        "history",
        "spend",
        "activity",
        "agent_pause",
        "agent_resume",
        "agent_message",
        "agent_log",
        "events",
        "env_list",
        "env_end",
        "cost_reconcile",
        "repos",
        "inspect",
        "overview",
        "home",
        "value_show",
        "value_set",
        "value_clear",
        "schedule_show",
        "schedule_set",
        "schedule_clear",
        "pr-merged",
        "connectors",
    ] {
        assert!(
            names.iter().any(|n| n == want),
            "missing tool {want}: {names:?}"
        );
    }
    assert!(
        !names.iter().any(|n| n == "mcp"),
        "the server is not one of its own tools"
    );
}

#[test]
fn every_command_has_a_mark_and_every_mark_a_command() {
    let commands = cli_commands();
    for c in &commands {
        assert!(
            mark_of(c).is_some(),
            "`ratchet {c}` has no entry in ratchet_cli::table::MARKS: classify it as ReadOnly, Write or Destructive"
        );
    }
    for (key, _) in MARKS {
        assert!(
            commands.iter().any(|c| c == key),
            "MARKS lists `{key}`, which is not a command"
        );
    }
    for (cmd, arg) in HIDDEN_ARGS {
        let spec = command_table()
            .into_iter()
            .find(|c| c.key() == *cmd)
            .unwrap();
        assert!(
            !spec.args.iter().any(|a| a.name == *arg),
            "{cmd} --{arg} is hidden"
        );
    }
}

#[test]
fn marks_become_annotations_and_destructive_tools_ask_for_confirm() {
    let tools = owner(Arc::new(InProcess)).tools();
    let t = |n: &str| tools.iter().find(|t| t.name == n).unwrap();
    for n in ["run", "stop", "pr", "env_end"] {
        assert!(!t(n).read_only && t(n).destructive, "{n}");
        assert_eq!(
            t(n).input_schema["properties"]["confirm"]["type"],
            "boolean",
            "{n}"
        );
        assert!(t(n).description.contains("confirm"), "{n}");
    }
    for n in [
        "list",
        "show",
        "status",
        "report",
        "agent_log",
        "env_list",
        "validate",
    ] {
        assert!(t(n).read_only && !t(n).destructive, "{n}");
        assert!(
            t(n).input_schema["properties"].get("confirm").is_none(),
            "{n}"
        );
    }
    assert!(!t("agent_message").read_only && !t("agent_message").destructive);
    // schemas come from the clap arguments
    let run = &t("run").input_schema;
    assert_eq!(run["properties"]["budget"]["type"], "number");
    assert_eq!(run["properties"]["plateau"]["type"], "integer");
    assert_eq!(run["properties"]["no_start"]["type"], "boolean");
    assert_eq!(run["properties"]["agent"]["type"], "array");
    assert_eq!(
        run["properties"]["agent"]["items"]["enum"],
        json!(["claude-code", "codex", "grok", "mock"])
    );
    assert_eq!(run["additionalProperties"], false);
    assert_eq!(t("show").input_schema["required"], json!(["id"]));
    assert!(
        t("report").input_schema["properties"].get("out").is_none(),
        "--out writes a file; the tool returns text"
    );
}

#[test]
fn arguments_become_a_command_line() {
    let table = command_table();
    let spec = |k: &str| table.iter().find(|c| c.key() == k).unwrap();
    let args = |v: Value| -> Map<String, Value> { v.as_object().unwrap().clone() };

    assert_eq!(
        argv_for(spec("show"), &args(json!({"id": "-weird"}))).unwrap(),
        ["show", "--", "-weird"]
    );
    assert_eq!(
        argv_for(
            spec("run"),
            &args(json!({"repo": "acme/api", "budget": 12.5, "agent": ["codex", "grok"], "no_start": true, "name": "x y"}))
        )
        .unwrap(),
        ["run", "--name=x y", "--agent=codex", "--agent=grok", "--budget=12.5", "--no-start", "--", "acme/api"]
    );
    assert_eq!(
        argv_for(
            spec("agent log"),
            &args(json!({"id": "a1", "limit": 5, "after": -1}))
        )
        .unwrap(),
        ["agent", "log", "--limit=5", "--after=-1", "--", "a1"]
    );
    // false flags and nulls are left out
    assert_eq!(
        argv_for(spec("run"), &args(json!({"no_start": false, "name": null}))).unwrap(),
        ["run"]
    );
    // events never run unbounded
    assert_eq!(
        argv_for(spec("events"), &args(json!({}))).unwrap(),
        ["events", "--count=20", "--timeout=10s"]
    );
    assert_eq!(
        argv_for(spec("events"), &args(json!({"count": 3}))).unwrap(),
        ["events", "--count=3", "--timeout=10s"]
    );
    // mistakes are explained
    assert!(
        argv_for(spec("show"), &args(json!({})))
            .unwrap_err()
            .contains("missing \"id\"")
    );
    assert!(
        argv_for(spec("show"), &args(json!({"id": 3})))
            .unwrap_err()
            .contains("wrong type")
    );
    assert!(
        argv_for(spec("show"), &args(json!({"id": "a", "bogus": 1})))
            .unwrap_err()
            .contains("unknown argument \"bogus\"")
    );
}

#[derive(Default)]
struct Spy {
    seen: Mutex<Vec<Vec<String>>>,
}

#[async_trait]
impl CommandRunner for Spy {
    async fn run(&self, argv: Vec<String>) -> (i32, String, String) {
        self.seen.lock().unwrap().push(argv);
        (0, r#"{"stopped":true}"#.into(), String::new())
    }
}

fn text(o: &ToolOutput) -> String {
    o.text.clone()
}

#[tokio::test]
async fn destructive_tools_only_preview_until_confirm_is_true() {
    let spy = Arc::new(Spy::default());
    let tools = owner(spy.clone());
    for tool in [
        ("stop", json!({"id": "o1"})),
        ("pr", json!({"id": "o1"})),
        ("env_end", json!({"sandbox": "s1"})),
        ("run", json!({"repo": "acme/api"})),
    ] {
        // no confirm, false, or the wrong type: nothing runs
        for confirm in [None, Some(json!(false)), Some(json!("true"))] {
            let mut a = tool.1.clone();
            if let Some(c) = confirm {
                a["confirm"] = c;
            }
            let out = tools.call(tool.0, a).await.unwrap();
            assert!(!out.is_error, "a preview is an answer, not a failure");
            let v = out.structured.unwrap();
            assert_eq!(v["confirmed"], false);
            assert_eq!(v["changed"], false);
            assert!(
                v["wouldRun"].as_str().unwrap().starts_with("ratchet "),
                "{v}"
            );
            assert!(v["hint"].as_str().unwrap().contains("confirm: true"));
        }
    }
    assert!(
        spy.seen.lock().unwrap().is_empty(),
        "nothing ran without confirm"
    );

    let mut a = json!({"id": "o1", "reason": "enough"});
    a["confirm"] = json!(true);
    let out = tools.call("stop", a).await.unwrap();
    assert_eq!(out.structured.unwrap(), json!({"stopped": true}));
    assert_eq!(
        spy.seen.lock().unwrap().as_slice(),
        [[
            "ratchet",
            "--json",
            "--yes",
            "stop",
            "--reason=enough",
            "--",
            "o1"
        ]]
    );
}

#[tokio::test]
async fn read_and_write_tools_need_no_confirmation() {
    let spy = Arc::new(Spy::default());
    let tools = owner(spy.clone());
    tools.call("list", json!({})).await.unwrap();
    tools
        .call("agent_message", json!({"id": "a1", "text": "try caching"}))
        .await
        .unwrap();
    assert_eq!(spy.seen.lock().unwrap().len(), 2);
    assert!(tools.call("no_such_tool", json!({})).await.is_err());
    assert!(tools.call("mcp", json!({})).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_runs_the_command_against_the_daemon() {
    let env = Env::start().await;
    let tools = OwnerTools::new(
        vec![format!("--socket={}", env.socket())],
        Arc::new(InProcess),
    );

    let out = tools.call("status", json!({})).await.unwrap();
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(
        out.structured.as_ref().unwrap()["daemon"]["apiVersion"],
        "v1"
    );

    let out = tools.call("list", json!({"range": "90d"})).await.unwrap();
    assert_eq!(
        out.structured.unwrap()["optimizations"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let out = tools
        .call("show", json!({"id": "opt_demo_search"}))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));

    // a failure is a tool error the model can read, with the CLI's own message
    let out = tools
        .call("show", json!({"id": "opt_missing"}))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(text(&out).contains("exited with code 1"), "{}", text(&out));
    assert!(text(&out).contains("opt_missing"), "{}", text(&out));

    // a destructive call with confirm really stops the optimization
    let before = env
        .ratchet(&["show", "opt_demo_search", "--json"])
        .await
        .json();
    assert_eq!(before["summary"]["status"], "running");
    let out = tools
        .call("stop", json!({"id": "opt_demo_search", "confirm": true}))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    let after = env
        .ratchet(&["show", "opt_demo_search", "--json"])
        .await
        .json();
    assert_ne!(after["summary"]["status"], "running");
    env.stop().await;
}

/// `ratchet mcp` as a client would start it (`claude mcp add ratchet -- ratchet mcp`): JSON-RPC over stdio.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_speaks_mcp_on_stdio_and_reads_the_daemon_token_itself() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let env = Env::start().await;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ratchet"))
        .arg("mcp")
        .arg(format!("--socket={}", env.socket()))
        .env_remove("RATCHET_TOKEN")
        .env_remove("RATCHET_TOKEN_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();

    async fn ask(
        stdin: &mut tokio::process::ChildStdin,
        out: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        id: u32,
        method: &str,
        params: Value,
    ) -> Value {
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
            .to_string()
            + "\n";
        stdin.write_all(line.as_bytes()).await.unwrap();
        serde_json::from_str(&out.next_line().await.unwrap().unwrap()).unwrap()
    }

    let init = ask(
        &mut stdin,
        &mut out,
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
    )
    .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();

    let list = ask(&mut stdin, &mut out, 2, "tools/list", json!({})).await;
    let names: Vec<String> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    let expected: Vec<String> = command_table().iter().map(|c| c.tool_name()).collect();
    assert_eq!(names, expected);

    // the token comes from ratchetd.token next to the socket: none was passed
    let call = ask(
        &mut stdin,
        &mut out,
        3,
        "tools/call",
        json!({"name": "status", "arguments": {}}),
    )
    .await;
    assert_eq!(call["result"]["isError"], false, "{call}");
    assert_eq!(
        call["result"]["structuredContent"]["daemon"]["apiVersion"],
        "v1"
    );

    drop(stdin);
    assert!(
        child.wait().await.unwrap().success(),
        "the server ends when the client closes stdin"
    );
    env.stop().await;
}
