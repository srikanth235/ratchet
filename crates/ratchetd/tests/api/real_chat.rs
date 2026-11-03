//! The chat service on the real harness layer: `CliChatEngine` with the real Claude Code adapter and parser, a fake process in
//! place of the binary (it replays a stream recorded from `claude 2.1.316`), a real git worktree of a local repository.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ratchet_agents::detect::{HarnessAuth, HarnessInfo, HarnessKind as DetectKind};
use ratchet_agents::models::ModelCatalog;
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{ClaudeCodeOptions, CliAgentOptions, LocalAdapterOptions};
use ratchet_engine::desktop::{HarnessCache, HarnessProbe};
use ratchetd::api::{ChatQuery, CreateChatInput, SendChatMessageInput};
use ratchetd::chat::{ChatEngine, ChatOptions, ChatService};
use ratchetd::mcp::McpHub;
use ratchetd::real::{CliChatEngine, CliChatOptions};
use serde_json::Value;

const RECORDING: &str =
    include_str!("../../../ratchet-agents/tests/fixtures/claude-2.1.316-real-deltas.jsonl");
const RECORDED_SESSION: &str = "9c1e5d7a-0b2f-4c63-8a41-5d2f7e6b3c10";

struct Probe(Vec<HarnessInfo>);

#[async_trait]
impl HarnessProbe for Probe {
    async fn login_path(&self) -> Result<Option<String>, String> {
        Ok(Some("/usr/bin:/bin".into()))
    }
    async fn detect(&self, _: Option<&str>) -> Result<Vec<HarnessInfo>, String> {
        Ok(self.0.clone())
    }
}

