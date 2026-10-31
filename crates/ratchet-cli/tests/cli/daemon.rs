//! The commands that talk to the daemon, run against an in-process `ratchetd` on a temp socket.

use ratchetd::api::*;
use ratchetd::sim::SimOptions;

use crate::common::{Env, ratchet};

fn parse<T: serde::de::DeserializeOwned>(o: &crate::common::Out) -> T {
    serde_json::from_value(o.json()).unwrap_or_else(|e| panic!("{e}\n{}", o.stdout))
}

#[tokio::test(flavor = "multi_thread")]
async fn status_shows_the_daemon_and_the_optimizations() {
    let env = Env::start().await;
    let o = env.ratchet(&["status"]).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stdout
            .contains("ratchetd 0.0.0 (simulation engine, simulation mode)"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains(&env.socket()));
    assert!(o.stdout.contains("ledger: in memory"));
    assert!(o.stdout.contains("4 optimizations, 1 running"));
    assert!(o.stdout.contains("opt_demo_search") && o.stdout.contains("search-api p95 latency"));
    assert!(o.stdout.contains("182 -> 131 ms (28.0%)"), "{}", o.stdout);

    let j = env.ratchet(&["status", "--json"]).await.json();
    assert_eq!(j["daemon"]["apiVersion"], "v1");
    assert_eq!(j["daemon"]["engine"]["kind"], "sim");
    assert_eq!(j["optimizations"].as_array().unwrap().len(), 4);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_show_and_the_per_run_reads() {
    let env = Env::start().await;
    let o = env.ratchet(&["list", "--range", "90d"]).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.starts_with("ID"));
    assert!(o.stdout.contains("Verified wins: 8"), "{}", o.stdout);
    let alias = env.ratchet(&["ls", "--json"]).await;
    let snap: OptimizationsSnapshot = parse(&alias);
    assert_eq!(snap.range, KpiRange::Days30);
    assert_eq!(snap.optimizations.len(), 4);

    let show = env.ratchet(&["show", "opt_demo_search"]).await;
    assert_eq!(show.code, 0, "{}", show.stderr);
    for want in [
        "search-api p95 latency  (opt_demo_search)",
        "acme/search-api (main)",
        "lower is better: http_req_duration.p95",
        "182 -> 131 ms (28.0%)",
        "5 verified",
        "Baseline vs best",
        "opt_demo_search_agt_0",
        "claude-code",
    ] {
        assert!(
            show.stdout.contains(want),
            "missing {want:?}:\n{}",
            show.stdout
        );
    }
    let d: OptimizationDetail = parse(&env.ratchet(&["show", "opt_demo_search", "--json"]).await);
    assert_eq!(d.agents.len(), 2);

    let c = env.ratchet(&["candidates", "opt_demo_search"]).await;
    assert!(
        c.stdout.contains("failed_confirm") && c.stdout.contains("blocked"),
        "{}",
        c.stdout
    );
    let cj: Vec<CandidateVm> = parse(
        &env.ratchet(&["candidates", "opt_demo_search", "--json"])
            .await,
    );
    assert_eq!(cj.len(), 11);

    let h = env.ratchet(&["history", "opt_demo_search"]).await;
    assert!(
        h.stdout.contains("baseline") && h.stdout.contains("win 5"),
        "{}",
        h.stdout
    );
    let hj: RatchetSeries = parse(
        &env.ratchet(&[
            "history",
            "opt_demo_search",
            "--metric",
            "http_req_duration.p99",
            "--json",
        ])
        .await,
    );
    assert_eq!(hj.metric, "http_req_duration.p99");

    let s = env.ratchet(&["spend", "opt_demo_search"]).await;
    assert!(
        s.stdout.contains("Model tokens") && s.stdout.contains("of $120.00 budget"),
        "{}",
        s.stdout
    );
    let a = env
        .ratchet(&[
            "activity",
            "--optimization",
            "opt_demo_ingest",
            "--limit",
            "3",
        ])
        .await;
    assert_eq!(a.stdout.lines().count(), 3, "{}", a.stdout);
    assert!(a.stdout.contains("Verified win"));
    let aj: Vec<ActivityVm> = parse(&env.ratchet(&["activity", "--json"]).await);
    assert_eq!(aj.len(), 20.min(aj.len()));
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn run_creates_starts_and_stop_ends() {
    let env = Env::start().await;
    let created = env
        .ratchet(&[
            "run",
            "--demo",
            "demo-search",
            "--no-start",
            "--budget",
            "20",
            "--name",
            "my run",
        ])
        .await;
    assert_eq!(created.code, 0, "{}", created.stderr);
    let id = created.stdout.split_whitespace().next().unwrap().to_owned();
    assert!(created.stdout.contains("(created, not started)"));
    let d: OptimizationDetail = parse(&env.ratchet(&["show", &id, "--json"]).await);
    assert_eq!(
        (
            d.summary.name.as_str(),
            d.summary.status,
            d.summary.budget_usd
        ),
        ("my run", UiStatus::Draft, 20.0)
    );

    let started = env
        .ratchet(&[
            "run",
            "--demo",
            "demo-cli",
            "--agent",
            "claude-code,codex",
            "--time-limit",
            "90m",
            "--json",
        ])
        .await;
    assert_eq!(started.code, 0, "{}", started.stderr);
    let v = started.json();
    assert_eq!(v["started"], true);
    let id2 = v["id"].as_str().unwrap().to_owned();
    env.sim.tick(&id2).await.unwrap();
    let d: OptimizationDetail = parse(&env.ratchet(&["show", &id2, "--json"]).await);
    assert_eq!(d.summary.status, UiStatus::Running);
    assert_eq!(d.agents.len(), 2);
    assert_eq!(d.time_limit_s, 5400);

    let stop = env.ratchet(&["stop", &id2, "--reason", "enough"]).await;
    assert_eq!(stop.code, 0, "{}", stop.stderr);
    assert_eq!(stop.stdout, format!("stopped {id2}\n"));
    let d: OptimizationDetail = parse(&env.ratchet(&["show", &id2, "--json"]).await);
    assert_eq!(d.summary.status, UiStatus::Stopped);
    assert_eq!(
        env.ratchet(&["stop", &id2, "--json"]).await.json()["stopped"],
        true
    );
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn run_needs_a_target_and_reports_the_daemons_complaints() {
    let env = Env::start().await;
    let o = env.ratchet(&["run"]).await;
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("needs a repo"));
    // a repo run without GitHub is refused with the reason
    env.ratchet(&["run", "--demo", "demo-search", "--no-start"])
        .await; // warm the sim
    env.sim.complete_github_flow();
    let off = ratchet(&[
        "--socket",
        &env.socket(),
        "run",
        "acme/search-api",
        "--budget",
        "-1",
    ])
    .await;
    assert_eq!(off.code, 1);
    assert!(
        off.stderr
            .contains("ratchet: the optimization request is invalid"),
        "{}",
        off.stderr
    );
    assert!(
        off.stderr.contains("- budgetUsd:"),
        "per-field problems are listed:\n{}",
        off.stderr
    );
    let bad = env
        .ratchet(&["run", "--demo", "demo-search", "--budget", "-1", "--json"])
        .await;
    assert_eq!(bad.code, 1);
    let e: serde_json::Value = serde_json::from_str(&bad.stderr).unwrap();
    assert_eq!(e["error"]["code"], "invalid_request");
    assert_eq!(e["error"]["fields"][0]["path"], "budgetUsd");
    assert_eq!(bad.stdout, "", "errors go to stderr");
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn run_wait_follows_the_events_until_the_run_ends() {
    let env = Env::with(SimOptions {
        auto_run: true,
        tick_ms: 10,
        baseline_ms: 10,
        github_flow_ms: 0,
        resume_runs: false,
        ..SimOptions::default()
    })
    .await;
    let o = env
        .ratchet(&[
            "run",
            "--demo",
            "demo-search",
            "--budget",
            "4",
            "--wait",
            "--wait-timeout",
            "30s",
        ])
        .await;
    assert_eq!(o.code, 0, "{}\n{}", o.stdout, o.stderr);
    assert!(
        o.stdout.contains(": baselining") || o.stdout.contains(": running"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains(": completed"), "{}", o.stdout);
    let last = o.stdout.lines().last().unwrap();
    assert!(
        last.contains("completed \u{b7}") && last.contains(" spent"),
        "{last}"
    );

    let j = env
        .ratchet(&[
            "run",
            "--demo",
            "demo-search",
            "--budget",
            "4",
            "--wait",
            "--json",
        ])
        .await;
    assert_eq!(j.code, 0, "{}", j.stderr);
    let v = j.json();
    assert_eq!(v["status"], "completed");
    assert_eq!(v["timedOut"], false);
    assert!(v["optimization"]["spendUsd"].as_f64().unwrap() >= 4.0);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn run_wait_gives_up_after_its_timeout_with_exit_1() {
    let env = Env::start().await; // nothing ticks: the run never ends
    let t = std::time::Instant::now();
    let o = env
        .ratchet(&[
            "run",
            "--demo",
            "demo-search",
            "--wait",
            "--wait-timeout",
            "1s",
        ])
        .await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("gave up waiting"), "{}", o.stderr);
    assert!(t.elapsed().as_secs() < 8);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn report_pr_and_cost() {
    let env = Env::start().await;
    let r = env.ratchet(&["report", "opt_demo_ingest"]).await;
    assert_eq!(r.code, 0);
    assert!(
        r.stdout.starts_with("# Ratchet report: Ingest throughput"),
        "{}",
        r.stdout
    );
    let out = env.dir.path().join("report.md");
    let w = env
        .ratchet(&["report", "opt_demo_ingest", "--out", out.to_str().unwrap()])
        .await;
    assert_eq!(w.stdout, format!("wrote {}\n", out.display()));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), r.stdout);
    let rj: ReportVm = parse(&env.ratchet(&["report", "opt_demo_ingest", "--json"]).await);
    assert_eq!(rj.markdown, r.stdout);

    let pr = env.ratchet(&["pr", "opt_demo_ingest"]).await;
    assert_eq!(pr.code, 0, "{}", pr.stderr);
    assert!(
        pr.stdout
            .starts_with("https://github.com/acme/ingest/pull/")
    );
    let show = env.ratchet(&["show", "opt_demo_ingest"]).await;
    assert!(show.stdout.contains(pr.stdout.trim()), "{}", show.stdout);
    let nope = env.ratchet(&["pr", "opt_demo_web"]).await;
    assert_eq!(nope.code, 1);
    assert!(nope.stderr.contains("no verified wins"), "{}", nope.stderr);

    // cost reconcile (ported from cost.test.ts, now through the daemon that owns the ledger)
    let first = env.ratchet(&["cost", "reconcile", "opt_demo_edge"]).await;
    assert_eq!(first.code, 0, "{}", first.stderr);
    assert!(first.stdout.contains("reconciled: "), "{}", first.stdout);
    assert!(first.stdout.contains("correction row(s) written"));
    assert!(
        first
            .stdout
            .lines()
            .any(|l| l.trim_start().starts_with("agent_runner") && l.ends_with("written")),
        "{}",
        first.stdout
    );
    let again = env
        .ratchet(&["cost", "reconcile", "opt_demo_edge", "--json"])
        .await;
    let v = again.json();
    assert_eq!(v["status"], "unchanged");
    assert_eq!(v["writtenUsd"], 0.0);
    let running = env.ratchet(&["cost", "reconcile", "opt_demo_search"]).await;
    assert_eq!(running.code, 0);
    assert!(
        running.stdout.contains("pending: nothing was written"),
        "{}",
        running.stdout
    );
    let unknown = env.ratchet(&["cost", "reconcile", "opt-zzz"]).await;
    assert_eq!(unknown.code, 1);
    assert!(unknown.stderr.contains("not found"), "{}", unknown.stderr);
    let help = ratchet(&["cost", "reconcile", "--help"]).await;
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("Idempotent"));
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_pause_resume_message_and_log() {
    let env = Env::start().await;
    let a = "opt_demo_search_agt_0";
    assert_eq!(
        env.ratchet(&["agent", "pause", a]).await.stdout,
        format!("paused {a}\n")
    );
    assert_eq!(
        env.ratchet(&["agent", "resume", a]).await.stdout,
        format!("resumed {a}\n")
    );
    let m = env
        .ratchet(&["agent", "message", a, "look at the parser"])
        .await;
    assert_eq!(m.code, 0, "{}", m.stderr);
    let log = env.ratchet(&["agent", "log", a, "--limit", "3"]).await;
    assert_eq!(log.stdout.lines().count(), 3, "{}", log.stdout);
    assert!(log.stdout.contains("You: look at the parser"));
    let page: AgentEventsPage = parse(&env.ratchet(&["agent", "log", a, "--json"]).await);
    let after = env
        .ratchet(&[
            "agent",
            "log",
            a,
            "--after",
            &page.cursor.to_string(),
            "--json",
        ])
        .await;
    assert!(parse::<AgentEventsPage>(&after).events.is_empty());
    let bad = env.ratchet(&["agent", "pause", "nope"]).await;
    assert_eq!(bad.code, 1);
    assert!(
        bad.stderr.contains("agent nope not found"),
        "{}",
        bad.stderr
    );
    let empty = env.ratchet(&["agent", "message", a, "   "]).await;
    assert_eq!(empty.code, 1);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn events_streams_json_lines_and_resumes() {
    let env = Env::start().await;
    let from = env.daemon.hub.last_id();
    env.ratchet(&["agent", "pause", "opt_demo_search_agt_0"])
        .await;
    env.ratchet(&["agent", "resume", "opt_demo_search_agt_0"])
        .await;
    let o = env
        .ratchet(&[
            "events",
            "--last-event-id",
            &from.to_string(),
            "--count",
            "2",
            "--json",
        ])
        .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    let lines: Vec<serde_json::Value> = o
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["type"], "hello");
    assert!(lines[0].get("id").is_none());
    assert_eq!(lines.len(), 3, "hello and the two events");
    assert_eq!(lines[1]["id"], from + 1);
    assert_eq!(lines[1]["type"], "agent-event");
    assert_eq!(lines[1]["event"]["text"], "Paused by user");
    assert_eq!(lines[2]["id"], from + 2);

    let human = env
        .ratchet(&[
            "events",
            "--last-event-id",
            &from.to_string(),
            "--count",
            "1",
        ])
        .await;
    assert!(
        human.stdout.contains("#") && human.stdout.contains("agent-event"),
        "{}",
        human.stdout
    );
    // a client that is too far behind is told to resync, and it does not count as an event
    let stale = env
        .ratchet(&[
            "events",
            "--last-event-id",
            "999999",
            "--timeout",
            "1s",
            "--json",
        ])
        .await;
    assert_eq!(stale.code, 0);
    assert!(
        stale
            .stdout
            .lines()
            .any(|l| l.contains("\"type\":\"resync\"")),
        "{}",
        stale.stdout
    );
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn environments_list_and_end() {
    let env = Env::start().await;
    let o = env.ratchet(&["env", "list"]).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stdout.contains("sbx-sim-1") && o.stdout.contains("opt_demo_search"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("last teardown:"));
    let busy = env.ratchet(&["env", "end", "sbx-sim-1"]).await;
    assert_eq!(busy.code, 1);
    assert!(
        busy.stderr.contains("a run is still active"),
        "{}",
        busy.stderr
    );
    env.ratchet(&["stop", "opt_demo_search"]).await;
    let ended = env.ratchet(&["env", "end", "sbx-sim-1"]).await;
    assert_eq!(ended.code, 0, "{}", ended.stderr);
    assert!(ended.stdout.starts_with("ended sbx-sim-1\n"));
    assert!(
        ended.stdout.contains("nothing tagged is left running"),
        "{}",
        ended.stdout
    );
    let j: EnvironmentsVm = parse(&env.ratchet(&["env", "list", "--json"]).await);
    assert!(j.environments.is_empty());
    assert_eq!(
        env.ratchet(&["env", "list"]).await.stdout.lines().next(),
        Some("no environment is running")
    );
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn repos_inspect_overview_and_connectors() {
    let env = Env::start().await;
    let r = env.ratchet(&["repos", "shop"]).await;
    assert!(
        r.stdout.contains("acme-org/shop-api") && !r.stdout.contains("acme-cli"),
        "{}",
        r.stdout
    );
    let rj: Vec<RepoVm> = parse(&env.ratchet(&["repos", "--json"]).await);
    assert!(rj.len() >= 5);

    let i = env.ratchet(&["inspect", "acme-org/shop-api"]).await;
    assert_eq!(i.code, 0, "{}", i.stderr);
    assert!(
        i.stdout.contains("no ratchet.yaml") && i.stdout.contains("http-latency"),
        "{}",
        i.stdout
    );
    let i = env.ratchet(&["inspect", "acme/search-api"]).await;
    assert!(
        i.stdout.contains("minimize http_req_duration.p95"),
        "{}",
        i.stdout
    );
    let local = tempfile::tempdir().unwrap();
    let i = env
        .ratchet(&["inspect", local.path().to_str().unwrap()])
        .await;
    assert_eq!(i.code, 1);
    assert!(i.stdout.contains("not a git repository"), "{}", i.stdout);
    let unknown = env.ratchet(&["inspect", "acme/unknown"]).await;
    assert_eq!(unknown.code, 1);
    assert!(unknown.stderr.contains("not found"));

    let ov = env.ratchet(&["overview"]).await;
    assert_eq!(ov.code, 0, "{}", ov.stderr);
    assert!(
        ov.stdout.contains("value locked in") && ov.stdout.contains("Needs you"),
        "{}",
        ov.stdout
    );
    let ovj: OverviewVm = parse(&env.ratchet(&["overview", "--range", "all", "--json"]).await);
    assert_eq!(ovj.range, OverviewRange::All);

    let c = env.ratchet(&["connectors"]).await;
    assert!(
        c.stdout.contains("acme-prod") && c.stdout.contains("expired"),
        "{}",
        c.stdout
    );
    let cj: ConnectorsVm = parse(&env.ratchet(&["connectors", "--json"]).await);
    assert_eq!(cj.connectors.len(), 7);
    assert!(cj.encryption_available);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_problems_are_explained() {
    // no daemon: the socket does not exist
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("ratchetd.sock");
    std::fs::write(dir.path().join("ratchetd.token"), "abc").unwrap();
    let o = ratchet(&["--socket", sock.to_str().unwrap(), "status"]).await;
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("cannot reach ratchetd at"),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("is the daemon running"));
    let j = ratchet(&["--socket", sock.to_str().unwrap(), "status", "--json"]).await;
    let e: serde_json::Value = serde_json::from_str(&j.stderr).unwrap();
    assert_eq!(e["error"]["code"], "unreachable");

    // a daemon, but no token to be found
    let env = Env::start().await;
    let nowhere = tempfile::tempdir().unwrap();
    let o = ratchet(&[
        "--socket",
        &env.socket(),
        "--token-file",
        nowhere.path().join("t").to_str().unwrap(),
        "status",
    ])
    .await;
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("cannot read the daemon's token"),
        "{}",
        o.stderr
    );

    // the wrong token
    let o = ratchet(&["--socket", &env.socket(), "--token", "wrong", "status"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("wrong bearer token"), "{}", o.stderr);
    let o = ratchet(&[
        "--socket",
        &env.socket(),
        "--token",
        "wrong",
        "status",
        "--json",
    ])
    .await;
    let e: serde_json::Value = serde_json::from_str(&o.stderr).unwrap();
    assert_eq!(e["error"]["code"], "unauthorized");

    // the right token given explicitly, or by file
    let token = env.daemon.token.clone();
    let o = ratchet(&["--socket", &env.socket(), "--token", &token, "status"]).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    let tf = env.dir.path().join("ratchetd.token");
    let o = ratchet(&[
        "--socket",
        &env.socket(),
        "--token-file",
        tf.to_str().unwrap(),
        "status",
    ])
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    env.stop().await;
}
