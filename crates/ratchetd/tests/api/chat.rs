//! Chat: `POST /v1/chats` and friends, the stream, cards, steering, interrupt, resume, revocation. Every chat here runs on the
//! scripted harness (no model, no CLI); its tools are the daemon's real ones, so a chat creates a real simulated optimization.

use std::time::Duration;

use hyper::StatusCode;
use ratchet_core::ports::Access;
use ratchetd::chat::{ChatEngine, ChatHarness, ChatOptions, ScriptedChatEngine};
use serde_json::{Value, json};
use std::sync::Arc;

use super::common::{Http, Sse, SseReader, TestDaemon};

fn quick() -> ChatOptions {
    ChatOptions {
        idle: Duration::from_secs(600),
        ..ChatOptions::default()
    }
}

async fn daemon() -> TestDaemon {
    TestDaemon::with_chat(quick()).await
}

async fn create(d: &TestDaemon, body: Value) -> Value {
    let r = d.http.post("/v1/chats", body).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    r.json()
}

async fn say(d: &TestDaemon, id: &str, text: &str) -> Value {
    let r = d
        .http
        .post(&format!("/v1/chats/{id}/messages"), json!({"text": text}))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
    r.json()
}

/// Everything up to and including the next `done`.
async fn turn(sse: &mut SseReader) -> Vec<Sse> {
    let mut out = Vec::new();
    loop {
        let m = sse.next().await.expect("the stream ended before done");
        let done = m.event.as_deref() == Some("done");
        out.push(m);
        if done {
            return out;
        }
    }
}

fn names(events: &[Sse]) -> Vec<&str> {
    events
        .iter()
        .map(|e| e.event.as_deref().unwrap_or("?"))
        .collect()
}

fn of<'a>(events: &'a [Sse], name: &str) -> Vec<&'a Sse> {
    events
        .iter()
        .filter(|e| e.event.as_deref() == Some(name))
        .collect()
}

fn token_of(spec: &ratchet_core::ports::SessionSpec) -> String {
    let server = spec.mcp.iter().find(|s| s.name == "ratchet").unwrap();
    let at = server.args.iter().position(|a| a == "--session").unwrap();
    server.args[at + 1].clone()
}

async fn session_status(d: &TestDaemon, token: &str) -> StatusCode {
    d.http.with_token(token).get("/v1/mcp/session").await.status
}

