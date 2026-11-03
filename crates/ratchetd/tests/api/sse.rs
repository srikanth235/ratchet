//! The event stream: hello first, typed events with increasing ids, `Last-Event-ID` resume, resync.

use crate::common;

use common::{Sse, SseReader, TestDaemon};
use ratchetd::api::ApiEvent;
use ratchetd::sim::SimOptions;
use serde_json::json;

fn manual() -> SimOptions {
    SimOptions {
        auto_run: false,
        github_flow_ms: 0,
        ..SimOptions::default()
    }
}

/// Create and start a demo optimization, then run `ticks` turns.
async fn run_demo(d: &TestDaemon, ticks: usize) -> String {
    let r = d
        .http
        .post(
            "/v1/optimizations",
            json!({"demo": "demo-search", "agents": ["claude-code", "codex"], "budgetUsd": 50,
                   "timeLimitSeconds": 3600, "plateauEvals": 50}),
        )
        .await;
    let id = r.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(
        d.http
            .post(&format!("/v1/optimizations/{id}/start"), json!(null))
            .await
            .status,
        204
    );
    for _ in 0..ticks {
        d.sim.tick(&id).await.unwrap();
    }
    id
}

async fn take(sse: &mut SseReader, n: usize) -> Vec<Sse> {
    let mut out = Vec::new();
    while out.len() < n {
        out.push(sse.next().await.expect("stream ended early"));
    }
    out
}

