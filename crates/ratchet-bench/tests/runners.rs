//! Port of `packages/bench/src/runners.test.ts`: the same behaviour is asserted for the LocalBenchRunner and for the
//! RemoteBenchRunner (whose generated scripts are run by a local bash), then the remote-only and local-only specifics.
//!
//! The fixture bench and check scripts are bash (`bench/bench.sh`, `tests/check.sh`) instead of node.

// What only the Linux-gated remote tests use is unused elsewhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::*;
use ratchet_bench::testkit::{
    Change, ConfigOptions, FixtureRepo, LocalFakeCompute, MemoryStore, alive, list_dir,
    make_config, make_fixture_repo, real_clock,
};
use ratchet_bench::{
    BenchError, BenchProgress, LocalBenchRunner, LocalBenchRunnerOptions, ProgressBenchRunner,
};
use ratchet_core::RatchetConfig;
use ratchet_core::ports::{BenchJob, BenchJobPhase, CancellationToken};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Local,
    Remote,
}

#[derive(Default, Clone)]
struct Opts {
    setup: Option<String>,
    bench_ms: Option<u64>,
    guardrail_ms: Option<u64>,
    env: Vec<(String, String)>,
}

struct Harness {
    runner: Arc<dyn ProgressBenchRunner>,
    store: Arc<MemoryStore>,
    /// Remote only.
    compute: Option<Arc<LocalFakeCompute>>,
    /// Local only: where job dirs are created.
    scratch: Option<PathBuf>,
    /// Remote only: the fake bucket root (`s3://<bucket>/<key>` is `<s3>/<bucket>/<key>`).
    s3: Option<PathBuf>,
}