async fn until(secs: u64, mut ok: impl AsyncFnMut() -> bool) {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !ok().await {
        assert!(tokio::time::Instant::now() < end, "timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn engine(d: &TestDaemon) -> Arc<ScriptedChatEngine> {
    d.sim.scripted_chat()
}

async fn opened_stream(d: &TestDaemon, id: &str) -> SseReader {
    let mut sse = d.http.stream(&format!("/v1/chats/{id}/stream"), None).await;
    let ready = sse.next().await.unwrap();
    assert_eq!(ready.event.as_deref(), Some("ready"));
    assert_eq!(ready.id, None, "ready is not part of the sequence");
    sse
}

// ---------------------------------------------------------------- the end-to-end path

#[tokio::test]
async fn a_scripted_chat_creates_an_optimization_end_to_end_with_no_human_gate() {
    let d = daemon().await;
    let chat = create(&d, json!({"repo": "acme/shop"})).await;
    assert_eq!(chat["harness"], "claude-code");
    assert_eq!(chat["model"], "sonnet");
    assert_eq!(chat["state"], "idle");
    assert_eq!(chat["repo"], "acme/shop");
    assert_eq!(
        chat["live"], false,
        "no CLI is started before the first message"
    );
    let id = chat["id"].as_str().unwrap().to_owned();
    let mut sse = opened_stream(&d, &id).await;

    // the user's message: drafted, with a card
    let accepted = say(
        &d,
        &id,
        "Make checkout p95 faster in acme/shop. Run it every night, $50 a run.",
    )
    .await;
    assert_eq!(accepted, json!({"seq": 1, "turn": 1}));
    let first = turn(&mut sse).await;
    let n = names(&first);
    assert_eq!(n[0], "message", "the user's text is echoed first: {n:?}");
    assert_eq!(first[0].data["message"]["role"], "user");
    assert_eq!(n.last(), Some(&"done"));
    assert!(n.contains(&"card") && n.contains(&"delta") && n.contains(&"usage"));

    // ids count up by one per event, from the first after `ready`
    for pair in first.windows(2) {
        assert_eq!(pair[1].id.unwrap(), pair[0].id.unwrap() + 1, "{n:?}");
    }

    // steps: reading the repo, the templates, drafting
    let steps: Vec<(String, String)> = of(&first, "tool")
        .iter()
        .map(|t| {
            (
                t.data["tool"]["name"].as_str().unwrap().to_owned(),
                t.data["tool"]["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        steps,
        [
            ("checkout".to_owned(), "running".to_owned()),
            ("checkout".to_owned(), "done".to_owned()),
            ("list_templates".to_owned(), "running".to_owned()),
            ("list_templates".to_owned(), "done".to_owned()),
            ("draft_optimization".to_owned(), "running".to_owned()),
            ("draft_optimization".to_owned(), "done".to_owned()),
        ]
    );

    // the card carries one editable row per setting, from what the user said
    let card = &of(&first, "card")[0].data["card"];
    assert_eq!(card["kind"], "draft");
    assert_eq!(card["status"], "draft");
    assert_eq!(card["repo"], "acme/shop");
    let rows: Vec<(&str, &str)> = card["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["key"].as_str().unwrap(), r["value"].as_str().unwrap()))
        .collect();
    assert!(rows.contains(&("limits", "up to $50")), "{rows:?}");
    assert!(
        rows.contains(&("schedule", "every night at 02:00")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|(k, _)| *k == "agents"));
    let draft_id = card["id"].as_str().unwrap().to_owned();

    // deltas, in order, add up to the finished reply, which names its harness and model
    let reply = of(&first, "message")
        .into_iter()
        .find(|m| m.data["message"]["role"] == "assistant")
        .unwrap();
    assert_eq!(reply.data["message"]["harness"], "claude-code");
    assert_eq!(reply.data["message"]["model"], "sonnet");
    let streamed: String = of(&first, "delta")
        .iter()
        .map(|e| e.data["text"].as_str().unwrap())
        .collect();
    assert_eq!(streamed, reply.data["message"]["text"].as_str().unwrap());
    let done = &first.last().unwrap().data;
    assert_eq!(done["outcome"], "completed");
    assert_eq!(done["turn"], 1);

    // "create it": the same call the card's button makes; the card updates in place and the optimization exists
    say(&d, &id, "Looks good, create it").await;
    let second = turn(&mut sse).await;
    let updated = &of(&second, "card")[0].data["card"];
    assert_eq!(
        updated["id"],
        draft_id.as_str(),
        "an update of the same card"
    );
    assert_eq!(updated["status"], "created");
    let optimization = updated["optimization"].as_str().unwrap().to_owned();
    assert_eq!(second.last().unwrap().data["outcome"], "completed");
    let detail = d
        .http
        .get_json(&format!("/v1/optimizations/{optimization}"))
        .await;
    assert_eq!(detail["summary"]["repo"], "acme/shop");

    // the history has everything, in order, with the card in its final state
    let history = d.http.get_json(&format!("/v1/chats/{id}")).await;
    let roles: Vec<&str> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles.first(), Some(&"user"));
    assert!(roles.contains(&"card") && roles.contains(&"tool") && roles.contains(&"assistant"));
    let seqs: Vec<i64> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["seq"].as_i64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
    let stored_card = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "card")
        .unwrap();
    assert_eq!(stored_card["card"]["status"], "created");
    d.stop().await;
}

#[tokio::test]
async fn a_new_draft_replaces_the_old_one_and_the_card_updates() {
    let d = daemon().await;
    let id = create(&d, json!({"repo": "acme/shop"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "Speed up the test suite in acme/shop").await;
    let first = turn(&mut sse).await;
    let draft_one = of(&first, "card")[0].data["card"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    say(&d, &id, "use Codex too, and only weekdays").await;
    let second = turn(&mut sse).await;
    let cards = of(&second, "card");
    assert_eq!(
        cards.len(),
        2,
        "the old card is marked, the new one arrives"
    );
    assert_eq!(cards[0].data["card"]["id"], draft_one.as_str());
    assert_eq!(cards[0].data["card"]["status"], "replaced");
    assert_eq!(cards[1].data["replaces"], draft_one.as_str());
    assert_eq!(cards[1].data["card"]["status"], "draft");
    let agents = cards[1].data["card"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "agents")
        .unwrap();
    assert_eq!(agents["value"], "Claude Code → Codex · on this Mac");
    assert_eq!(agents["raw"], json!(["claude-code", "codex"]));
    d.stop().await;
}

#[tokio::test]
async fn the_session_is_read_only_with_the_role_prompt_and_a_scoped_token() {
    let d = daemon().await;
    let id = create(&d, json!({"repo": "acme/shop"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "hello").await;
    turn(&mut sse).await;

    let opened = engine(&d).opened();
    assert_eq!(opened.len(), 1);
    let spec = &opened[0].spec;
    assert_eq!(spec.access, Access::ReadOnly);
    assert_eq!(spec.model.as_deref(), Some("sonnet"));
    assert_eq!(spec.resume, None);
    let prompt = spec.system_append.as_deref().unwrap();
    assert!(prompt.contains("only through the Ratchet tools") && prompt.contains("acme/shop"));
    assert!(
        std::path::Path::new(&spec.workdir)
            .join("README.md")
            .exists()
    );
    // the token is the chat's own: it opens the chat tool set for acme/shop
    let session = d
        .http
        .with_token(&token_of(spec))
        .get_json("/v1/mcp/session")
        .await;
    assert_eq!(session["set"], "chat");
    assert_eq!(session["repo"], "acme/shop");
    d.stop().await;
}

// ---------------------------------------------------------------- interrupt, resume, idle, delete

#[tokio::test]
async fn interrupt_stops_a_slow_reply_and_keeps_what_was_written() {
    let d = daemon().await;
    let id = create(&d, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "think about it [simulate:slow]").await;
    sse.until("delta").await;

    // a second message while replying is refused, with words for it
    let busy = d
        .http
        .post(
            &format!("/v1/chats/{id}/messages"),
            json!({"text": "again"}),
        )
        .await;
    assert_eq!(busy.status, StatusCode::CONFLICT);
    assert!(busy.json()["message"].as_str().unwrap().contains("Stop"));
    assert_eq!(
        d.http.get_json(&format!("/v1/chats/{id}")).await["state"],
        "replying"
    );
    assert!(
        d.http.get_json(&format!("/v1/chats/{id}")).await["pending"]["text"]
            .as_str()
            .unwrap()
            .contains("still working")
    );

    let r = d
        .http
        .post(&format!("/v1/chats/{id}/interrupt"), json!(null))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let rest = turn(&mut sse).await;
    assert_eq!(rest.last().unwrap().data["outcome"], "interrupted");
    assert!(of(&rest, "error").is_empty(), "stopping is not an error");

    let chat = d.http.get_json(&format!("/v1/chats/{id}")).await;
    assert_eq!(chat["state"], "idle");
    let last = chat["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["role"], "assistant");
    assert!(last["text"].as_str().unwrap().contains("still working"));

    // interrupting again is fine, and the chat takes the next message
    let again = d
        .http
        .post(&format!("/v1/chats/{id}/interrupt"), json!(null))
        .await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    say(&d, &id, "never mind").await;
    assert_eq!(
        turn(&mut sse).await.last().unwrap().data["outcome"],
        "completed"
    );
    d.stop().await;
}

#[tokio::test]
async fn a_chat_that_idles_closes_its_session_and_reopens_with_resume() {
    let d = TestDaemon::with_chat(ChatOptions {
        idle: Duration::from_millis(250),
        ..ChatOptions::default()
    })
    .await;
    let id = create(&d, json!({"repo": "acme/shop"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "Make acme/shop faster").await;
    let first = turn(&mut sse).await;
    let draft = of(&first, "card")[0].data["card"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        d.http.get_json(&format!("/v1/chats/{id}")).await["live"],
        true
    );
    let old_token = token_of(&engine(&d).opened()[0].spec);
    assert_eq!(session_status(&d, &old_token).await, StatusCode::OK);

    // quiet: the session closes, its token is revoked, the chat is still there
    until(5, async || {
        d.http.get_json(&format!("/v1/chats/{id}")).await["live"] == false
    })
    .await;
    assert_eq!(engine(&d).closed(), ["scripted-1"]);
    assert_eq!(
        session_status(&d, &old_token).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        d.http.get_json(&format!("/v1/chats/{id}")).await["state"],
        "idle"
    );

    // the next message opens a new session that continues the old CLI session; the conversation carried over
    say(&d, &id, "create it").await;
    let second = turn(&mut sse).await;
    assert_eq!(second.last().unwrap().data["outcome"], "completed");
    let opened = engine(&d).opened();
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[1].spec.resume.as_deref(), Some("scripted-1"));
    assert_eq!(opened[1].session_id, "scripted-1");
    let new_token = token_of(&opened[1].spec);
    assert_ne!(new_token, old_token);
    assert_eq!(session_status(&d, &new_token).await, StatusCode::OK);
    let card = &of(&second, "card")[0].data["card"];
    assert_eq!(
        (
            card["id"].as_str().unwrap(),
            card["status"].as_str().unwrap()
        ),
        (draft.as_str(), "created")
    );
    d.stop().await;
}

#[tokio::test]
async fn deleting_a_chat_revokes_its_token_removes_its_checkout_and_ends_its_stream() {
    let d = daemon().await;
    let id = create(&d, json!({"repo": "acme/shop"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "hello").await;
    turn(&mut sse).await;
    let spec = engine(&d).opened()[0].spec.clone();
    let token = token_of(&spec);
    let checkout = std::path::PathBuf::from(&spec.workdir);
    assert!(checkout.exists());
    assert_eq!(session_status(&d, &token).await, StatusCode::OK);

    let r = d.http.delete(&format!("/v1/chats/{id}")).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(session_status(&d, &token).await, StatusCode::UNAUTHORIZED);
    assert!(!checkout.exists(), "the checkout is gone");
    assert_eq!(engine(&d).closed(), ["scripted-1"]);
    assert_eq!(
        d.http.get(&format!("/v1/chats/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        d.http.delete(&format!("/v1/chats/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(sse.next().await.is_none(), "the stream ends with the chat");
    d.stop().await;
}

#[tokio::test]
async fn the_pool_closes_the_session_idle_the_longest() {
    let d = TestDaemon::with_chat(ChatOptions {
        max_live: 2,
        ..quick()
    })
    .await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let id = create(&d, json!({})).await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut sse = opened_stream(&d, &id).await;
        say(&d, &id, "hi").await;
        turn(&mut sse).await;
        ids.push(id);
    }
    assert_eq!(engine(&d).opened().len(), 3);
    assert_eq!(
        engine(&d).closed(),
        ["scripted-1"],
        "the first chat's session made room"
    );
    let live: Vec<bool> = {
        let mut v = Vec::new();
        for id in &ids {
            v.push(d.http.get_json(&format!("/v1/chats/{id}")).await["live"] == true);
        }
        v
    };
    assert_eq!(live, [false, true, true]);
    d.stop().await;
}

// ---------------------------------------------------------------- harness choice and failures

#[tokio::test]
async fn the_harness_is_the_request_then_the_users_default_then_the_first_signed_in() {
    let d = daemon().await;
    // first signed in
    assert_eq!(create(&d, json!({})).await["harness"], "claude-code");
    // the request wins
    let codex = create(&d, json!({"harness": "codex", "model": "gpt-5"})).await;
    assert_eq!(
        (codex["harness"].as_str(), codex["model"].as_str()),
        (Some("codex"), Some("gpt-5"))
    );
    let default_model = create(&d, json!({"harness": "codex"})).await;
    assert_eq!(default_model["model"], "gpt-5.1-codex-mini");
    // the user's default, when no request names one
    d.sim
        .ledger()
        .kv_set(ratchetd::chat::DEFAULT_HARNESS_KEY, "codex")
        .await
        .unwrap();
    assert_eq!(create(&d, json!({})).await["harness"], "codex");
    // a default that is signed out falls back to the first that is signed in
    d.sim
        .ledger()
        .kv_set(ratchetd::chat::DEFAULT_HARNESS_KEY, "grok")
        .await
        .unwrap();
    assert_eq!(create(&d, json!({})).await["harness"], "claude-code");
    // a harness that is asked for and not ready is refused with its hint
    let r = d.http.post("/v1/chats", json!({"harness": "grok"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["code"], "not_ready");
    assert!(r.json()["message"].as_str().unwrap().contains("grok login"));
    // none ready: say what to do
    engine(&d).set_harnesses(Vec::new());
    let none = d.http.post("/v1/chats", json!({})).await;
    assert_eq!(none.status, StatusCode::CONFLICT);
    assert!(
        none.json()["message"]
            .as_str()
            .unwrap()
            .starts_with("No agent is signed in")
    );
    d.stop().await;
}

#[tokio::test]
async fn only_the_first_chat_with_a_harness_carries_the_data_notice() {
    let d = daemon().await;
    let first = create(&d, json!({})).await;
    assert_eq!(first["dataNotice"]["harness"], "claude-code");
    assert!(first["dataNotice"]["text"].as_str().unwrap().starts_with(
        "First time: your messages, the repo name and the files it reads go to Claude Code"
    ));
    let second = create(&d, json!({})).await;
    assert!(second.get("dataNotice").is_none());
    let codex = create(&d, json!({"harness": "codex"})).await;
    assert_eq!(codex["dataNotice"]["harness"], "codex");
    // it is still the first chat's on a later read
    let again = d
        .http
        .get_json(&format!("/v1/chats/{}", first["id"].as_str().unwrap()))
        .await;
    assert!(again.get("dataNotice").is_some());
    d.stop().await;
}

#[tokio::test]
async fn a_rate_limit_is_told_with_when_it_lifts_and_a_way_out() {
    let d = daemon().await;
    let id = create(&d, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "hello [simulate:rate-limit]").await;
    let events = turn(&mut sse).await;
    let error = &of(&events, "error")[0].data["error"];
    assert_eq!(error["code"], "rate_limited");
    assert_eq!(error["harness"], "claude-code");
    assert_eq!(error["title"], "Claude Code reached its usage limit");
    let resets = error["retry"]["resetsAt"].as_i64().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        (resets - now - 3_600_000).abs() < 60_000,
        "resets in about an hour"
    );
    assert_eq!(error["retry"]["canRetryNow"], false);
    assert_eq!(error["fixes"], json!(["switch-harness"]));
    assert_eq!(error["switchTo"], "codex");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("continue with Codex")
    );
    let done = &events.last().unwrap().data;
    assert_eq!(done["outcome"], "failed");
    // the order: error, then done
    let n = names(&events);
    assert_eq!(&n[n.len() - 2..], ["error", "done"]);

    // the chat is usable again (a fresh session is opened with resume)
    say(&d, &id, "try once more").await;
    assert_eq!(
        turn(&mut sse).await.last().unwrap().data["outcome"],
        "completed"
    );
    assert_eq!(engine(&d).opened().len(), 2);
    d.stop().await;
}

#[tokio::test]
async fn an_expired_login_says_to_sign_in_and_a_crash_says_to_try_again() {
    let d = daemon().await;
    let id = create(&d, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "[simulate:auth-expired]").await;
    let auth = turn(&mut sse).await;
    let e = &of(&auth, "error")[0].data["error"];
    assert_eq!(e["code"], "auth_expired");
    assert_eq!(e["title"], "Claude Code needs you to sign in again");
    assert_eq!(e["fixes"], json!(["sign-in", "switch-harness"]));
    assert_eq!(e["retry"]["canRetryNow"], false);

    say(&d, &id, "[simulate:crash]").await;
    let crash = turn(&mut sse).await;
    let e = &of(&crash, "error")[0].data["error"];
    assert_eq!(e["code"], "failed");
    assert_eq!(e["fixes"], json!(["retry"]));
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("exited with status 1.")
    );
    assert_eq!(e["retry"]["canRetryNow"], true);
    d.stop().await;
}

#[tokio::test]
async fn a_chat_whose_harness_was_signed_out_meanwhile_says_so() {
    let d = daemon().await;
    let id = create(&d, json!({"harness": "codex"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    let mut list = engine(&d).harnesses().await;
    list.iter_mut().for_each(|h: &mut ChatHarness| {
        if h.kind == ratchetd::api::HarnessKind::Codex {
            h.ready = false;
            h.hint = "Sign in to Codex: run `codex login`.".into();
        }
    });
    engine(&d).set_harnesses(list);
    say(&d, &id, "hello").await;
    let events = turn(&mut sse).await;
    let e = &of(&events, "error")[0].data["error"];
    assert_eq!(e["code"], "unavailable");
    assert!(e["message"].as_str().unwrap().contains("codex login"));
    assert_eq!(engine(&d).opened().len(), 0, "nothing was started");
    d.stop().await;
}

// ---------------------------------------------------------------- steering

async fn start_demo(d: &TestDaemon) -> String {
    let r = d
        .http
        .post(
            "/v1/optimizations",
            json!({"demo": "demo-search", "agents": ["claude-code"], "budgetUsd": 50,
                   "timeLimitSeconds": 3600, "plateauEvals": 50}),
        )
        .await;
    let id = r.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(
        d.http
            .post(&format!("/v1/optimizations/{id}/start"), json!(null))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    id
}

#[tokio::test]
async fn a_steering_chat_queues_a_note_and_it_turns_read_when_an_agent_takes_it_up() {
    let d = daemon().await;
    let opt = start_demo(&d).await;
    let chat = create(&d, json!({"optimization": opt})).await;
    assert_eq!(chat["optimization"], opt.as_str());
    let id = chat["id"].as_str().unwrap().to_owned();
    let mut sse = opened_stream(&d, &id).await;

    say(&d, &id, "focus on database queries").await;
    let events = turn(&mut sse).await;
    let queued = &of(&events, "note")[0].data;
    assert_eq!(queued["card"]["kind"], "note");
    assert_eq!(queued["card"]["status"], "queued");
    assert_eq!(queued["card"]["title"], "focus on database queries");
    assert_eq!(queued["card"]["optimization"], opt.as_str());
    let note_id = queued["card"]["id"].as_str().unwrap().to_owned();

    // the agents act: the note is read
    for _ in 0..3 {
        d.sim.tick(&opt).await.unwrap();
    }
    let read = loop {
        let m = sse.until("note").await;
        if m.data["card"]["status"] == "read" {
            break m;
        }
    };
    assert_eq!(read.data["card"]["id"], note_id.as_str());
    assert_eq!(read.data["card"]["readBy"], "Claude Code");
    assert!(read.data["card"]["readAt"].as_i64().unwrap() > 0);
    assert_eq!(read.data["seq"], queued["seq"], "an update of the same row");

    // the history keeps the note in its final state
    let history = d.http.get_json(&format!("/v1/chats/{id}")).await;
    let note = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "note")
        .unwrap();
    assert_eq!(note["card"]["status"], "read");
    // the session is bound to the optimization
    let token = token_of(&engine(&d).opened()[0].spec);
    let s = d.http.with_token(&token).get_json("/v1/mcp/session").await;
    assert_eq!(s["optimization"], opt.as_str());
    d.stop().await;
}

#[tokio::test]
async fn a_chat_for_an_optimization_that_does_not_exist_is_refused() {
    let d = daemon().await;
    let r = d
        .http
        .post("/v1/chats", json!({"optimization": "opt_nope"}))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    d.stop().await;
}

// ---------------------------------------------------------------- the stream

#[tokio::test]
async fn a_client_that_reconnects_gets_what_it_missed_and_one_that_is_too_far_behind_is_told_to_refetch()
 {
    let d = daemon().await;
    let id = create(&d, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    say(&d, &id, "hello there").await;
    let events = turn(&mut sse).await;
    let ids: Vec<u64> = events.iter().map(|e| e.id.unwrap()).collect();
    let cut = ids[2];

    let mut again = d
        .http
        .stream(&format!("/v1/chats/{id}/stream"), Some(cut))
        .await;
    let ready = again.next().await.unwrap();
    assert_eq!(ready.data["resync"], false);
    assert_eq!(ready.data["lastEventId"], *ids.last().unwrap());
    let mut replayed = Vec::new();
    while replayed.len() < ids.len() - 3 {
        replayed.push(again.next().await.unwrap().id.unwrap());
    }
    assert_eq!(
        replayed,
        ids[3..],
        "exactly the events after the one it saw"
    );

    // from before a restart (an id the chat never reached)
    let mut future = d
        .http
        .stream(&format!("/v1/chats/{id}/stream"), Some(9999))
        .await;
    assert_eq!(future.next().await.unwrap().data["resync"], true);
    d.stop().await;
}

#[tokio::test]
async fn a_client_that_joins_mid_reply_gets_the_partial_text_in_ready() {
    let d = daemon().await;
    let id = create(&d, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut first = opened_stream(&d, &id).await;
    say(&d, &id, "go [simulate:slow]").await;
    first.until("delta").await;
    first.until("delta").await;

    let mut late = d.http.stream(&format!("/v1/chats/{id}/stream"), None).await;
    let ready = late.next().await.unwrap();
    assert_eq!(ready.data["state"], "replying");
    assert!(
        ready.data["pending"]["text"]
            .as_str()
            .unwrap()
            .starts_with("still working")
    );
    d.http
        .post(&format!("/v1/chats/{id}/interrupt"), json!(null))
        .await;
    turn(&mut first).await;
    d.stop().await;
}

// ---------------------------------------------------------------- input

#[tokio::test]
async fn bad_input_is_refused_with_a_reason() {
    let d = daemon().await;
    let id = create(&d, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/v1/chats/{id}/messages");
    for text in ["", "   "] {
        let r = d.http.post(&path, json!({"text": text})).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert_eq!(r.json()["fields"][0]["path"], "text");
    }
    let long = "x".repeat(8001);
    assert_eq!(
        d.http.post(&path, json!({"text": long})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        d.http.post(&path, json!({})).await.status,
        StatusCode::BAD_REQUEST
    );
    let nowhere = d
        .http
        .post("/v1/chats/chat_nope/messages", json!({"text": "hi"}))
        .await;
    assert_eq!(nowhere.status, StatusCode::NOT_FOUND);
    let bad = d
        .http
        .post("/v1/chats", json!({"repo": "not a repo"}))
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        d.http.get("/v1/chats/chat_nope/stream").await.status,
        StatusCode::NOT_FOUND
    );
    // nothing has been sent without the owner's token
    let no_auth = Http {
        token: None,
        ..d.http.clone()
    };
    assert_eq!(
        no_auth.post("/v1/chats", json!({})).await.status,
        StatusCode::UNAUTHORIZED
    );
    d.stop().await;
}

#[tokio::test]
async fn history_pages_by_seq() {
    let d = daemon().await;
    let id = create(&d, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sse = opened_stream(&d, &id).await;
    for t in ["one", "two"] {
        say(&d, &id, t).await;
        turn(&mut sse).await;
    }
    let all = d.http.get_json(&format!("/v1/chats/{id}")).await;
    let total = all["messages"].as_array().unwrap().len();
    assert!(total >= 4);
    let page = d
        .http
        .get_json(&format!("/v1/chats/{id}?after=1&limit=2"))
        .await;
    assert_eq!(page["messages"].as_array().unwrap().len(), 2);
    assert_eq!(page["messages"][0]["seq"], 2);
    assert_eq!(page["hasMore"], true);
    let rest = d
        .http
        .get_json(&format!("/v1/chats/{id}?after=1&limit=1000"))
        .await;
    assert_eq!(rest["hasMore"], false);
    d.stop().await;
}

async fn wait_idle(svc: &Arc<ratchetd::chat::ChatService>, id: &str) -> Value {
    let end = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let chat = svc.get_chat(id, &Default::default()).await.unwrap();
        let chat = serde_json::to_value(chat).unwrap();
        if chat["state"] == "idle" && chat["messages"].as_array().unwrap().len() >= 2 {
            return chat;
        }
        assert!(
            tokio::time::Instant::now() < end,
            "the chat never went idle"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_new_service_on_the_same_ledger_continues_the_chat_with_resume() {
    // the second service stands for a daemon restart: it knows the chat only from the ledger
    let ledger = ratchet_ledger::Ledger::open_memory(Default::default())
        .await
        .unwrap();
    let engine = ScriptedChatEngine::new(Default::default());
    let mcp = Arc::new(ratchetd::mcp::McpHub::new());
    let make = || {
        ratchetd::chat::ChatService::new(ledger.clone(), engine.clone(), mcp.clone(), None, quick())
    };
    let first = make();
    let chat = first
        .create_chat(ratchetd::api::CreateChatInput::default())
        .await
        .unwrap();
    first
        .send_message(
            &chat.id,
            ratchetd::api::SendChatMessageInput {
                text: "hello".into(),
            },
        )
        .await
        .unwrap();
    wait_idle(&first, &chat.id).await;
    first.shutdown().await;
    assert_eq!(
        engine.closed(),
        ["scripted-1"],
        "shutting down closes the session"
    );

    let second = make();
    let seen = wait_idle(&second, &chat.id).await;
    assert_eq!(seen["harness"], "claude-code");
    assert_eq!(seen["live"], false);
    assert_eq!(seen["messages"][0]["text"], "hello");
    second
        .send_message(
            &chat.id,
            ratchetd::api::SendChatMessageInput {
                text: "and again".into(),
            },
        )
        .await
        .unwrap();
    let end = tokio::time::Instant::now() + Duration::from_secs(5);
    while engine.opened().len() < 2 {
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let opened = engine.opened();
    assert_eq!(opened[1].spec.resume.as_deref(), Some("scripted-1"));
    second.shutdown().await;
}