fn info(kind: DetectKind, installed: bool, auth: HarnessAuth, hint: &str) -> HarnessInfo {
    HarnessInfo {
        kind,
        installed,
        version: installed.then(|| "2.1.316".to_owned()),
        path: installed.then(|| format!("/fake/{kind:?}")),
        auth,
        hint: hint.to_owned(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn local_repo(root: &Path) -> PathBuf {
    let dir = root.join("shop-origin");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(dir.join("README.md"), "# shop\n").unwrap();
    git(&dir, &["add", "."]);
    git(
        &dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "init",
        ],
    );
    dir
}

struct Rig {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    origin: PathBuf,
    spawner: FakeSpawner,
    svc: Arc<ChatService>,
    mcp: Arc<McpHub>,
    engine: Arc<CliChatEngine>,
}

fn rig(found: Vec<HarnessInfo>) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("chats");
    let origin = local_repo(tmp.path());
    let spawner = FakeSpawner::new(FakeProcessSpec::stdout(RECORDING));
    let url = origin.display().to_string();
    let engine = Arc::new(CliChatEngine::new(CliChatOptions {
        harnesses: Arc::new(HarnessCache::new(Arc::new(Probe(found)))),
        models: ModelCatalog::bundled().clone(),
        root: root.clone(),
        clone_url: Some(Arc::new(move |_slug| url.clone())),
        github_token: None,
        adapters: LocalAdapterOptions {
            spawner: Some(Arc::new(spawner.clone())),
            // one process per turn: the fake process is a replay, not a protocol
            claude: Some(ClaudeCodeOptions {
                cli: CliAgentOptions {
                    disable_live_session: true,
                    ..CliAgentOptions::default()
                },
                ..ClaudeCodeOptions::default()
            }),
            ..LocalAdapterOptions::default()
        },
    }));
    let mcp = Arc::new(McpHub::new());
    let ledger =
        futures::executor::block_on(ratchet_ledger::Ledger::open_memory(Default::default()))
            .unwrap();
    let svc = ChatService::new(
        ledger,
        engine.clone(),
        mcp.clone(),
        None,
        ChatOptions {
            ratchetd: "/usr/local/bin/ratchetd".into(),
            ..ChatOptions::default()
        },
    );
    Rig {
        _tmp: tmp,
        root,
        origin,
        spawner,
        svc,
        mcp,
        engine,
    }
}

fn claude_ready() -> Vec<HarnessInfo> {
    vec![
        info(
            DetectKind::ClaudeCode,
            true,
            HarnessAuth::SignedIn,
            "signed in",
        ),
        info(
            DetectKind::Codex,
            true,
            HarnessAuth::SignedOut,
            "Sign in to Codex: run `codex login`",
        ),
        info(
            DetectKind::Grok,
            false,
            HarnessAuth::SignedOut,
            "Install Grok",
        ),
    ]
}

async fn say(svc: &Arc<ChatService>, id: &str, text: &str) {
    svc.send_message(id, SendChatMessageInput { text: text.into() })
        .await
        .unwrap();
}

async fn done(svc: &Arc<ChatService>, id: &str, turn: u64) -> Value {
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let chat =
            serde_json::to_value(svc.get_chat(id, &ChatQuery::default()).await.unwrap()).unwrap();
        let replies = chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "assistant")
            .count() as u64;
        if chat["state"] == "idle" && replies >= turn {
            return chat;
        }
        assert!(tokio::time::Instant::now() < end, "no reply: {chat}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

#[tokio::test]
async fn the_engine_lists_harnesses_in_chat_order_with_the_catalogs_chat_models() {
    let r = rig(claude_ready());
    let list = r.engine.harnesses().await;
    let kinds: Vec<_> = list.iter().map(|h| (h.kind.id(), h.ready)).collect();
    assert_eq!(
        kinds,
        [("claude-code", true), ("codex", false), ("grok", false)]
    );
    assert_eq!(list[0].default_model.as_deref(), Some("sonnet"));
    assert_eq!(list[1].default_model.as_deref(), Some("gpt-5.1-codex-mini"));
    assert!(list[1].hint.contains("codex login"));
    // the user's harness is Claude Code unless they choose another that is ready
    let chat = r.svc.create_chat(CreateChatInput::default()).await.unwrap();
    assert_eq!(chat.harness.id(), "claude-code");
    let refused = r
        .svc
        .create_chat(CreateChatInput {
            harness: Some(ratchetd::api::HarnessKind::Codex),
            ..CreateChatInput::default()
        })
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("codex login"), "{refused}");
}

#[tokio::test]
async fn a_chat_runs_the_real_claude_adapter_read_only_with_ratchets_tools_and_resumes_its_session()
{
    let r = rig(claude_ready());
    let chat = r
        .svc
        .create_chat(CreateChatInput {
            repo: Some("acme/shop".into()),
            ..CreateChatInput::default()
        })
        .await
        .unwrap();
    let mut stream = r.svc.stream(&chat.id, None).await.unwrap();
    say(&r.svc, &chat.id, "What is in this repo?").await;
    let first = done(&r.svc, &chat.id, 1).await;

    // the reply came through the real parser: deltas first, then the message, named with the harness and the CLI's own model
    let mut kinds = Vec::new();
    while let Ok(m) = stream.live.try_recv() {
        kinds.push(m.1.name());
    }
    let first_delta = kinds
        .iter()
        .position(|k| *k == "delta")
        .expect("deltas were streamed");
    let reply = kinds.iter().position(|k| *k == "message").unwrap();
    assert!(first_delta < reply || kinds[0] == "message", "{kinds:?}");
    let reply = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap();
    assert_eq!(reply["harness"], "claude-code");
    assert_eq!(reply["model"], "claude-sonnet-5-5");
    assert!(
        reply["usage"]["inputTokens"].as_u64().unwrap() > 0,
        "the turn's usage is on the reply"
    );

    // how the CLI was started: read-only, strictly the Ratchet MCP server, the role prompt, in the chat's own checkout
    let call = r.spawner.call(0);
    let args = &call.args;
    assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    assert!(
        flag(args, "--mcp-config").is_some(),
        "the Ratchet server is handed over"
    );
    assert_eq!(
        r.mcp.sessions.len(),
        1,
        "one token, the chat's own, live while the chat is"
    );
    assert!(
        flag(args, "--append-system-prompt")
            .unwrap()
            .contains("only through the Ratchet tools")
    );
    let allowed = flag(args, "--allowedTools").unwrap_or_default();
    for write in ["Edit", "Write", "Bash"] {
        assert!(
            !allowed.split(',').any(|t| t == write),
            "{write} is not allowed: {allowed}"
        );
    }
    assert!(flag(args, "--disallowedTools").unwrap().contains("Bash"));
    assert_eq!(args.iter().filter(|a| *a == "--resume").count(), 0);
    let worktree = r.root.join("worktrees").join(&chat.id);
    assert_eq!(call.options.cwd, worktree);
    assert!(
        worktree.join("README.md").exists(),
        "a checkout of the default branch"
    );
    assert!(git(&worktree, &["branch", "--show-current"]).contains(&chat.id));

    // the second message continues the CLI session the first one started
    say(&r.svc, &chat.id, "And the tests?").await;
    done(&r.svc, &chat.id, 2).await;
    assert_eq!(
        flag(&r.spawner.call(1).args, "--resume"),
        Some(RECORDED_SESSION)
    );
    assert_eq!(r.spawner.call_count(), 2);

    // deleting removes the worktree and its branch and revokes the token
    r.svc.delete_chat(&chat.id).await.unwrap();
    assert!(!worktree.exists());
    assert!(
        r.mcp.sessions.is_empty(),
        "the token is revoked with the chat"
    );
    let clone = r.root.join("clones").join("acme__shop");
    assert!(!git(&clone, &["branch", "--list"]).contains("ratchet-chat"));
    assert!(!git(&clone, &["worktree", "list"]).contains(&chat.id));
    assert!(r.origin.exists());
}

#[tokio::test]
async fn a_home_chat_gets_an_empty_directory_and_no_repo_access() {
    let r = rig(claude_ready());
    let chat = r.svc.create_chat(CreateChatInput::default()).await.unwrap();
    say(&r.svc, &chat.id, "hello").await;
    done(&r.svc, &chat.id, 1).await;
    let cwd = r.spawner.call(0).options.cwd;
    assert_eq!(cwd, r.root.join("scratch").join(&chat.id));
    assert_eq!(std::fs::read_dir(&cwd).unwrap().count(), 0);
    assert!(!r.root.join("clones").exists(), "nothing was cloned");
    r.svc.delete_chat(&chat.id).await.unwrap();
    assert!(!cwd.exists());
}

#[tokio::test]
async fn a_repo_that_cannot_be_read_says_so_in_words() {
    let r = rig(claude_ready());
    std::fs::remove_dir_all(&r.origin).unwrap();
    let chat = r
        .svc
        .create_chat(CreateChatInput {
            repo: Some("acme/shop".into()),
            ..CreateChatInput::default()
        })
        .await
        .unwrap();
    let mut stream = r.svc.stream(&chat.id, None).await.unwrap();
    say(&r.svc, &chat.id, "hello").await;
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    let error = loop {
        let m = tokio::time::timeout_at(end, stream.live.recv())
            .await
            .expect("no error event")
            .unwrap();
        if let ratchetd::api::ChatStreamEvent::Error { error } = &*m.1 {
            break error.clone();
        }
    };
    assert_eq!(error.code, ratchetd::api::ChatErrorCode::Failed);
    assert!(
        error
            .message
            .starts_with("Ratchet could not read acme/shop"),
        "{}",
        error.message
    );
    assert_eq!(r.spawner.call_count(), 0, "the CLI never started");
}
