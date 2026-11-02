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
            "--yes",
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
            "--yes",
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

    let stop = env
        .ratchet(&["stop", &id2, "--reason", "enough", "--yes"])
        .await;
    assert_eq!(stop.code, 0, "{}", stop.stderr);
    assert_eq!(stop.stdout, format!("stopped {id2}\n"));
    let d: OptimizationDetail = parse(&env.ratchet(&["show", &id2, "--json"]).await);
    assert_eq!(d.summary.status, UiStatus::Stopped);
    assert_eq!(
        env.ratchet(&["stop", &id2, "--json", "--yes"]).await.json()["stopped"],
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
    env.ratchet(&["run", "--yes", "--demo", "demo-search", "--no-start"])
        .await; // warm the sim
    env.sim.complete_github_flow();
    let off = ratchet(&[
        "--socket",
        &env.socket(),
        "run",
        "--yes",
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
        .ratchet(&[
            "run",
            "--yes",
            "--demo",
            "demo-search",
            "--budget",
            "-1",
            "--json",
        ])
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
            "--yes",
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
            "--yes",
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
            "--yes",
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

    let pr = env.ratchet(&["pr", "opt_demo_ingest", "--yes"]).await;
    assert_eq!(pr.code, 0, "{}", pr.stderr);
    assert!(
        pr.stdout
            .starts_with("https://github.com/acme/ingest/pull/")
    );
    let show = env.ratchet(&["show", "opt_demo_ingest"]).await;
    assert!(show.stdout.contains(pr.stdout.trim()), "{}", show.stdout);
    let nope = env.ratchet(&["pr", "opt_demo_web", "--yes"]).await;
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
    let busy = env.ratchet(&["env", "end", "sbx-sim-1", "--yes"]).await;
    assert_eq!(busy.code, 1);
    assert!(
        busy.stderr.contains("a run is still active"),
        "{}",
        busy.stderr
    );
    env.ratchet(&["stop", "opt_demo_search", "--yes"]).await;
    let ended = env.ratchet(&["env", "end", "sbx-sim-1", "--yes"]).await;
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
        ov.stdout.contains("value locked in")
            && ov.stdout.contains("Needs you")
            && ov
                .stdout
                .contains("Open a pull request for 2 verified wins")
            && ov.stdout.contains("Ready to ship"),
        "{}",
        ov.stdout
    );
    let ovj: OverviewVm = parse(&env.ratchet(&["overview", "--range", "all", "--json"]).await);
    assert_eq!(ovj.range, OverviewRange::All);
    assert!(!ovj.empty && ovj.value.is_some() && !ovj.needs_you.is_empty());
    assert_eq!(ovj.rows.len(), 4);

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

/// The `--yes` contract: `run`, `stop`, `env end` and `pr` only preview without it, exit 2, and change nothing.
#[tokio::test(flavor = "multi_thread")]
async fn destructive_commands_preview_and_exit_2_without_yes() {
    let env = Env::start().await;

    let run = env
        .ratchet(&[
            "run",
            "--demo",
            "demo-search",
            "--agent",
            "claude-code,codex",
            "--budget",
            "20",
            "--where",
            "local",
        ])
        .await;
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(
        run.stdout.is_empty(),
        "preview goes to stderr: {}",
        run.stdout
    );
    for want in [
        "demo-search",
        "claude-code, codex",
        "up to $20.00",
        "local",
        "re-run with --yes",
        "Nothing was changed",
    ] {
        assert!(
            run.stderr.contains(want),
            "missing {want:?}:\n{}",
            run.stderr
        );
    }

    let cases: [(&[&str], &str); 3] = [
        (&["stop", "opt_demo_search"], "Would stop opt_demo_search"),
        (&["pr", "opt_demo_ingest"], "pull request"),
        (
            &["env", "end", "sbx-sim-1"],
            "tear down the environment sbx-sim-1",
        ),
    ];
    for (args, want) in cases {
        let o = env.ratchet(args).await;
        assert_eq!(o.code, 2, "{args:?}: {}", o.stderr);
        assert!(o.stderr.contains(want), "{args:?}:\n{}", o.stderr);
        assert!(o.stderr.contains("re-run with --yes"), "{}", o.stderr);
    }

    // --json: one object on stdout, a stable error code, and the preview
    let j = env.ratchet(&["stop", "opt_demo_search", "--json"]).await;
    assert_eq!(j.code, 2);
    let v = j.json();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "confirmation_required");
    assert_eq!(v["changed"], false);
    assert_eq!(v["preview"]["optimization"], "opt_demo_search");
    assert!(j.stderr.is_empty(), "{}", j.stderr);

    // nothing happened, and the optimization is still running
    let d: OptimizationDetail = parse(&env.ratchet(&["show", "opt_demo_search", "--json"]).await);
    assert_eq!(d.summary.status, UiStatus::Running);

    // -y is the short form, and --yes works before the subcommand too
    let stopped = env.ratchet(&["-y", "stop", "opt_demo_search"]).await;
    assert_eq!(stopped.code, 0, "{}", stopped.stderr);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_preview_needs_no_daemon() {
    // an unreachable socket: the refusal comes first, so it is still exit 2 rather than "cannot connect"
    let o = ratchet(&["--socket", "/nonexistent/ratchetd.sock", "stop", "opt_x"]).await;
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("re-run with --yes"));
    // with --yes it does try, and fails to connect (exit 1)
    let o = ratchet(&[
        "--socket",
        "/nonexistent/ratchetd.sock",
        "stop",
        "opt_x",
        "--yes",
    ])
    .await;
    assert_eq!(o.code, 1, "{}", o.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_talks_to_the_scripted_assistant_and_ends() {
    let env = Env::start().await;
    let made = env.ratchet(&["chat", "new", "--repo", "acme/shop"]).await;
    assert_eq!(made.code, 0, "{}", made.stderr);
    assert!(
        made.stdout.contains("Claude Code · sonnet"),
        "{}",
        made.stdout
    );
    assert!(made.stderr.starts_with("First time:"), "{}", made.stderr);
    let id = made.stdout.split_whitespace().next().unwrap().to_owned();

    // a reply streams as text; steps and the card go to stderr
    let sent = env.ratchet(&["chat", "send", &id, "hello"]).await;
    assert_eq!(sent.code, 0, "{}", sent.stderr);
    assert!(
        sent.stdout.contains("Here is the optimization")
            || sent.stdout.contains("Which repository"),
        "{}",
        sent.stdout
    );

    // --json: one stream event per line, ending with done
    let j = env
        .ratchet(&["chat", "send", &id, "[simulate:crash]", "--json"])
        .await;
    assert_eq!(j.code, 1, "a failed turn exits 1: {}", j.stdout);
    let events: Vec<serde_json::Value> = j
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let kinds: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert_eq!(&kinds[kinds.len() - 2..], ["error", "done"], "{kinds:?}");

    let shown = env.ratchet(&["chat", "show", &id]).await;
    assert_eq!(shown.code, 0);
    assert!(
        shown.stdout.contains("you") && shown.stdout.contains("hello"),
        "{}",
        shown.stdout
    );
    let chat: ChatVm = parse(&env.ratchet(&["chat", "show", &id, "--json"]).await);
    assert!(chat.messages.len() >= 3);

    assert_eq!(env.ratchet(&["chat", "stop", &id]).await.code, 0);
    let ended = env.ratchet(&["chat", "end", &id]).await;
    assert_eq!(ended.code, 0, "{}", ended.stderr);
    let gone = env.ratchet(&["chat", "show", &id]).await;
    assert_ne!(gone.code, 0);
    let bad = env.ratchet(&["chat", "new", "--harness", "gemini"]).await;
    assert_eq!(bad.code, 2);
    env.stop().await;
}

// ---- value model, schedule, home (P7) -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn the_value_model_round_trips_through_the_cli_and_the_daemon() {
    let env = Env::start().await;
    let id = "opt_demo_ingest";
    let before: ValueModelVm = parse(&env.ratchet(&["value", "show", id, "--json"]).await);
    assert!(before.set);

    // flags left out keep what is there: only the words change
    let o = env
        .ratchet(&[
            "value",
            "set",
            id,
            "--note",
            "events per second x pipeline value",
        ])
        .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stdout.contains("events per second x pipeline value"),
        "{}",
        o.stdout
    );
    let after: ValueModelVm = parse(&env.ratchet(&["value", "show", id, "--json"]).await);
    assert_eq!(
        (
            after.usd_per_unit,
            after.period,
            after.unit.clone(),
            after.basis
        ),
        (
            before.usd_per_unit,
            before.period,
            before.unit.clone(),
            before.basis
        )
    );
    assert_eq!(
        after.note.as_deref(),
        Some("events per second x pipeline value")
    );

    // every field, then read back through the API the app uses
    let o = env
        .ratchet(&[
            "value",
            "set",
            id,
            "--usd-per-unit",
            "750",
            "--period",
            "month",
            "--unit",
            "k/s",
            "--basis",
            "measured",
            "--note",
            "k/s x revenue per event",
            "--json",
        ])
        .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    let set: ValueModelVm = parse(&o);
    assert_eq!(
        set.summary,
        "$750 per k/s a month \u{b7} measured \u{b7} k/s x revenue per event"
    );
    let client = ratchet_cli::client::Client::new(env.socket(), env.daemon.token.clone());
    let via_api: serde_json::Value = client
        .get(&format!("/v1/optimizations/{id}/value"))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&set).unwrap(), via_api);

    // the overview re-values the wins with it
    let ov: OverviewVm = parse(&env.ratchet(&["overview", "--json"]).await);
    let row = ov.rows.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.value_model.as_ref(), Some(&set));

    // clearing needs --yes, like every command that removes something
    let refused = env.ratchet(&["value", "clear", id]).await;
    assert_eq!(refused.code, 2);
    assert!(refused.stderr.contains("--yes"), "{}", refused.stderr);
    assert!(parse::<ValueModelVm>(&env.ratchet(&["value", "show", id, "--json"]).await).set);
    let cleared: ValueModelVm = parse(
        &env.ratchet(&["value", "clear", id, "--yes", "--json"])
            .await,
    );
    assert!(!cleared.set);
    let shown = env.ratchet(&["value", "show", id]).await;
    assert!(shown.stdout.starts_with("Not set."), "{}", shown.stdout);

    // a first model needs its number, and says how to give it
    let missing = env.ratchet(&["value", "set", id, "--unit", "ms"]).await;
    assert_eq!(missing.code, 2);
    assert!(
        missing.stderr.contains("--usd-per-unit"),
        "{}",
        missing.stderr
    );
    let zero = env
        .ratchet(&["value", "set", id, "--usd-per-unit", "0"])
        .await;
    assert_eq!(zero.code, 1);
    assert!(zero.stderr.contains("greater than 0"), "{}", zero.stderr);
    let unknown = env.ratchet(&["value", "show", "nope"]).await;
    assert_eq!(unknown.code, 1);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn schedule_and_merge_commands_and_the_forecast_they_feed() {
    let env = Env::start().await;
    let id = "opt_demo_ingest";
    assert_eq!(
        env.ratchet(&["schedule", "show", id]).await.stdout,
        "Not scheduled\n"
    );
    let o = env
        .ratchet(&["schedule", "set", id, "--every", "nightly"])
        .await;
    assert_eq!(
        o.stdout, "Every night (30 runs in the next 30 days)\n",
        "{}",
        o.stderr
    );
    let ov: OverviewVm = parse(&env.ratchet(&["overview", "--json"]).await);
    assert_eq!(ov.forecast.scheduled_runs, 30);
    assert!(ov.forecast.expected_usd > 0.0);
    let j: ScheduleVm = parse(&env.ratchet(&["schedule", "clear", id, "--json"]).await);
    assert_eq!(j.schedule, None);
    assert_eq!(
        env.ratchet(&["schedule", "set", id, "--every", "hourly"])
            .await
            .code,
        2
    );

    // a merge needs a pull request first
    let none = env.ratchet(&["pr-merged", id]).await;
    assert_eq!(none.code, 1);
    assert!(none.stderr.contains("no pull request"), "{}", none.stderr);
    assert_eq!(env.ratchet(&["pr", id, "--yes"]).await.code, 0);
    let merged = env.ratchet(&["pr-merged", id]).await;
    assert_eq!(merged.code, 0, "{}", merged.stderr);
    let home: HomeVm = parse(&env.ratchet(&["home", "--json"]).await);
    assert_eq!(home.pull_requests.merged, 1);
    assert!(home.story.delivered.usd_per_year > 0.0);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn home_and_overview_print_the_same_value_story() {
    let env = Env::with(SimOptions {
        auto_run: false,
        github_flow_ms: 0,
        seed_value_story: true,
        ..SimOptions::default()
    })
    .await;
    let home = env.ratchet(&["home"]).await;
    let overview = env.ratchet(&["overview"]).await;
    assert_eq!(
        (home.code, overview.code),
        (0, 0),
        "{}{}",
        home.stderr,
        overview.stderr
    );
    let story: ValueStory = parse::<OverviewVm>(&env.ratchet(&["overview", "--json"]).await).story;
    assert!(
        story.line.contains("delivered") && story.line.contains("pending"),
        "{}",
        story.line
    );
    // both screens carry the sentence verbatim
    assert!(home.stdout.contains(&story.line), "{}", home.stdout);
    assert!(overview.stdout.contains(&story.line), "{}", overview.stdout);
    assert!(
        overview.stdout.contains("next 30 days"),
        "{}",
        overview.stdout
    );
    assert!(
        overview.stdout.contains("in your plan"),
        "{}",
        overview.stdout
    );
    let h: HomeVm = parse(&env.ratchet(&["home", "--json"]).await);
    assert_eq!(h.story, story);
    env.stop().await;
}
