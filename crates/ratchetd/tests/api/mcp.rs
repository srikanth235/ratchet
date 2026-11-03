//! The MCP side of the daemon: session tokens, tool-set scoping, `draft_optimization` and the `ratchetd mcp` child process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use hyper::StatusCode;
use ratchet_core::ports::{
    CandidateCommit, CommitFilesInput, CommitRef, FileDiff, GitHost, PortError, PortResult,
    PullRequestInput, PullRequestRef,
};
use ratchet_mcp::ToolSet;
use ratchetd::mcp::config_branch::CONFIG_BRANCH;
use ratchetd::mcp::{ConfigHosts, McpHub, Scope};
use ratchetd::server::{Daemon, DaemonConfig};
use ratchetd::service::ServiceResult;
use ratchetd::sim::{SimOptions, SimService};
use serde_json::{Value, json};

use super::common::Http;

/// A git host that remembers what was committed where: the "fake git host" of the P3 test.
#[derive(Default)]
struct FakeGit {
    /// branch -> tip sha
    branches: Mutex<HashMap<String, String>>,
    commits: Mutex<Vec<CommitFilesInput>>,
    other_calls: Mutex<Vec<String>>,
}

impl FakeGit {
    fn other(&self, what: &str) -> PortError {
        self.other_calls.lock().unwrap().push(what.to_owned());
        PortError::Unsupported("fake host")
    }
}

#[async_trait]
impl GitHost for FakeGit {
    async fn create_branch(&self, _from: &str, branch: &str) -> PortResult<()> {
        Err(self.other(&format!("create_branch {branch}")))
    }
    async fn list_candidates(
        &self,
        _b: &str,
        _s: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>> {
        Err(self.other("list_candidates"))
    }
    async fn changed_files(&self, _b: &str, _s: &str) -> PortResult<Vec<String>> {
        Err(self.other("changed_files"))
    }
    async fn diff(&self, _b: &str, _s: &str) -> PortResult<Vec<FileDiff>> {
        Err(self.other("diff"))
    }
    async fn checkout(&self, _s: &str, _d: &std::path::Path) -> PortResult<()> {
        Err(self.other("checkout"))
    }
    async fn open_pull_request(&self, _i: &PullRequestInput) -> PortResult<PullRequestRef> {
        Err(self.other("open_pull_request"))
    }
    async fn resolve_ref(&self, git_ref: &str) -> PortResult<String> {
        self.branches
            .lock()
            .unwrap()
            .get(git_ref)
            .cloned()
            .ok_or_else(|| PortError::failed(format!("no such ref {git_ref}")))
    }
    fn remote_url(&self) -> String {
        "https://example.test/acme/api.git".into()
    }
    async fn commit_files(&self, input: &CommitFilesInput) -> PortResult<CommitRef> {
        let mut branches = self.branches.lock().unwrap();
        let exists = branches.contains_key(&input.branch);
        if input.update != exists {
            return Err(PortError::failed(format!(
                "branch {} {}",
                input.branch,
                if exists {
                    "already exists"
                } else {
                    "does not exist"
                }
            )));
        }
        let mut commits = self.commits.lock().unwrap();
        let sha = format!("c{:03}", commits.len() + 1);
        branches.insert(input.branch.clone(), sha.clone());
        commits.push(input.clone());
        Ok(CommitRef { sha })
    }
}

struct Hosts(Arc<FakeGit>);

#[async_trait]
impl ConfigHosts for Hosts {
    async fn host_for(&self, _repo: &str) -> ServiceResult<Arc<dyn GitHost>> {
        Ok(self.0.clone())
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    daemon: Daemon,
    sim: SimService,
    git: Arc<FakeGit>,
    owner: Http,
}

impl Rig {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sim = SimService::new(SimOptions {
            auto_run: false,
            github_flow_ms: 0,
            ..SimOptions::default()
        })
        .await
        .unwrap();
        let git = Arc::new(FakeGit::default());
        let socket = dir.path().join("ratchetd.sock");
        let mut cfg = DaemonConfig::new(&socket);
        cfg.mcp = Some(Arc::new(McpHub::with_hosts(Arc::new(Hosts(git.clone())))));
        let daemon = Daemon::start(Arc::new(sim.clone()), cfg).await.unwrap();
        let owner = Http {
            socket,
            token: Some(daemon.token.clone()),
        };
        Self {
            _dir: dir,
            daemon,
            sim,
            git,
            owner,
        }
    }