fn env_pairs(o: &Opts) -> Vec<(&str, &str)> {
    o.env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

fn factory(kind: Kind, fx: &FixtureRepo, o: &Opts) -> Harness {
    match kind {
        Kind::Local => {
            let store = MemoryStore::new();
            let scratch = fx.path().join("scratch");
            let mut lo = LocalBenchRunnerOptions::new(fx.host.clone(), &scratch, real_clock());
            lo.artifacts = Some(store.clone());
            lo.setup = o.setup.clone();
            lo.env = o.env.iter().cloned().collect();
            if let Some(ms) = o.bench_ms {
                lo.timeouts.bench_ms = ms;
            }
            if let Some(ms) = o.guardrail_ms {
                lo.timeouts.guardrail_ms = ms;
            }
            Harness {
                runner: Arc::new(LocalBenchRunner::new(lo)),
                store,
                compute: None,
                scratch: Some(scratch),
                s3: None,
            }
        }
        Kind::Remote => {
            let r = rig(fx, &env_pairs(o), |ro| {
                ro.setup = o.setup.clone();
                if let Some(ms) = o.bench_ms {
                    ro.timeouts.bench_ms = ms;
                }
                if let Some(ms) = o.guardrail_ms {
                    ro.timeouts.guardrail_ms = ms;
                }
            });
            Harness {
                runner: Arc::new(r.runner),
                store: r.store,
                compute: Some(r.compute),
                scratch: None,
                s3: Some(r.s3),
            }
        }
    }
}

fn marker(fx: &FixtureRepo, kind: Kind, what: &str) -> PathBuf {
    fx.path().join(format!("marker-{kind:?}-{what}"))
}

fn opts_env(marker: &Path) -> Opts {
    Opts {
        env: vec![("MARKER".into(), marker.to_string_lossy().into_owned())],
        ..Opts::default()
    }
}

fn guardrail_cfg(lines: &[&str]) -> RatchetConfig {
    make_config(&ConfigOptions {
        guardrails: Some(lines.iter().map(|s| (*s).to_owned()).collect()),
        ..ConfigOptions::default()
    })
}

fn run_cfg(run: &str, guardrails: &[&str]) -> RatchetConfig {
    make_config(&ConfigOptions {
        run: Some(run.to_owned()),
        guardrails: Some(guardrails.iter().map(|s| (*s).to_owned()).collect()),
        ..ConfigOptions::default()
    })
}

fn std_config() -> RatchetConfig {
    make_config(&ConfigOptions::default())
}

fn err_text(e: &BenchError) -> String {
    e.to_string()
}

fn m(o: &ratchet_core::ports::BenchOutcome, run: usize, key: &str) -> f64 {
    o.runs[run]
        .metrics
        .get(key)
        .copied()
        .unwrap_or_else(|| panic!("run {run} has no metric {key}: {:?}", o.runs[run].metrics))
}

// ------------------------------------------------------------------------------------------------ shared cases

async fn runs_the_pipeline(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let sha = fx.candidate(&[("src/p95.txt", Change::write("80"))]);
    let progress: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = progress.clone();
    let cb = move |p: BenchProgress| {
        sink.lock().expect("lock").push(match p {
            BenchProgress::Bench { run, of, .. } => format!("bench {run}/{of}"),
            BenchProgress::Guardrails { index, total, .. } => format!("guardrails {index}/{total}"),
            BenchProgress::Checkout => "checkout".into(),
            BenchProgress::Setup => "setup".into(),
            BenchProgress::Deps => "deps".into(),
            BenchProgress::Evaluate => "evaluate".into(),
        });
    };
    let out = h
        .runner
        .run_with_progress(
            &job(&sha),
            &std_config(),
            Some(&baseline(&[("peak_rss_mb", 100.0)])),
            None,
            Some(&cb),
        )
        .await
        .expect("outcome");
    assert_eq!(out.samples, [80.0, 80.0, 80.0]);
    assert_eq!(out.runs.len(), 3);
    for (k, v) in [
        ("http_req_duration.p95", 80.0),
        ("http_req_duration.p50", 1.0),
        ("error_rate", 0.0),
        ("peak_rss_mb", 100.0),
        ("trace_set", 0.0),
        ("run", 1.0),
    ] {
        assert_eq!(m(&out, 0, k), v, "{k}");
    }
    assert!(
        !out.runs[0].metrics.contains_key("label"),
        "strings and arrays are not metrics"
    );
    assert!(!out.runs[0].metrics.contains_key("arr"));
    assert_eq!(
        (0..3).map(|i| m(&out, i, "run")).collect::<Vec<_>>(),
        [1.0, 2.0, 3.0]
    );
    assert_eq!(
        out.guardrails
            .iter()
            .map(|g| (g.name.as_str(), g.passed))
            .collect::<Vec<_>>(),
        [
            ("bash tests/check.sh", true),
            ("error_rate < 0.001", true),
            ("peak_rss < 1.2x", true)
        ]
    );
    assert!(out.finished_at >= out.started_at);
    assert_eq!(out.logs_key.as_deref(), Some("bench/opt1/cand1/bench.log"));
    let log = h.store.get_text("bench/opt1/cand1/bench.log").expect("log");
    assert!(log.contains("some log line"));
    assert!(
        regex::Regex::new(r"bench run 3/3")
            .expect("re")
            .is_match(&log)
    );
    assert_eq!(
        *progress.lock().expect("lock"),
        [
            "checkout",
            "guardrails 1/1",
            "bench 1/3",
            "bench 2/3",
            "bench 3/3",
            "evaluate"
        ]
    );
}

async fn stores_the_baseline_log_under_baseline(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let j = job_with(&fx.base, |j| {
        j.phase = BenchJobPhase::Baseline;
        j.repeats = 1;
        j.candidate_id = None;
    });
    let out = run(&*h.runner, &j, &std_config(), None)
        .await
        .expect("outcome");
    assert_eq!(
        out.logs_key.as_deref(),
        Some("bench/opt1/baseline/baseline.log")
    );
    // relative bounds are skipped (not failed) while establishing the baseline
    assert!(
        out.guardrails
            .iter()
            .find(|g| g.name.starts_with("peak_rss"))
            .expect("peak_rss")
            .passed
    );
    assert!(out.guardrails.iter().all(|g| g.passed));
}

async fn blocks_before_the_bench_when_a_command_guardrail_fails(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let mk = marker(&fx, kind, "guard");
    let h = factory(kind, &fx, &opts_env(&mk));
    let sha = fx.candidate(&[("src/p95.txt", Change::write("BROKEN"))]);
    let out = run(&*h.runner, &job(&sha), &std_config(), None)
        .await
        .expect("outcome");
    assert!(out.runs.is_empty() && out.samples.is_empty());
    assert_eq!(out.guardrails.len(), 1);
    assert_eq!(
        (
            out.guardrails[0].name.as_str(),
            out.guardrails[0].passed,
            out.guardrails[0].detail.as_deref()
        ),
        ("bash tests/check.sh", false, Some("exited with status 1"))
    );
    assert!(!mk.exists(), "the bench never started");
    assert!(
        h.store
            .get_text("bench/opt1/cand1/bench.log")
            .expect("log")
            .contains("check failed: broken")
    );
}

async fn fails_fast_later_command_guardrails_are_not_run(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let mk = marker(&fx, kind, "failfast");
    let h = factory(kind, &fx, &Opts::default());
    let touch = format!("- run: \"touch {}\"", mk.display());
    let out = run(
        &*h.runner,
        &job(&fx.base),
        &guardrail_cfg(&["- run: \"exit 3\"", &touch]),
        None,
    )
    .await
    .expect("outcome");
    assert_eq!(out.guardrails.len(), 1);
    assert!(!out.guardrails[0].passed);
    assert_eq!(
        out.guardrails[0].detail.as_deref(),
        Some("exited with status 3")
    );
    assert!(!mk.exists());
}

async fn ignores_candidate_edits_to_protected_bench_files(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let sha = fx.candidate(&[
        ("bench/bench.sh", Change::write("#!/bin/bash\necho '{\"http_req_duration\":{\"p95\":1},\"error_rate\":0,\"peak_rss_mb\":1}'\n")),
        ("bench/extra.js", Change::write("evil")),
        ("tests/check.sh", Change::write("exit 0\n")),
        ("tests/new.sh", Change::write("evil")),
        ("src/p95.txt", Change::write("90")),
    ]);
    let cfg = guardrail_cfg(&[
        "- run: \"test ! -e bench/extra.js && test ! -e tests/new.sh && ! grep -q '^exit 0$' tests/check.sh\"",
    ]);
    let out = run(&*h.runner, &job(&sha), &cfg, None)
        .await
        .expect("outcome");
    // 90 = the real bench ran against the candidate's src; bench.sh said 1
    assert_eq!(out.samples, [90.0, 90.0, 90.0]);
    assert!(
        out.guardrails.iter().all(|g| g.passed),
        "{:?}",
        out.guardrails
    );
}

async fn does_not_follow_a_candidate_symlink_that_replaces_a_protected_directory(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let sha = fx.candidate(&[
        ("bench", Change::symlink("evil")),
        (
            "evil/bench.sh",
            Change::write("echo '{\"http_req_duration\":{\"p95\":1}}'\n"),
        ),
        ("src/p95.txt", Change::write("95")),
    ]);
    let out = run(&*h.runner, &job_with(&sha, |j| j.repeats = 1), &cfg(), None)
        .await
        .expect("outcome");
    assert_eq!(out.samples, [95.0]);
}

async fn records_one_value_per_repeat_with_their_metrics(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let mk = marker(&fx, kind, "repeats");
    let mut o = opts_env(&mk);
    o.env.push(("JITTER".into(), "1".into()));
    let h = factory(kind, &fx, &o);
    let out = run(
        &*h.runner,
        &job_with(&fx.base, |j| j.repeats = 4),
        &cfg(),
        None,
    )
    .await
    .expect("outcome");
    assert_eq!(out.runs.len(), 4);
    let rounded: Vec<f64> = out
        .samples
        .iter()
        .map(|x| (x * 10.0).round() / 10.0)
        .collect();
    assert_eq!(rounded, [100.0, 100.1, 100.2, 100.3]);
    assert_eq!(
        fs::read_to_string(&mk)
            .expect("marker")
            .trim()
            .lines()
            .count(),
        4
    );
}

async fn evaluates_threshold_guardrails_on_run_medians_relative_ones_against_the_baseline(
    kind: Kind,
) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let cfg = guardrail_cfg(&["- error_rate: \"< 0.001\"", "- peak_rss: \"< 1.2x\""]);
    let bad = fx.candidate(&[
        ("src/rss.txt", Change::write("150")),
        ("src/err.txt", Change::write("0.01")),
    ]);
    let base = baseline(&[("peak_rss_mb", 100.0)]);
    let out = run(
        &*h.runner,
        &job_with(&bad, |j| j.repeats = 2),
        &cfg,
        Some(&base),
    )
    .await
    .expect("outcome");
    assert_eq!(out.guardrails.len(), 2);
    assert_eq!(
        (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
        ("error_rate < 0.001", false)
    );
    assert_eq!(
        (out.guardrails[1].name.as_str(), out.guardrails[1].passed),
        ("peak_rss < 1.2x", false)
    );
    let d = out.guardrails[1].detail.clone().expect("detail");
    assert!(
        d.contains("peak_rss_mb = 150") && d.contains("baseline 100"),
        "{d}"
    );

    let ok = fx.candidate(&[("src/rss.txt", Change::write("110"))]);
    let out2 = run(
        &*h.runner,
        &job_with(&ok, |j| j.repeats = 2),
        &cfg,
        Some(&base),
    )
    .await
    .expect("outcome");
    assert!(
        out2.guardrails.iter().all(|g| g.passed),
        "{:?}",
        out2.guardrails
    );

    // no baseline: a relative bound cannot be evaluated, and fails closed
    let out3 = run(&*h.runner, &job_with(&ok, |j| j.repeats = 1), &cfg, None)
        .await
        .expect("outcome");
    assert!(!out3.guardrails[1].passed);
    assert!(
        out3.guardrails[1]
            .detail
            .clone()
            .expect("detail")
            .contains("baseline")
    );
}

async fn fails_a_threshold_guardrail_whose_metric_the_bench_does_not_report(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let out = run(
        &*h.runner,
        &job_with(&fx.base, |j| j.repeats = 1),
        &guardrail_cfg(&["- p99_latency: \"< 5\""]),
        None,
    )
    .await
    .expect("outcome");
    assert!(!out.guardrails[0].passed);
    assert!(
        out.guardrails[0]
            .detail
            .clone()
            .expect("detail")
            .contains("not reported")
    );
}

async fn passes_ratchet_trace_to_confirm_runs_only_and_skips_command_guardrails_there(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let sealed =
        h.s3.clone()
            .unwrap_or_else(|| fx.path().join("s3"))
            .join("sealed");
    let trace_file = sealed.join("trace.jsonl");
    fs::create_dir_all(&sealed).expect("mkdir");
    fs::write(&trace_file, "0123456789").expect("write");
    let configured = if kind == Kind::Local {
        trace_file.to_string_lossy().into_owned()
    } else {
        "s3://sealed/trace.jsonl".to_owned()
    };
    let cfg = make_config(&ConfigOptions {
        confirm_trace: Some(configured),
        ..ConfigOptions::default()
    });
    let base = baseline(&[("peak_rss_mb", 100.0)]);
    let confirm = run(
        &*h.runner,
        &job_with(&fx.base, |j| {
            j.phase = BenchJobPhase::Confirm;
            j.repeats = 2;
        }),
        &cfg,
        Some(&base),
    )
    .await
    .expect("outcome");
    let seen: Vec<[f64; 3]> = (0..2)
        .map(|i| {
            [
                m(&confirm, i, "trace_set"),
                m(&confirm, i, "trace_bytes"),
                m(&confirm, i, "phase_is_confirm"),
            ]
        })
        .collect();
    assert_eq!(seen, [[1.0, 10.0, 1.0], [1.0, 10.0, 1.0]]);
    // threshold guardrails only; the command guardrail passed in the bench phase
    assert_eq!(
        confirm
            .guardrails
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["error_rate < 0.001", "peak_rss < 1.2x"]
    );

    // job.trace overrides the config; a bench-phase job never gets the trace
    fs::write(sealed.join("other.jsonl"), "abc").expect("write");
    let other = if kind == Kind::Local {
        sealed.join("other.jsonl").to_string_lossy().into_owned()
    } else {
        "s3://sealed/other.jsonl".into()
    };
    let over = run(
        &*h.runner,
        &job_with(&fx.base, |j| {
            j.phase = BenchJobPhase::Confirm;
            j.repeats = 1;
            j.trace = Some(other.clone());
        }),
        &cfg,
        Some(&base),
    )
    .await
    .expect("outcome");
    assert_eq!(m(&over, 0, "trace_bytes"), 3.0);
    let bench = run(
        &*h.runner,
        &job_with(&fx.base, |j| {
            j.repeats = 1;
            j.trace = Some(trace_file.to_string_lossy().into_owned());
        }),
        &cfg,
        Some(&base),
    )
    .await
    .expect("outcome");
    assert_eq!(m(&bench, 0, "trace_set"), 0.0);
}

async fn refuses_a_confirm_job_without_a_trace(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let e = run(
        &*h.runner,
        &job_with(&fx.base, |j| j.phase = BenchJobPhase::Confirm),
        &std_config(),
        None,
    )
    .await
    .expect_err("refused");
    assert!(matches!(e, BenchError::Infra(_)), "{e:?}");
}

async fn aborts_a_running_bench_and_kills_its_processes(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let mk = marker(&fx, kind, "abort");
    let h = factory(kind, &fx, &opts_env(&mk));
    let sha = fx.candidate(&[("src/hang.txt", Change::write("1"))]);
    let token = CancellationToken::new();
    let t0 = std::time::Instant::now();
    let runner = h.runner.clone();
    let (j, c, t) = (job(&sha), std_config(), token.clone());
    let task =
        tokio::spawn(async move { runner.run_with_progress(&j, &c, None, Some(&t), None).await });
    wait_for(|| mk.exists(), 20_000).await;
    token.cancel();
    let e = task.await.expect("joined").expect_err("aborted");
    assert_eq!(e, BenchError::Aborted);
    assert!(e.is_abort());
    assert!(t0.elapsed().as_secs() < 15);
    let pids: Vec<i32> = fs::read_to_string(&mk)
        .expect("marker")
        .trim()
        .lines()
        .map(|l| l.parse().expect("pid"))
        .collect();
    wait_for(|| pids.iter().all(|p| !alive(*p)), 5000).await;
    if let Some(c) = &h.compute {
        assert_eq!(c.terminated().len(), 1, "instance terminated on abort");
    }
    if let Some(s) = &h.scratch {
        assert!(list_dir(s).is_empty(), "job dir cleaned up");
    }
    assert!(
        h.store.get_text("bench/opt1/cand1/bench.log").is_some(),
        "the log is still stored"
    );
}

async fn rejects_immediately_when_the_signal_is_already_aborted(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let token = CancellationToken::new();
    token.cancel();
    let e = h
        .runner
        .run_with_progress(&job(&fx.base), &std_config(), None, Some(&token), None)
        .await
        .expect_err("aborted");
    assert_eq!(e, BenchError::Aborted);
    if let Some(c) = &h.compute {
        assert!(c.launched().is_empty());
    }
}

async fn turns_a_bench_timeout_into_a_failed_bench_run_guardrail(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(
        kind,
        &fx,
        &Opts {
            bench_ms: Some(1000),
            ..Opts::default()
        },
    );
    let sha = fx.candidate(&[("src/hang.txt", Change::write("1"))]);
    let out = run(&*h.runner, &job(&sha), &cfg(), None)
        .await
        .expect("outcome");
    assert!(out.runs.is_empty());
    assert_eq!(out.guardrails.len(), 1);
    assert_eq!(
        (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
        ("bench.run", false)
    );
    assert!(
        out.guardrails[0]
            .detail
            .clone()
            .expect("detail")
            .contains("run 1/3: timed out after 1000ms")
    );
}

async fn turns_a_command_guardrail_timeout_into_a_failed_guardrail(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(
        kind,
        &fx,
        &Opts {
            guardrail_ms: Some(1000),
            ..Opts::default()
        },
    );
    let out = run(
        &*h.runner,
        &job(&fx.base),
        &guardrail_cfg(&["- run: sleep 30"]),
        None,
    )
    .await
    .expect("outcome");
    assert_eq!(out.guardrails.len(), 1);
    assert_eq!(
        (
            out.guardrails[0].name.as_str(),
            out.guardrails[0].passed,
            out.guardrails[0].detail.as_deref()
        ),
        ("sleep 30", false, Some("timed out after 1000ms"))
    );
}

async fn reports_bench_failures_non_zero_exit_no_json_missing_metric(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let j = job_with(&fx.base, |j| j.repeats = 2);
    let exit2 = run(&*h.runner, &j, &run_cfg("echo hi; exit 2", &[]), None)
        .await
        .expect("outcome");
    assert_eq!(
        (
            exit2.guardrails[0].name.as_str(),
            exit2.guardrails[0].passed,
            exit2.guardrails[0].detail.as_deref()
        ),
        ("bench.run", false, Some("run 1/2: exited with status 2"))
    );
    let no_json = run(&*h.runner, &j, &run_cfg("echo hi", &[]), None)
        .await
        .expect("outcome");
    assert!(
        no_json.guardrails[0]
            .detail
            .clone()
            .expect("detail")
            .contains("no JSON object")
    );
    let missing = run(&*h.runner, &j, &run_cfg("echo '{\"a\":1}'", &[]), None)
        .await
        .expect("outcome");
    assert!(
        missing.guardrails[0]
            .detail
            .clone()
            .expect("detail")
            .contains("objective metric \"http_req_duration.p95\" not found")
    );
    assert!(missing.runs.is_empty());
}

async fn keeps_the_completed_runs_when_a_later_run_fails(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let mk = marker(&fx, kind, "partial");
    let h = factory(kind, &fx, &opts_env(&mk));
    let run_cmd = format!(
        "n=$(wc -l < {m} 2>/dev/null || echo 0); echo x >> {m}; if [ \"$n\" -ge 1 ]; then exit 9; fi; echo '{{\"http_req_duration\":{{\"p95\":7}}}}'",
        m = mk.display()
    );
    let out = run(
        &*h.runner,
        &job_with(&fx.base, |j| j.repeats = 3),
        &run_cfg(&run_cmd, &["- error_rate: \"< 0.001\""]),
        None,
    )
    .await
    .expect("outcome");
    assert_eq!(out.samples, [7.0]);
    // thresholds are not evaluated for an incomplete bench
    assert_eq!(out.guardrails.len(), 1);
    assert_eq!(
        (
            out.guardrails[0].name.as_str(),
            out.guardrails[0].detail.as_deref()
        ),
        ("bench.run", Some("run 2/3: exited with status 9"))
    );
}

async fn runs_setup_before_the_guardrails_and_blocks_when_it_fails(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let ok = factory(
        kind,
        &fx,
        &Opts {
            setup: Some("echo 77 > src/p95.txt".into()),
            ..Opts::default()
        },
    );
    let out = run(
        &*ok.runner,
        &job_with(&fx.base, |j| j.repeats = 1),
        &cfg(),
        None,
    )
    .await
    .expect("outcome");
    assert_eq!(out.samples, [77.0]);

    let bad = factory(
        kind,
        &fx,
        &Opts {
            setup: Some("echo setup-broke >&2; exit 4".into()),
            ..Opts::default()
        },
    );
    let out2 = run(&*bad.runner, &job(&fx.base), &std_config(), None)
        .await
        .expect("outcome");
    assert!(out2.runs.is_empty());
    assert_eq!(out2.guardrails.len(), 1);
    assert_eq!(
        (
            out2.guardrails[0].name.as_str(),
            out2.guardrails[0].passed,
            out2.guardrails[0].detail.as_deref()
        ),
        ("setup", false, Some("exited with status 4"))
    );
    assert!(
        bad.store
            .get_text("bench/opt1/cand1/bench.log")
            .expect("log")
            .contains("setup-broke")
    );
}

async fn blocks_a_candidate_whose_setup_rewrites_trusted_bench_files_and_restores_them(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let setup = "echo \"echo '{\\\"http_req_duration\\\":{\\\"p95\\\":1}}'\" > bench/bench.sh";
    let h = factory(
        kind,
        &fx,
        &Opts {
            setup: Some(setup.into()),
            ..Opts::default()
        },
    );
    let out = run(
        &*h.runner,
        &job_with(&fx.base, |j| j.repeats = 1),
        &guardrail_cfg(&["- run: \"true\""]),
        None,
    )
    .await
    .expect("outcome");
    assert!(out.runs.is_empty());
    assert_eq!(out.guardrails.len(), 1);
    assert_eq!(
        (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
        ("protected files intact", false)
    );
    assert!(
        out.guardrails[0]
            .detail
            .clone()
            .expect("detail")
            .contains("bench/bench.sh")
    );
}

async fn protects_the_evaluator_and_ratchet_yaml_with_no_protected_list_at_all(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let setup = "echo \"echo '{\\\"http_req_duration\\\":{\\\"p95\\\":1}}'\" > bench/bench.sh";
    let h = factory(
        kind,
        &fx,
        &Opts {
            setup: Some(setup.into()),
            ..Opts::default()
        },
    );
    let cfg = make_config(&ConfigOptions {
        guardrails: Some(vec!["- run: \"true\"".into()]),
        protected_globs: Some(vec![]),
        ..ConfigOptions::default()
    });
    let out = run(
        &*h.runner,
        &job_with(&fx.base, |j| j.repeats = 1),
        &cfg,
        None,
    )
    .await
    .expect("outcome");
    assert!(out.runs.is_empty());
    assert_eq!(
        (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
        ("protected files intact", false)
    );
    assert!(
        out.guardrails[0]
            .detail
            .clone()
            .expect("detail")
            .contains("bench/bench.sh")
    );
}

async fn fails_with_a_bench_error_when_bench_ref_has_no_protected_files(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let cfg = make_config(&ConfigOptions {
        protected_globs: Some(vec!["nothing/**".into()]),
        run: Some("sh ./score.sh".into()),
        ..ConfigOptions::default()
    });
    let e = run(&*h.runner, &job(&fx.base), &cfg, None)
        .await
        .expect_err("refused");
    assert!(err_text(&e).contains("contains no files matching"), "{e}");
}

async fn fails_with_a_bench_error_when_the_candidate_cannot_be_checked_out(kind: Kind) {
    let fx = make_fixture_repo(&[]);
    let h = factory(kind, &fx, &Opts::default());
    let e = run(&*h.runner, &job(&"deadbeef".repeat(5)), &std_config(), None)
        .await
        .expect_err("refused");
    assert!(matches!(e, BenchError::Infra(_)), "{e:?}");
    if let Some(c) = &h.compute {
        assert_eq!(c.terminated().len(), 1);
    }
    if let Some(s) = &h.scratch {
        assert!(list_dir(s).is_empty());
    }
}

macro_rules! shared {
    ($($name:ident),* $(,)?) => {
        mod local {
            use super::*;
            $( #[tokio::test] async fn $name() { super::$name(Kind::Local).await } )*
        }
        // the remote runner's scripts run under GNU tools (see PARITY.md): Linux only
        #[cfg(target_os = "linux")]
        mod remote {
            use super::*;
            $( #[tokio::test] async fn $name() { super::$name(Kind::Remote).await } )*
        }
    };
}

shared!(
    runs_the_pipeline,
    stores_the_baseline_log_under_baseline,
    blocks_before_the_bench_when_a_command_guardrail_fails,
    fails_fast_later_command_guardrails_are_not_run,
    ignores_candidate_edits_to_protected_bench_files,
    does_not_follow_a_candidate_symlink_that_replaces_a_protected_directory,
    records_one_value_per_repeat_with_their_metrics,
    evaluates_threshold_guardrails_on_run_medians_relative_ones_against_the_baseline,
    fails_a_threshold_guardrail_whose_metric_the_bench_does_not_report,
    passes_ratchet_trace_to_confirm_runs_only_and_skips_command_guardrails_there,
    refuses_a_confirm_job_without_a_trace,
    aborts_a_running_bench_and_kills_its_processes,
    rejects_immediately_when_the_signal_is_already_aborted,
    turns_a_bench_timeout_into_a_failed_bench_run_guardrail,
    turns_a_command_guardrail_timeout_into_a_failed_guardrail,
    reports_bench_failures_non_zero_exit_no_json_missing_metric,
    keeps_the_completed_runs_when_a_later_run_fails,
    runs_setup_before_the_guardrails_and_blocks_when_it_fails,
    blocks_a_candidate_whose_setup_rewrites_trusted_bench_files_and_restores_them,
    protects_the_evaluator_and_ratchet_yaml_with_no_protected_list_at_all,
    fails_with_a_bench_error_when_bench_ref_has_no_protected_files,
    fails_with_a_bench_error_when_the_candidate_cannot_be_checked_out,
);

#[tokio::test]
async fn local_can_keep_candidate_only_protected_files_when_pruning_is_off() {
    let fx = make_fixture_repo(&[]);
    let mut o =
        LocalBenchRunnerOptions::new(fx.host.clone(), fx.path().join("scratch2"), real_clock());
    o.artifacts = Some(MemoryStore::new());
    o.prune_extra_protected = false;
    let runner = LocalBenchRunner::new(o);
    let sha = fx.candidate(&[("tests/new.sh", Change::write("x"))]);
    let cfg = guardrail_cfg(&["- run: test -e tests/new.sh"]);
    let out = run(&runner, &job_with(&sha, |j| j.repeats = 1), &cfg, None)
        .await
        .expect("outcome");
    assert!(out.guardrails[0].passed);
}

// ------------------------------------------------------------------------------------------------ RemoteBenchRunner specifics

#[cfg(target_os = "linux")]
mod remote_specifics {
    use super::*;

    fn plain(fx: &FixtureRepo) -> Rig {
        rig(fx, &[], |_| {})
    }

    fn sha_ok(fx: &FixtureRepo) -> BenchJob {
        job_with(&fx.base, |j| j.repeats = 1)
    }

    #[tokio::test]
    async fn launches_one_pinned_instance_per_job_with_the_right_purpose_and_always_terminates_it()
    {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        run(&r.runner, &sha_ok(&fx), &cfg(), None)
            .await
            .expect("bench");
        let trace = fx.path().join("trace.txt").to_string_lossy().into_owned();
        let _ = run(
            &r.runner,
            &job_with(&fx.base, |j| {
                j.repeats = 1;
                j.phase = BenchJobPhase::Confirm;
                j.trace = Some(trace.clone());
            }),
            &cfg(),
            None,
        )
        .await;
        let launched: Vec<_> = r
            .compute
            .launched()
            .into_iter()
            .map(|l| {
                (
                    l.instance_type,
                    format!("{:?}", l.purpose),
                    l.image_ref,
                    l.optimization_id.to_string(),
                )
            })
            .collect();
        assert_eq!(
            launched,
            [
                (
                    "c7i.2xlarge".to_owned(),
                    "Bench".to_owned(),
                    "ami-test".to_owned(),
                    "opt1".to_owned()
                ),
                (
                    "c7i.2xlarge".to_owned(),
                    "Confirm".to_owned(),
                    "ami-test".to_owned(),
                    "opt1".to_owned()
                ),
            ]
        );
        assert_eq!(r.compute.terminated(), ["i-fake1", "i-fake2"]);
    }

    #[tokio::test]
    async fn terminates_the_instance_even_when_a_step_throws_bad_ref() {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        let e = run(&r.runner, &job("nonexistent-ref"), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(err_text(&e).contains("cannot resolve candidate"), "{e}");
        assert_eq!(r.compute.terminated(), ["i-fake1"]);
    }

    #[tokio::test]
    async fn waits_for_the_instance_to_register_before_the_first_command() {
        let fx = make_fixture_repo(&[]);
        let r = rig(&fx, &[], |o| o.ready_delay_ms = Some(7));
        r.compute.set_not_ready_for(2);
        let out = run(&r.runner, &sha_ok(&fx), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [100.0]);
        assert_eq!(r.clock.slept(), [7, 7]);
    }

    #[tokio::test]
    async fn gives_up_when_the_instance_never_becomes_ready_and_still_terminates_it() {
        let fx = make_fixture_repo(&[]);
        let r = rig(&fx, &[], |o| o.ready_attempts = Some(3));
        r.compute.set_not_ready_for(99);
        let e = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("never ready");
        assert!(err_text(&e).contains("never became ready"), "{e}");
        assert_eq!(r.compute.terminated().len(), 1);
    }

    #[tokio::test]
    async fn reports_but_survives_a_failing_terminate() {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        r.compute.set_fail_terminate(true);
        let out = run(&r.runner, &sha_ok(&fx), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [100.0]);
        assert!(
            r.warnings()[0].contains("FAILED to terminate bench instance i-fake1"),
            "{:?}",
            r.warnings()
        );
    }

    #[tokio::test]
    async fn sends_one_script_per_step_and_keeps_each_output_within_the_ssm_limit() {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        let c = guardrail_cfg(&["- run: bash tests/check.sh", "- run: \"true\""]);
        run(&r.runner, &job_with(&fx.base, |j| j.repeats = 2), &c, None)
            .await
            .expect("outcome");
        // ping, checkout, 2 guardrails, 2 bench runs
        let scripts = r.compute.scripts();
        assert_eq!(scripts.len(), 6);
        // each is wrapped so it runs under bash even if SSM uses sh
        for s in &scripts {
            assert!(
                s.starts_with("f=$(mktemp)\ncat > \"$f\" <<'__RATCHET_STEP_EOF__'"),
                "{s}"
            );
        }
        let checkout = &scripts[1];
        assert!(checkout.contains("aws s3 cp"));
        assert!(checkout.contains(&format!("s3://{BUCKET}/bench-inputs/opt1/")));
        assert!(checkout.contains(&fx.base));
    }

    #[tokio::test]
    async fn pre_stages_the_candidate_and_bench_ref_and_never_contacts_a_git_remote_from_the_instance()
     {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        run(&r.runner, &sha_ok(&fx), &cfg(), None)
            .await
            .expect("outcome");
        let puts = r.staging.puts();
        assert_eq!(puts.len(), 2);
        let code = regex::Regex::new(r"^bench-inputs/opt1/[0-9a-f-]{36}/code\.tgz$").expect("re");
        let bench = regex::Regex::new(r"^bench-inputs/opt1/[0-9a-f-]{36}/bench\.tgz$").expect("re");
        assert!(
            code.is_match(&puts[0]) && bench.is_match(&puts[1]),
            "{puts:?}"
        );
        let forbidden =
            regex::Regex::new(r"git (--git-dir|fetch|clone|ls-remote)|github\.com|https?://")
                .expect("re");
        for s in r.compute.scripts() {
            assert!(!forbidden.is_match(&s));
            assert!(
                !s.contains(&fx.repo.to_string_lossy().into_owned()),
                "no remote url, no token"
            );
        }
    }

    #[tokio::test]
    async fn refuses_to_run_a_job_without_a_staging_bucket_or_store_fail_closed() {
        let fx = make_fixture_repo(&[]);
        let r = rig(&fx, &[], |o| o.staging_bucket = None);
        let e = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(err_text(&e).contains("needs stagingBucket"), "{e}");
        assert!(r.compute.commands().is_empty());
        assert_eq!(r.compute.terminated().len(), 1);
        let no_store = rig(&fx, &[], |o| {
            o.staging_store = None;
            o.artifacts = None;
        });
        let e = run(&no_store.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(err_text(&e).contains("needs stagingBucket"), "{e}");
    }

    #[tokio::test]
    async fn rejects_an_optimization_id_that_is_not_a_z0_9_dash_before_anything_is_staged() {
        for bad in [
            "*", "?", "/", "..", "a/b", "../opt2", "opt1/..", "a*", "Opt1", "",
        ] {
            let fx = make_fixture_repo(&[]);
            let r = plain(&fx);
            let j = job_with(&fx.base, |j| {
                j.optimization_id = ratchet_core::OptimizationId::new(bad)
            });
            let e = run(&r.runner, &j, &cfg(), None).await.expect_err(bad);
            assert!(
                err_text(&e).contains("must match [a-z0-9-]"),
                "{bad:?}: {e}"
            );
            assert!(r.staging.puts().is_empty());
            assert!(r.compute.commands().is_empty());
        }
    }

    #[tokio::test]
    async fn rejects_a_staging_bucket_that_is_not_a_plain_bucket_name() {
        for bad in ["a/b", "bucket*", "..", "x'; rm -rf /", "A_B"] {
            let fx = make_fixture_repo(&[]);
            let r = rig(&fx, &[], |o| o.staging_bucket = Some(bad.into()));
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err(bad);
            assert!(
                err_text(&e).contains("not a valid bucket name"),
                "{bad:?}: {e}"
            );
            assert!(r.staging.puts().is_empty());
        }
    }

    #[tokio::test]
    async fn detects_a_staged_archive_that_was_swapped_after_upload_sha256_check_on_the_instance() {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        r.staging.set_after_put(|key, path| {
            if key.ends_with("code.tgz") {
                fs::write(path, "not the archive").expect("swap");
            }
        });
        let e = run(&r.runner, &sha_ok(&fx), &cfg(), None)
            .await
            .expect_err("mismatch");
        assert!(err_text(&e).contains("sha256 mismatch"), "{e}");
    }

    #[tokio::test]
    async fn parses_a_very_chatty_bench_because_only_the_stdout_tail_travels() {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        let noisy = "for i in $(seq 1 5000); do echo \"noise line $i\"; done; echo '{\"http_req_duration\":{\"p95\":3}}'";
        let out = run(&r.runner, &sha_ok(&fx), &run_cfg(noisy, &[]), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [3.0]);
    }

    #[tokio::test]
    async fn rejects_refs_that_are_not_plain_git_refs_before_sending_anything() {
        let fx = make_fixture_repo(&[]);
        let r = plain(&fx);
        let e = run(&r.runner, &job("--upload-pack=evil"), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(err_text(&e).contains("not a plain git ref"), "{e}");
        assert_eq!(r.compute.terminated().len(), 1);
    }

    mod isolation_user {
        use super::*;

        fn shimmed(fx: &FixtureRepo, overrides: &[(&str, &str)]) -> Rig {
            user_rig(fx, overrides, |_| {})
        }

        fn holdout_files(r: &Rig) -> Vec<PathBuf> {
            list_children(&r.work_root)
                .into_iter()
                .map(|d| d.join("holdout"))
                .filter(|d| d.exists())
                .flat_map(|d| {
                    list_children(&d).into_iter().filter(|f| {
                        f.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("trace"))
                    })
                })
                .collect()
        }

        fn seed_trace(r: &Rig, body: &str) -> String {
            fs::create_dir_all(r.s3.join(BUCKET)).expect("mkdir");
            fs::write(r.s3.join(BUCKET).join("trace.json"), body).expect("write");
            format!("s3://{BUCKET}/trace.json")
        }

        #[tokio::test]
        async fn verifies_both_users_the_imds_block_the_holdout_permissions_and_the_launcher_then_splits_the_principals()
         {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[]);
            let out = run(
                &r.runner,
                &sha_ok(&fx),
                &guardrail_cfg(&["- run: \"true\""]),
                None,
            )
            .await
            .expect("outcome");
            assert_eq!(out.samples, [100.0]);
            let l = r.shim_log();
            for u in ["ratchet-bench", "ratchet-holdout"] {
                assert!(
                    l.contains(&format!(
                        "iptables -C OUTPUT -m owner --uid-owner {u} -d 169.254.169.254 -j REJECT"
                    )),
                    "{u}"
                );
            }
            assert!(
                l.contains("chown -R ratchet-bench:") && l.contains("chown -R ratchet-holdout:")
            );
            // setup / guardrails: candidate user, no new privileges. Bench harness: the OTHER user, from the snapshot
            assert!(l.contains("setpriv --reuid=ratchet-bench --regid=ratchet-bench --clear-groups --no-new-privs -- env -u AWS_ACCESS_KEY_ID"));
            assert!(regex::Regex::new(r"setpriv --reuid=ratchet-holdout --regid=ratchet-holdout --clear-groups -- env -u AWS_ACCESS_KEY_ID").expect("re").is_match(&l));
            // before and after every step: both users' processes are killed
            assert!(l.matches("pkill -KILL -u ratchet-bench").count() >= 4);
            assert!(l.matches("pkill -KILL -u ratchet-holdout").count() >= 4);
            let step = r
                .compute
                .scripts()
                .into_iter()
                .find(|s| s.contains("bench.sh"))
                .expect("bench step");
            assert!(step.contains(
                "env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN"
            ));
            assert!(step.contains("RATCHET_CANDIDATE_EXEC="));
        }

        #[tokio::test]
        async fn runs_the_harness_from_a_root_owned_read_only_snapshot_of_the_built_tree() {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[]);
            let bench = "printf '{\"http_req_duration\":{\"p95\":1},\"cwd_is_snapshot\":%s,\"src_visible\":%s}\\n' \"$([[ $PWD == */harness ]] && echo 1 || echo 0)\" \"$([ -e src/p95.txt ] && echo 1 || echo 0)\"";
            let out = run(&r.runner, &sha_ok(&fx), &run_cfg(bench, &[]), None)
                .await
                .expect("outcome");
            assert_eq!(
                (m(&out, 0, "cwd_is_snapshot"), m(&out, 0, "src_visible")),
                (1.0, 1.0)
            );
            assert!(
                regex::Regex::new(r"chown -hR root:root .*harness\.tmp")
                    .expect("re")
                    .is_match(&r.shim_log())
            );
            let bs = r
                .compute
                .scripts()
                .into_iter()
                .filter(|s| s.contains("snapshot()"))
                .count();
            assert_eq!(
                bs, 1,
                "one bench run, one script; the snapshot is made once and reused by later runs"
            );
        }

        #[tokio::test]
        async fn reuses_the_snapshot_for_later_runs_instead_of_copying_the_tree_again() {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[]);
            run(
                &r.runner,
                &job_with(&fx.base, |j| j.repeats = 2),
                &cfg(),
                None,
            )
            .await
            .expect("outcome");
            let scripts: Vec<String> = r
                .compute
                .scripts()
                .into_iter()
                .filter(|s| s.contains("snapshot()"))
                .collect();
            assert_eq!(scripts.len(), 2);
            assert!(
                scripts
                    .iter()
                    .all(|s| s.contains("[ -d \"$H\" ] || snapshot"))
            );
        }

        #[tokio::test]
        async fn refuses_to_run_the_candidate_when_the_image_has_no_bench_user() {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[("id", "exit 1")]);
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err("refused");
            assert!(
                err_text(&e).contains("bench user ratchet-bench does not exist"),
                "{e}"
            );
            assert_eq!(r.compute.terminated().len(), 1);
            assert!(r.compute.scripts().iter().all(|s| !s.contains("bench.sh")));
        }

        #[tokio::test]
        async fn refuses_when_the_image_has_no_holdout_user() {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(
                &fx,
                &[("id", r#"[ "$2" = ratchet-bench ] && exit 0; exit 1"#)],
            );
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err("refused");
            assert!(
                err_text(&e).contains("holdout user ratchet-holdout does not exist"),
                "{e}"
            );
        }

        #[tokio::test]
        async fn refuses_to_run_the_candidate_when_the_imds_block_rule_is_missing() {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[("iptables", "exit 1")]);
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err("refused");
            assert!(err_text(&e).contains("IMDS is not blocked"), "{e}");
        }

        #[tokio::test]
        async fn refuses_when_the_imds_rule_exists_for_the_bench_user_but_not_for_the_holdout_user()
        {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(
                &fx,
                &[(
                    "iptables",
                    r#"case "$*" in *ratchet-holdout*) exit 1;; esac; exit 0"#,
                )],
            );
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err("refused");
            assert!(
                err_text(&e).contains("IMDS is not blocked for ratchet-holdout"),
                "{e}"
            );
        }

        #[tokio::test]
        async fn refuses_to_run_the_candidate_when_instance_metadata_is_still_reachable_as_the_bench_user()
         {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[("setpriv", "exit 0")]);
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err("refused");
            assert!(err_text(&e).contains("metadata is reachable"), "{e}");
        }

        #[tokio::test]
        async fn refuses_when_the_holdout_directory_could_be_read_by_the_bench_user() {
            let fx = make_fixture_repo(&[]);
            // a setpriv that lets the bench user open anything
            let r = shimmed(
                &fx,
                &[(
                    "setpriv",
                    r#"if [[ "$*" == *"/dev/tcp/169.254.169.254"* ]]; then exit 1; fi; while [ "$1" != -- ]; do shift; done; shift; exec "$@""#,
                )],
            );
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err("refused");
            assert!(
                err_text(&e).contains("holdout directory is readable by ratchet-bench"),
                "{e}"
            );
        }

        #[tokio::test]
        async fn refuses_when_the_launcher_does_not_run_commands_as_the_bench_user() {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[("ratchet-candidate", "echo root")]);
            let e = run(&r.runner, &job(&fx.base), &cfg(), None)
                .await
                .expect_err("refused");
            assert!(
                regex::Regex::new(r"candidate launcher .* does not run commands as ratchet-bench")
                    .expect("re")
                    .is_match(&err_text(&e)),
                "{e}"
            );
        }

        mod the_holdout_confirm {
            use super::*;

            fn confirm_job(fx: &FixtureRepo, uri: &str) -> BenchJob {
                job_with(&fx.base, |j| {
                    j.repeats = 2;
                    j.phase = BenchJobPhase::Confirm;
                    j.trace = Some(uri.into());
                })
            }

            #[tokio::test]
            async fn is_fetched_by_root_only_after_setup_and_guardrails_have_finished_and_is_gone_again_after_every_run()
             {
                let fx = make_fixture_repo(&[]);
                let r = shimmed(&fx, &[]);
                let uri = seed_trace(&r, "{\"requests\":[1,2,3]}");
                // setup and the command guardrail FAIL if the holdout exists while they run (cwd is <work>/code)
                let gone = "test -z \"$(ls ../holdout 2>/dev/null)\"";
                let out = run(
                    &r.runner,
                    &confirm_job(&fx, &uri),
                    &guardrail_cfg(&[&format!("- run: '{gone}'")]),
                    None,
                )
                .await
                .expect("outcome");
                assert!(
                    out.guardrails.iter().all(|g| g.name != "bench.run"),
                    "{:?}",
                    out.guardrails
                );
                assert_eq!(out.samples, [100.0, 100.0]);
                assert_eq!(
                    (0..2)
                        .map(|i| (m(&out, i, "trace_set"), m(&out, i, "trace_bytes")))
                        .collect::<Vec<_>>(),
                    [(1.0, 20.0), (1.0, 20.0)]
                );
                assert!(holdout_files(&r).is_empty(), "wiped after the last run");
                // order of remote steps: ping/checkout, then every bench step; the sealed location is only in bench steps
                let scripts = r.compute.scripts();
                let with_trace: Vec<usize> = scripts
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.contains("trace.json"))
                    .map(|(i, _)| i)
                    .collect();
                let first_bench = scripts
                    .iter()
                    .position(|s| s.contains("bench.sh"))
                    .expect("a bench step");
                assert_eq!(with_trace.len(), 2);
                assert!(with_trace.iter().all(|i| *i >= first_bench));
                for s in &scripts[..first_bench] {
                    assert!(!s.contains("aws s3 cp --only-show-errors \"$URI\""));
                }
                // permissions: fetched as root (no setpriv before the fetch), handed to the harness user alone, mode 0600
                let bench_script = &scripts[first_bench];
                let idx = |n: &str| {
                    bench_script
                        .find(n)
                        .unwrap_or_else(|| panic!("missing {n}"))
                };
                assert!(idx("chown \"$HU:\" \"$TRACE\"") > idx("aws s3 cp"));
                assert!(idx("chmod 600 \"$TRACE\"") > idx("chown \"$HU:\" \"$TRACE\""));
                assert!(idx("setpriv") > idx("chmod 600 \"$TRACE\""));
                assert!(
                    !regex::Regex::new(r"chmod 4[0-9]0|chgrp")
                        .expect("re")
                        .is_match(bench_script)
                );
                assert!(
                    regex::Regex::new(r"chown ratchet-holdout: .*holdout/trace\.json")
                        .expect("re")
                        .is_match(&r.shim_log())
                );
                assert_eq!(r.compute.terminated().len(), 1);
            }

            #[tokio::test]
            async fn keeps_ratchet_trace_and_the_sealed_location_out_of_everything_the_candidate_user_runs()
             {
                let fx = make_fixture_repo(&[]);
                let r = user_rig(&fx, &[], |o| o.setup = Some("true".into()));
                let uri = seed_trace(&r, "{\"requests\":[1,2,3]}");
                run(&r.runner, &confirm_job(&fx, &uri), &cfg(), None)
                    .await
                    .expect("outcome");
                let candidate_steps: Vec<String> = r
                    .compute
                    .scripts()
                    .into_iter()
                    .filter(|s| s.contains("--no-new-privs"))
                    .collect();
                assert!(candidate_steps.len() >= 2, "checkout (probes) and setup");
                for s in &candidate_steps {
                    assert!(!s.contains("RATCHET_TRACE") && !s.contains("trace.json"));
                }
            }

            #[tokio::test]
            async fn never_passes_the_holdout_to_candidate_processes_the_launcher_starts_them_with_a_clean_environment()
             {
                let fx = make_fixture_repo(&[]);
                // like the real launcher: `env -i`
                let r = shimmed(
                    &fx,
                    &[(
                        "ratchet-candidate",
                        r#"if [ "$*" = "id -un" ]; then echo ratchet-bench; exit 0; fi; exec env -i PATH="$PATH" "$@""#,
                    )],
                );
                let uri = seed_trace(&r, "{\"requests\":[1,2,3]}");
                let probe = "leaked=$(\"$RATCHET_CANDIDATE_EXEC\" sh -c 'test -n \"$RATCHET_TRACE\" && echo 1 || echo 0'); printf '{\"http_req_duration\":{\"p95\":1},\"leaked\":%s,\"harness_has_trace\":%s}\\n' \"$leaked\" \"$([ -n \"${RATCHET_TRACE:-}\" ] && echo 1 || echo 0)\"";
                let out = run(
                    &r.runner,
                    &confirm_job(&fx, &uri),
                    &run_cfg(probe, &[]),
                    None,
                )
                .await
                .expect("outcome");
                assert_eq!(
                    (m(&out, 0, "harness_has_trace"), m(&out, 0, "leaked")),
                    (1.0, 0.0)
                );
            }

            #[tokio::test]
            async fn is_fetched_once_per_run_and_the_harness_has_it_at_that_run() {
                let fx = make_fixture_repo(&[]);
                let r = shimmed(&fx, &[]);
                let uri = seed_trace(&r, "0123456789");
                let out = run(&r.runner, &confirm_job(&fx, &uri), &cfg(), None)
                    .await
                    .expect("outcome");
                assert_eq!(
                    (0..2)
                        .map(|i| m(&out, i, "trace_bytes"))
                        .collect::<Vec<_>>(),
                    [10.0, 10.0]
                );
            }

            #[tokio::test]
            async fn is_wiped_when_the_harness_crashes_or_times_out() {
                let fx = make_fixture_repo(&[]);
                let r = user_rig(&fx, &[], |o| o.timeouts.bench_ms = 1500);
                let uri = seed_trace(&r, "{\"requests\":[1,2,3]}");
                let out = run(
                    &r.runner,
                    &confirm_job(&fx, &uri),
                    &run_cfg("sleep 60", &[]),
                    None,
                )
                .await
                .expect("outcome");
                assert!(
                    out.guardrails
                        .iter()
                        .any(|g| g.name == "bench.run" && !g.passed)
                );
                assert!(holdout_files(&r).is_empty());
                let r2 = shimmed(&fx, &[]);
                let uri2 = seed_trace(&r2, "{\"requests\":[1,2,3]}");
                let out2 = run(
                    &r2.runner,
                    &confirm_job(&fx, &uri2),
                    &run_cfg("exit 3", &[]),
                    None,
                )
                .await
                .expect("outcome");
                assert!(
                    out2.guardrails
                        .iter()
                        .any(|g| g.name == "bench.run" && !g.passed)
                );
                assert!(holdout_files(&r2).is_empty());
            }

            #[tokio::test]
            async fn turns_a_failed_fetch_into_an_infrastructure_error_not_a_candidate_failure() {
                let fx = make_fixture_repo(&[]);
                let r = shimmed(&fx, &[]);
                let e = run(
                    &r.runner,
                    &confirm_job(&fx, &format!("s3://{BUCKET}/missing.json")),
                    &cfg(),
                    None,
                )
                .await
                .expect_err("refused");
                assert!(
                    regex::Regex::new(r"fetching the holdout .* failed")
                        .expect("re")
                        .is_match(&err_text(&e)),
                    "{e}"
                );
                assert_eq!(r.compute.terminated().len(), 1);
            }

            #[tokio::test]
            async fn does_not_write_the_output_of_a_holdout_run_to_the_stored_log() {
                let fx = make_fixture_repo(&[]);
                let r = shimmed(&fx, &[]);
                let uri = seed_trace(&r, "{\"requests\":[1,2,3]}");
                let noisy = "echo SECRET-STDERR >&2; echo SECRET-STDOUT; echo '{\"http_req_duration\":{\"p95\":2}}'";
                let out = run(
                    &r.runner,
                    &confirm_job(&fx, &uri),
                    &run_cfg(noisy, &[]),
                    None,
                )
                .await
                .expect("outcome");
                assert_eq!(out.samples, [2.0, 2.0]);
                let log = r
                    .store
                    .get_text(out.logs_key.as_deref().expect("logs key"))
                    .expect("log");
                assert!(!log.contains("SECRET-STDERR") && !log.contains("SECRET-STDOUT"));
                assert!(
                    regex::Regex::new(r"holdout: 20 bytes fetched")
                        .expect("re")
                        .is_match(&log)
                );
                // a bench-phase run of the same command does keep its tails (debuggability)
                let out2 = run(
                    &r.runner,
                    &job_with(&fx.base, |j| {
                        j.repeats = 1;
                        j.candidate_id = Some(ratchet_core::CandidateId::new("c2"));
                    }),
                    &run_cfg(noisy, &[]),
                    None,
                )
                .await
                .expect("outcome");
                assert!(
                    r.store
                        .get_text(out2.logs_key.as_deref().expect("logs key"))
                        .expect("log")
                        .contains("SECRET-STDERR")
                );
            }
        }

        #[tokio::test]
        async fn bench_and_baseline_phases_never_contain_the_sealed_location_or_a_fetch_even_if_a_trace_is_passed()
         {
            let fx = make_fixture_repo(&[]);
            let r = shimmed(&fx, &[]);
            let uri = seed_trace(&r, "{\"requests\":[1,2,3]}");
            let out = run(
                &r.runner,
                &job_with(&fx.base, |j| {
                    j.repeats = 2;
                    j.trace = Some(uri.clone());
                }),
                &cfg(),
                None,
            )
            .await
            .expect("outcome");
            assert_eq!(
                (0..2).map(|i| m(&out, i, "trace_set")).collect::<Vec<_>>(),
                [0.0, 0.0]
            );
            for s in r.compute.scripts() {
                assert!(!s.contains("trace.json"));
                assert!(!s.contains("aws s3 cp --only-show-errors \"$URI\""));
                assert!(!s.contains("RATCHET_TRACE"));
            }
            assert_eq!(
                r.compute
                    .launched()
                    .iter()
                    .map(|l| format!("{:?}", l.purpose))
                    .collect::<Vec<_>>(),
                ["Bench"]
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------ LocalBenchRunner specifics

mod local_specifics {
    use super::*;

    #[tokio::test]
    async fn checks_out_the_candidate_and_bench_ref_into_a_fresh_dir_under_scratch_root_and_cleans_up()
     {
        let fx = make_fixture_repo(&[]);
        let scratch = fx.path().join("scr");
        let runner = LocalBenchRunner::new(LocalBenchRunnerOptions::new(
            fx.host.clone(),
            &scratch,
            real_clock(),
        ));
        let sha = fx.candidate(&[("src/p95.txt", Change::write("60"))]);
        let out = run(
            &runner,
            &job_with(&sha, |j| j.repeats = 1),
            &std_config(),
            Some(&baseline(&[("peak_rss_mb", 100.0)])),
        )
        .await
        .expect("outcome");
        assert_eq!(out.samples, [60.0]);
        let checkouts = fx.host.checkouts();
        let last2 = &checkouts[checkouts.len() - 2..];
        assert_eq!(
            last2.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>(),
            [sha.as_str(), "ratchet/bench"]
        );
        assert!(last2.iter().all(|(_, d)| d.starts_with(&scratch)));
        assert!(list_dir(&scratch).is_empty());
    }

    #[tokio::test]
    async fn keep_workdir_leaves_the_job_directory_for_debugging() {
        let fx = make_fixture_repo(&[]);
        let scratch = fx.path().join("scr-keep");
        let mut o = LocalBenchRunnerOptions::new(fx.host.clone(), &scratch, real_clock());
        o.keep_workdir = true;
        let runner = LocalBenchRunner::new(o);
        run(
            &runner,
            &job_with(&fx.base, |j| j.repeats = 1),
            &cfg(),
            None,
        )
        .await
        .expect("outcome");
        let dirs = list_dir(&scratch);
        assert_eq!(dirs.len(), 1);
        assert!(scratch.join(&dirs[0]).join("code/bench/bench.sh").exists());
    }
}