#[tokio::test]
async fn hello_comes_first_and_events_follow_in_order() {
    let d = TestDaemon::start().await;
    let mut sse = d.http.events(None).await;
    let hello = sse.next().await.unwrap();
    assert_eq!(hello.event.as_deref(), Some("hello"));
    assert_eq!(
        hello.id, None,
        "hello has no id: it is not part of the sequence"
    );
    let last_before = hello.data["lastEventId"].as_u64().unwrap();
    assert_eq!(hello.data["type"], "hello");
    assert_eq!(hello.data["version"], ratchetd::VERSION);

    let id = run_demo(&d, 3).await;
    // read the burst; every message must parse as an ApiEvent and carry the next id
    let mut seen: Vec<Sse> = Vec::new();
    let mut expected = last_before + 1;
    while seen
        .iter()
        .filter(|e| e.event.as_deref() == Some("candidate"))
        .count()
        < 3
    {
        let m = sse.next().await.unwrap();
        assert_eq!(m.id, Some(expected), "ids count up without gaps: {m:?}");
        expected += 1;
        let parsed: ApiEvent =
            serde_json::from_value(m.data.clone()).unwrap_or_else(|e| panic!("{e}: {m:?}"));
        assert_eq!(
            parsed.name(),
            m.event.as_deref().unwrap(),
            "the SSE event name is the type tag"
        );
        seen.push(m);
    }
    let names: Vec<&str> = seen.iter().map(|m| m.event.as_deref().unwrap()).collect();
    for want in [
        "sandbox",
        "optimization",
        "run-active",
        "agent-event",
        "candidate",
        "spend",
    ] {
        assert!(names.contains(&want), "no {want} event in {names:?}");
    }
    // the baseline phase comes before the first candidate
    let first_cand = names.iter().position(|n| *n == "candidate").unwrap();
    let running = seen
        .iter()
        .position(|m| m.event.as_deref() == Some("optimization") && m.data["status"] == "running")
        .unwrap();
    assert!(running < first_cand);
    // camelCase payloads
    let cand = seen
        .iter()
        .find(|m| m.event.as_deref() == Some("candidate"))
        .unwrap();
    assert_eq!(cand.data["optimizationId"], id.as_str());
    assert!(
        cand.data["candidateId"]
            .as_str()
            .unwrap()
            .starts_with("cand_")
    );
    assert!(
        ["verified", "rejected", "failed_confirm", "blocked"]
            .contains(&cand.data["verdict"].as_str().unwrap())
    );
    let ra = seen
        .iter()
        .find(|m| m.event.as_deref() == Some("run-active"))
        .unwrap();
    assert_eq!(ra.data["active"], true);
    assert!(
        ra.data["optimizationIds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == id.as_str())
    );
    // an agent event carries the log line
    let ae = seen
        .iter()
        .find(|m| m.event.as_deref() == Some("agent-event"))
        .unwrap();
    assert!(ae.data["event"]["text"].is_string());
    d.stop().await;
}

#[tokio::test]
async fn last_event_id_resumes_without_gap_or_duplicate() {
    let d = TestDaemon::start().await;
    let mut a = d.http.events(None).await;
    a.next().await.unwrap(); // hello
    let id = run_demo(&d, 2).await;
    // read a first batch, remember where we were
    let first = take(&mut a, 5).await;
    let cut = first.last().unwrap().id.unwrap();
    drop(a);

    // more happens while we are away
    for _ in 0..3 {
        d.sim.tick(&id).await.unwrap();
    }
    let newest = d.daemon.hub.last_id();
    assert!(newest > cut + 3);

    let mut b = d.http.events(Some(cut)).await;
    let hello = b.next().await.unwrap();
    assert_eq!(hello.event.as_deref(), Some("hello"));
    assert_eq!(hello.data["lastEventId"].as_u64().unwrap(), newest);
    let replay = take(&mut b, (newest - cut) as usize).await;
    let ids: Vec<u64> = replay.iter().map(|m| m.id.unwrap()).collect();
    assert_eq!(
        ids,
        (cut + 1..=newest).collect::<Vec<_>>(),
        "exactly what was missed, in order"
    );
    assert!(replay.iter().all(|m| m.event.as_deref() != Some("resync")));

    // and the live stream continues from there
    d.sim.tick(&id).await.unwrap();
    let live = b.next().await.unwrap();
    assert_eq!(live.id, Some(newest + 1));
    d.stop().await;
}

#[tokio::test]
async fn too_old_or_too_new_means_resync() {
    let d = TestDaemon::with_window(manual(), 4).await;
    run_demo(&d, 3).await; // far more than 4 events
    let newest = d.daemon.hub.last_id();
    assert!(newest > 10);

    // older than the window: hello, resync, then live
    let mut old = d.http.events(Some(1)).await;
    assert_eq!(old.next().await.unwrap().event.as_deref(), Some("hello"));
    let resync = old.next().await.unwrap();
    assert_eq!(resync.event.as_deref(), Some("resync"));
    assert_eq!(resync.id, None);
    assert!(
        resync.data["reason"]
            .as_str()
            .unwrap()
            .contains("no longer kept")
    );

    // from before a restart: ahead of the numbering
    let mut ahead = d.http.events(Some(newest + 1000)).await;
    ahead.next().await.unwrap();
    let r = ahead.next().await.unwrap();
    assert_eq!(r.event.as_deref(), Some("resync"));
    assert!(r.data["reason"].as_str().unwrap().contains("restarted"));

    // inside the window: replay, no resync
    let mut ok = d.http.events(Some(newest - 2)).await;
    ok.next().await.unwrap();
    let m = ok.next().await.unwrap();
    assert_eq!(m.id, Some(newest - 1));
    d.stop().await;
}

#[tokio::test]
async fn the_status_route_tells_a_fresh_client_where_to_resume() {
    let d = TestDaemon::start().await;
    run_demo(&d, 1).await;
    let st = d.http.get_json("/v1/status").await;
    assert_eq!(st["lastEventId"].as_u64().unwrap(), d.daemon.hub.last_id());
    // a client that fetched a snapshot and then resumes from that id gets only newer events
    let from = st["lastEventId"].as_u64().unwrap();
    let mut sse = d.http.events(Some(from)).await;
    sse.next().await.unwrap();
    d.http
        .post("/v1/agents/opt_demo_search_agt_0/pause", json!(null))
        .await;
    let m = sse.next().await.unwrap();
    assert_eq!(m.id, Some(from + 1));
    assert_eq!(m.event.as_deref(), Some("agent-event"));
    d.stop().await;
}

#[tokio::test]
async fn github_and_connector_events_are_published() {
    let d = TestDaemon::start().await;
    let mut sse = d.http.events(None).await;
    sse.next().await.unwrap();
    d.http.send("DELETE", "/v1/github/session", None).await;
    let m = sse.until("github").await;
    assert_eq!(m.data, json!({"type": "github", "signedIn": false}));
    d.http.post("/v1/github/sign-in", json!(null)).await;
    d.sim.complete_github_flow();
    let m = sse.until("github").await;
    assert_eq!(m.data["signedIn"], true);
    let list = d.http.get("/v1/connectors").await.json();
    let staging = list["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "acme-staging")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    d.http
        .post(&format!("/v1/connectors/{staging}/test"), json!(null))
        .await;
    let m = sse.until("connector").await;
    assert_eq!(m.data["connectorId"], staging.as_str());
    assert_eq!(m.data["status"], "expired");
    d.stop().await;
}