    fn session(&self, set: ToolSet, scope: Scope) -> (Http, String) {
        let s = self.daemon.mcp.sessions.issue(set, scope);
        (self.owner.with_token(&s.token), s.id)
    }

    async fn stop(self) {
        self.sim.shutdown();
        self.daemon.stop().await.unwrap();
    }
}

async fn call(http: &Http, tool: &str, arguments: Value) -> (StatusCode, Value) {
    let r = http
        .post(
            "/v1/mcp/call",
            json!({"tool": tool, "arguments": arguments}),
        )
        .await;
    (r.status, r.json())
}

#[tokio::test]
async fn a_session_token_opens_the_mcp_routes_only_and_the_owner_token_does_not_open_them() {
    let rig = Rig::start().await;
    let (chat, _) = rig.session(ToolSet::Chat, Scope::default());

    // the session token is refused everywhere the owner token works
    for path in [
        "/v1/status",
        "/v1/optimizations",
        "/v1/settings",
        "/v1/events",
    ] {
        assert_eq!(
            chat.get(path).await.status,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    // and the owner token is not a session
    let owner_on_mcp = rig.owner.get("/v1/mcp/session").await;
    assert_eq!(owner_on_mcp.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        rig.owner
            .without_token()
            .get("/v1/mcp/session")
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    let r = chat.get("/v1/mcp/session").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["set"], "chat");
    rig.stop().await;
}

#[tokio::test]
async fn revoking_a_token_ends_the_session() {
    let rig = Rig::start().await;
    let (chat, id) = rig.session(ToolSet::Chat, Scope::default());
    assert_eq!(
        call(&chat, "list_templates", json!({})).await.0,
        StatusCode::OK
    );
    assert!(rig.daemon.mcp.sessions.revoke(&id));
    assert_eq!(
        call(&chat, "list_templates", json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        chat.get("/v1/mcp/session").await.status,
        StatusCode::UNAUTHORIZED
    );
    rig.stop().await;
}

#[tokio::test]
async fn the_token_decides_the_tool_set() {
    let rig = Rig::start().await;
    let (chat, _) = rig.session(ToolSet::Chat, Scope::default());
    let (agent, _) = rig.session(
        ToolSet::Optimization,
        Scope {
            optimization: Some("nope".into()),
            ..Scope::default()
        },
    );
    // a chat token cannot use the optimization tools, and the other way round
    for tool in [
        "steering_notes",
        "history",
        "connector_query",
        "submit_candidate",
    ] {
        let (status, body) = call(&chat, tool, json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{tool}");
        assert!(
            body["message"].as_str().unwrap().contains("not available"),
            "{body}"
        );
    }
    for tool in ToolSet::Chat.names() {
        let (status, body) = call(&agent, &tool, json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{tool}");
        assert!(
            body["message"].as_str().unwrap().contains("not available"),
            "{body}"
        );
    }
    rig.stop().await;
}

#[tokio::test]
async fn read_tools_answer_with_the_apps_own_data() {
    let rig = Rig::start().await;
    let (chat, _) = rig.session(ToolSet::Chat, Scope::default());
    let (status, body) = call(&chat, "list_optimizations", json!({"range": "7d"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let direct = rig.owner.get_json("/v1/optimizations?range=7d").await;
    assert_eq!(body["result"]["optimizations"], direct["optimizations"]);
    let (status, templates) = call(&chat, "list_templates", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!templates["result"].as_array().unwrap().is_empty());
    // unknown arguments are rejected
    let (status, _) = call(&chat, "list_templates", json!({"bogus": 1})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    rig.stop().await;
}

#[tokio::test]
async fn a_chat_bound_to_an_optimization_cannot_touch_another() {
    let rig = Rig::start().await;
    let list = rig.owner.get_json("/v1/optimizations").await;
    let ids: Vec<String> = list["optimizations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(ids.len() >= 2, "the simulation seeds several optimizations");
    let (bound, _) = rig.session(
        ToolSet::Chat,
        Scope {
            optimization: Some(ids[0].clone()),
            ..Scope::default()
        },
    );
    assert_eq!(
        call(&bound, "get_optimization", json!({"optimization": ids[0]}))
            .await
            .0,
        StatusCode::OK
    );
    let (status, body) = call(&bound, "get_optimization", json!({"optimization": ids[1]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("bound to optimization")
    );
    let (status, _) = call(
        &bound,
        "add_steering_note",
        json!({"optimization": ids[1], "text": "hi"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, listed) = call(&bound, "list_optimizations", json!({})).await;
    assert_eq!(
        listed["result"]["optimizations"].as_array().unwrap().len(),
        1
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_chat_bound_to_a_repo_drafts_for_that_repo_only() {
    let rig = Rig::start().await;
    let (chat, _) = rig.session(
        ToolSet::Chat,
        Scope {
            repo: Some("acme/api".into()),
            ..Scope::default()
        },
    );
    let (status, _) = call(
        &chat,
        "draft_optimization",
        json!({"repo": "other/repo", "goal": "faster"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(rig.git.commits.lock().unwrap().is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn draft_optimization_writes_to_the_config_branch_only() {
    let rig = Rig::start().await;
    let (chat, _) = rig.session(ToolSet::Chat, Scope::default());

    let (status, first) = call(
        &chat,
        "draft_optimization",
        json!({"repo": "acme/api", "goal": "Make the test suite faster", "template": "test-suite-time",
               "schedule": "nightly", "budget": 25, "agents": ["claude-code", "codex"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let card = &first["result"];
    assert_eq!(card["configBranch"], CONFIG_BRANCH);
    assert_eq!(card["baseRef"], CONFIG_BRANCH);
    assert_eq!(card["repo"], "acme/api");
    assert_eq!(card["budgetUsd"], 25.0);
    assert_eq!(card["agents"], json!(["claude-code", "codex"]));
    assert!(card["draftId"].as_str().unwrap().starts_with("draft_"));
    assert!(
        card["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "ratchet.yaml")
    );

    // a second draft adds a commit on top of the same branch
    let (status, second) = call(
        &chat,
        "draft_optimization",
        json!({"repo": "acme/api", "goal": "Shrink the bundle", "template": "build-size"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");

    {
        let commits = rig.git.commits.lock().unwrap();
        assert_eq!(commits.len(), 2);
        assert!(
            commits.iter().all(|c| c.branch == CONFIG_BRANCH),
            "every commit goes to {CONFIG_BRANCH}: {:?}",
            commits.iter().map(|c| &c.branch).collect::<Vec<_>>()
        );
        assert!(
            !commits[0].update,
            "the first draft creates the branch from the base ref"
        );
        assert_eq!(commits[0].base_ref, "main");
        assert!(commits[1].update, "later drafts add to it");
        assert_eq!(commits[1].base_ref, CONFIG_BRANCH);
        // nothing else was touched: no other branch, no pull request, no push to the base ref
        assert!(
            rig.git.other_calls.lock().unwrap().is_empty(),
            "{:?}",
            rig.git.other_calls
        );
        assert!(!rig.git.branches.lock().unwrap().contains_key("main"));
        assert_eq!(rig.git.branches.lock().unwrap().len(), 1);
        // the brief carries the goal
        let brief = commits[0]
            .files
            .iter()
            .find(|f| f.path == "RATCHET.md")
            .unwrap();
        assert!(
            brief
                .content
                .starts_with("# Goal\n\nMake the test suite faster")
        );
    }

    // bad input is refused before anything is written
    for bad in [
        json!({"repo": "not a repo", "goal": "x"}),
        json!({"repo": "acme/api", "goal": "  "}),
        json!({"repo": "acme/api", "goal": "x", "template": "no-such-template"}),
        json!({"repo": "acme/api", "goal": "x", "budget": -1}),
    ] {
        assert_eq!(
            call(&chat, "draft_optimization", bad).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(rig.git.commits.lock().unwrap().len(), 2);
    rig.stop().await;
}

#[tokio::test]
async fn create_optimization_from_a_draft_starts_it_without_asking_anyone() {
    let rig = Rig::start().await;
    let (chat, _) = rig.session(ToolSet::Chat, Scope::default());
    let (_, drafted) = call(
        &chat,
        "draft_optimization",
        json!({"repo": "acme/api", "goal": "Make the test suite faster"}),
    )
    .await;
    let draft_id = drafted["result"]["draftId"].as_str().unwrap().to_owned();

    let (status, created) = call(&chat, "create_optimization", json!({"draft_id": draft_id})).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["result"]["started"], true);
    let id = created["result"]["optimization"].as_str().unwrap();
    let detail = rig.owner.get_json(&format!("/v1/optimizations/{id}")).await;
    assert_eq!(detail["summary"]["repo"], "acme/api");
    assert_eq!(detail["baseRef"], CONFIG_BRANCH);
    let status_text = detail["summary"]["status"].as_str().unwrap();
    assert!(
        !matches!(status_text, "draft" | "needs-you" | "waiting"),
        "started, not waiting on a person: {status_text}"
    );

    // the draft is spent; a second create is a conflict
    let (status, _) = call(&chat, "create_optimization", json!({"draft_id": draft_id})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &chat,
        "create_optimization",
        json!({"draft_id": "draft_missing"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    rig.stop().await;
}

fn ratchetd_for(rig: &Rig, token: &str) -> ratchet_core::ports::McpServer {
    ratchet_mcp::launch::ratchetd_server(
        env!("CARGO_BIN_EXE_ratchetd"),
        token,
        Some(&rig.owner.socket),
    )
}

/// Each harness gets `ratchetd mcp --session <token>` through its own flags or config; started from each rendering, the server
/// lists the chat set exactly, and the daemon refuses what is outside it.
#[tokio::test(flavor = "multi_thread")]
async fn each_harness_rendering_starts_ratchetd_mcp_and_lists_exactly_the_chat_tools() {
    use ratchet_mcp::testing::StdioClient;
    use std::collections::BTreeMap;

    let rig = Rig::start().await;
    let issued = rig
        .daemon
        .mcp
        .sessions
        .issue(ToolSet::Chat, Scope::default());
    let renderings =
        ratchet_agents::testing::rendered_mcp_servers(&ratchetd_for(&rig, &issued.token));
    assert_eq!(renderings.len(), 5);
    for (harness, server) in renderings {
        assert_eq!(
            server.args,
            ["mcp", "--session", issued.token.as_str()],
            "{harness}"
        );
        let mut client = StdioClient::spawn(&server, &BTreeMap::new()).await.unwrap();
        client.initialize().await;
        assert_eq!(
            client.tool_names().await,
            ToolSet::Chat.names(),
            "{harness} lists exactly the chat set"
        );
        // a call goes through the daemon...
        let ok = client.call("list_templates", json!({})).await;
        assert_eq!(ok["result"]["isError"], false, "{harness}: {ok}");
        // ...and one outside the set is refused before it gets there
        for tool in ["steering_notes", "submit_candidate"] {
            let r = client.call(tool, json!({})).await;
            assert_eq!(r["error"]["code"], -32602, "{harness} {tool}");
        }
        assert!(client.finish().await, "{harness}");
    }
    assert!(rig.git.commits.lock().unwrap().is_empty());
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ratchetd_mcp_serves_the_optimization_set_for_an_optimization_token_and_stops_after_revocation()
 {
    use ratchet_mcp::testing::StdioClient;
    use std::collections::BTreeMap;

    let rig = Rig::start().await;
    let issued = rig.daemon.mcp.sessions.issue(
        ToolSet::Optimization,
        Scope {
            optimization: Some("opt_demo_search".into()),
            ..Scope::default()
        },
    );
    let mut client = StdioClient::spawn(&ratchetd_for(&rig, &issued.token), &BTreeMap::new())
        .await
        .unwrap();
    client.initialize().await;
    assert_eq!(client.tool_names().await, ToolSet::Optimization.names());
    let history = client.call("history", json!({"limit": 3})).await;
    assert_eq!(history["result"]["isError"], false, "{history}");
    assert!(history["result"]["structuredContent"]["series"]["metric"].is_string());
    let chat_tool = client
        .call("run_now", json!({"optimization": "opt_demo_search"}))
        .await;
    assert_eq!(chat_tool["error"]["code"], -32602);

    // revoke: the running server's next call fails with a readable message
    rig.daemon.mcp.sessions.revoke(&issued.id);
    let after = client.call("history", json!({})).await;
    assert_eq!(after["result"]["isError"], true);
    assert!(
        after["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("revoked")
    );
    assert!(client.finish().await);

    // and a revoked token cannot even start a server
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_ratchetd"))
        .args(["mcp", "--session", &issued.token])
        .env_clear()
        .env("RATCHET_SOCKET", &rig.owner.socket)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "stdout carries JSON-RPC only");
    assert!(String::from_utf8_lossy(&out.stderr).contains("revoked"));
    rig.stop().await;
}

// ---------------------------------------------------------------------------------------------------- P6: the optimization tools, locally

struct Answers;

#[async_trait]
impl ratchetd::mcp::ConnectorQuery for Answers {
    async fn query(&self, connector: &str, query: &str) -> ServiceResult<Value> {
        match connector {
            "datadog" => Ok(json!({"connector": "datadog", "series": [1, 2, 3], "query": query})),
            "grafana" => Err(ratchetd::service::ServiceError::NotReady(
                "Needs you: Grafana is signed out. Sign in again in Customize > Connectors; this optimization carries on without it."
                    .into(),
            )),
            other => Err(ratchetd::service::ServiceError::NotFound(format!(
                "no connector named \"{other}\""
            ))),
        }
    }
}

#[tokio::test]
async fn connector_query_and_submit_candidate_work_for_a_local_agent_and_stay_inside_its_scope() {
    let rig = Rig::start().await;
    let agent = "opt_demo_search_agt_0".to_owned();
    let (opt, _) = rig.session(
        ToolSet::Optimization,
        Scope {
            optimization: Some("opt_demo_search".into()),
            agent: Some(agent.clone()),
            ..Scope::default()
        },
    );

    // no connector can be asked until the daemon has someone to ask
    let (status, body) = call(
        &opt,
        "connector_query",
        json!({"connector": "datadog", "query": "p95"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");

    rig.daemon.mcp.set_connector_query(Arc::new(Answers));
    let (status, body) = call(
        &opt,
        "connector_query",
        json!({"connector": "datadog", "query": "p95"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["series"], json!([1, 2, 3]));
    // a signed-out connector is the one thing that needs a person: the agent is told, and carries on
    let (status, body) = call(
        &opt,
        "connector_query",
        json!({"connector": "grafana", "query": "p95"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .starts_with("Needs you: Grafana is signed out")
    );
    // a query is read-only text of bounded size
    let (status, _) = call(
        &opt,
        "connector_query",
        json!({"connector": "datadog", "query": "x".repeat(2001)}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // submit_candidate goes to the agent's own runner and shows in its log
    let (status, body) = call(
        &opt,
        "submit_candidate",
        json!({"sha": "abc1234", "note": "cache the parse"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["submitted"], "abc1234");
    let log = rig
        .owner
        .get_json(&format!("/v1/agents/{agent}/events"))
        .await;
    assert!(
        log["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["text"] == "submit_candidate named abc1234"),
        "{log}"
    );
    // a session with no agent cannot name a candidate
    let (no_agent, _) = rig.session(
        ToolSet::Optimization,
        Scope {
            optimization: Some("opt_demo_search".into()),
            ..Scope::default()
        },
    );
    let (status, _) = call(&no_agent, "submit_candidate", json!({"sha": "abc1234"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    rig.stop().await;
}

#[tokio::test]
async fn the_agents_list_is_readable_and_changeable_through_the_api() {
    let rig = Rig::start().await;
    let got = rig
        .owner
        .get_json("/v1/optimizations/opt_demo_search/agents")
        .await;
    assert_eq!(got["source"], "settings");
    assert!(!got["harnesses"].as_array().unwrap().is_empty());
    assert_eq!(got["waitLimitS"], 1800);
    assert_eq!(got["crashStrikes"], 3);
    assert_eq!(got["recheckS"], 300);

    let put = rig
        .owner
        .send(
            "PUT",
            "/v1/optimizations/opt_demo_search/agents",
            Some(json!({"harnesses": ["claude-code", "codex", "grok"], "crashStrikes": 2})),
        )
        .await;
    assert_eq!(put.status, StatusCode::OK, "{}", put.text());
    let v = put.json();
    assert_eq!(v["summary"], "Claude Code \u{2192} Codex \u{2192} Grok");
    assert_eq!(v["crashStrikes"], 2);
    let again = rig
        .owner
        .get_json("/v1/optimizations/opt_demo_search/agents")
        .await;
    assert_eq!(again["harnesses"], json!(["claude-code", "codex", "grok"]));

    for (body, why) in [
        (json!({"harnesses": []}), "an empty list"),
        (json!({"harnesses": ["mock"]}), "mock is not an agent"),
        (
            json!({"harnesses": ["codex"], "crashStrikes": 0}),
            "zero strikes",
        ),
    ] {
        let r = rig
            .owner
            .send(
                "PUT",
                "/v1/optimizations/opt_demo_search/agents",
                Some(body),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{why}: {}", r.text());
    }
    let missing = rig.owner.get("/v1/optimizations/nope/agents").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    rig.stop().await;
}
