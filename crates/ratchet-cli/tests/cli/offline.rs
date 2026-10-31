//! The offline commands, ported from `packages/cli/src/{program,commands/{validate,init,baseline}}.test.ts`.

use ratchet_core::config::load_config;
use ratchet_core::enums::TemplateKind;
use ratchet_core::templates::{TemplateFilter, list_templates};

use crate::common::{RepoOpts, make_repo, ratchet};

fn templates() -> Vec<ratchet_core::templates::TemplateInfo> {
    list_templates(TemplateFilter {
        kind: Some(TemplateKind::Template),
        category: None,
    })
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn has_node() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

// ---- validate (validate.test.ts) ----------------------------------------------------------------------------

#[tokio::test]
async fn validate_prints_a_summary_and_exits_0_for_a_valid_config() {
    let repo = make_repo(RepoOpts::new(&[1.0]));
    let o = ratchet(&["validate", repo.config.to_str().unwrap()]).await;
    assert_eq!(o.code, 0);
    assert_eq!(o.stderr, "");
    for want in [
        "OK",
        "minimize lat.p95 (min gain 3%)",
        "sh bench.sh",
        "pytest -q; error_rate < 0.001",
        "bench/**, ratchet.yaml",
    ] {
        assert!(
            o.stdout.contains(want),
            "missing {want:?} in:\n{}",
            o.stdout
        );
    }
}

#[tokio::test]
async fn validate_prints_errors_with_yaml_paths_and_exits_1_for_an_invalid_config() {
    let dir = tmp();
    let p = dir.path().join("ratchet.yaml");
    std::fs::write(
        &p,
        "version: 0\nobjective: { metric: m, direction: sideways, min_gain: 0.03 }\nbench: { ref: r, run: x, repeats: 3, hardware: h }\nguardrails:\n  - error_rate: \"around 1\"\nagent: { instructions: R.md, allowed_instance_types: [a] }\n",
    )
    .unwrap();
    let o = ratchet(&["validate", p.to_str().unwrap()]).await;
    assert_eq!(o.code, 1);
    assert_eq!(o.stdout, "");
    assert!(o.stderr.contains("invalid (2 errors)"), "{}", o.stderr);
    assert!(o.stderr.contains("objective.direction: direction must be"));
    assert!(
        o.stderr
            .contains("guardrails[0].error_rate: invalid threshold \"around 1\"")
    );
}

#[tokio::test]
async fn validate_exits_1_when_the_file_is_missing() {
    let o = ratchet(&["validate", "/nonexistent/ratchet.yaml"]).await;
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("cannot find /nonexistent/ratchet.yaml"),
        "{}",
        o.stderr
    );
}

#[tokio::test]
async fn validate_json_has_the_summary_or_the_errors() {
    let repo = make_repo(RepoOpts::new(&[1.0]));
    let o = ratchet(&["validate", repo.config.to_str().unwrap(), "--json"]).await;
    assert_eq!(o.code, 0);
    let v = o.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["summary"]["metric"], "lat.p95");
    assert_eq!(v["summary"]["evaluator"]["run"], "sh bench.sh");
    assert_eq!(v["summary"]["minGain"], 0.03);
    let o = ratchet(&["validate", "/nonexistent/ratchet.yaml", "--json"]).await;
    assert_eq!(o.code, 1);
    let v = o.json();
    assert_eq!(v["ok"], false);
    assert!(
        v["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("cannot find")
    );
}

// ---- init (init.test.ts) --------------------------------------------------------------------------------------

#[tokio::test]
async fn init_lists_the_templates_and_separately_the_demos() {
    let o = ratchet(&["init", "--list-templates"]).await;
    assert_eq!(o.code, 0);
    for id in [
        "http-latency",
        "cli-cold-start",
        "test-suite-time",
        "build-size",
        "memory",
        "forecast-backtest",
        "llm-eval",
    ] {
        assert!(o.stdout.contains(id), "{id}");
    }
    assert!(o.stdout.contains("bench/http-latency.mjs"));
    assert!(o.stdout.contains("eval/backtest.mjs"));
    assert!(o.stdout.contains("holdout: hidden request mix"));
    assert!(o.stdout.contains("no holdout"));
    assert!(o.stdout.find("Try a demo").unwrap() > o.stdout.find("llm-eval").unwrap());
    for d in ["demo-search", "demo-cli", "demo-forecast"] {
        assert!(o.stdout.contains(&format!("{d}  [demo]")), "{d}");
    }
    assert!(o.stdout.contains("examples/demo-search"));
}

#[tokio::test]
async fn init_list_packs_is_the_same_listing_and_templates_is_too() {
    let a = ratchet(&["init", "--list-packs"]).await;
    let b = ratchet(&["init", "--list-templates"]).await;
    let c = ratchet(&["templates"]).await;
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(c.stdout, b.stdout);
    let j = ratchet(&["templates", "--json"]).await.json();
    assert_eq!(
        j.as_array().unwrap().len(),
        10,
        "seven templates, three demos"
    );
    assert_eq!(j[0]["id"], "http-latency");
}

#[tokio::test]
async fn init_writes_files_that_pass_validate_for_every_template() {
    for info in templates() {
        let dir = tmp();
        let d = dir.path().to_str().unwrap();
        let o = ratchet(&["init", "--template", &info.id, "--dir", d]).await;
        assert_eq!(o.code, 0, "{}: {}", info.id, o.stderr);
        for f in &info.files {
            assert!(dir.path().join(f).exists(), "{}: {f}", info.id);
        }
        let cfg = dir.path().join("ratchet.yaml");
        let v = ratchet(&["validate", cfg.to_str().unwrap()]).await;
        assert_eq!(v.code, 0, "{}: {}", info.id, v.stderr);
        assert_eq!(v.stderr, "");
        assert!(
            v.stdout.contains(&format!("pack        {}", info.category)),
            "{}",
            v.stdout
        );
        assert!(v.stdout.contains(&format!("(template {})", info.id)));
        assert!(v.stdout.contains("min gain calibrated from the baseline"));
        assert!(v.stdout.contains("repeats calibrated from the baseline"));
        assert!(v.stdout.contains("automatic: ratchet.yaml,"));
        assert!(v.stdout.contains("(estimated)"));
        let c = load_config(&cfg).unwrap();
        assert_eq!(c.agent.instructions, "RATCHET.md");
        assert!(dir.path().join("RATCHET.md").exists());
    }
}

#[tokio::test]
async fn init_honors_metric_direction_and_value_usd() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    let o = ratchet(&[
        "init",
        "--template",
        "http-latency",
        "--dir",
        d,
        "--metric",
        "rps",
        "--direction",
        "maximize",
        "--value-usd",
        "250",
    ])
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    let c = load_config(dir.path().join("ratchet.yaml")).unwrap();
    assert_eq!(c.objective.metric, "rps");
    assert_eq!(c.objective.direction.as_str(), "maximize");
    let v = c.objective.value.unwrap();
    assert_eq!((v.usd_per_unit, v.basis.as_str()), (250.0, "estimated"));
}

#[tokio::test]
async fn init_quotes_a_metric_that_needs_it_and_the_result_still_validates() {
    let dir = tmp();
    let o = ratchet(&[
        "init",
        "--template",
        "forecast-backtest",
        "--dir",
        dir.path().to_str().unwrap(),
        "--metric",
        "wape: p50",
    ])
    .await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    let c = load_config(dir.path().join("ratchet.yaml")).unwrap();
    assert_eq!(c.objective.metric, "wape: p50");
}

#[tokio::test]
async fn init_refuses_to_overwrite_without_force_and_writes_nothing_then() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    std::fs::write(dir.path().join("RATCHET.md"), "mine").unwrap();
    let o = ratchet(&["init", "--template", "cli-cold-start", "--dir", d]).await;
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("refusing to overwrite RATCHET.md"),
        "{}",
        o.stderr
    );
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["RATCHET.md"]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("RATCHET.md")).unwrap(),
        "mine"
    );

    let forced = ratchet(&[
        "init",
        "--template",
        "cli-cold-start",
        "--dir",
        d,
        "--force",
    ])
    .await;
    assert_eq!(forced.code, 0, "{}", forced.stderr);
    assert!(forced.stdout.contains("RATCHET.md (overwritten)"));
    assert!(
        std::fs::read_to_string(dir.path().join("RATCHET.md"))
            .unwrap()
            .contains("Ratchet brief")
    );
}

#[tokio::test]
async fn init_dry_run_prints_the_files_and_writes_nothing() {
    let dir = tmp();
    let o = ratchet(&[
        "init",
        "--template",
        "forecast-backtest",
        "--dir",
        dir.path().to_str().unwrap(),
        "--dry-run",
    ])
    .await;
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("would write eval/backtest.mjs"));
    assert!(o.stdout.contains("pack: forecasting"));
    assert!(o.stdout.contains("template: forecast-backtest"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let j = ratchet(&[
        "init",
        "--template",
        "memory",
        "--dir",
        dir.path().to_str().unwrap(),
        "--dry-run",
        "--json",
    ])
    .await
    .json();
    assert_eq!(j["dryRun"], true);
    assert_eq!(j["template"], "memory");
    assert_eq!(j["files"][0]["path"], "ratchet.yaml");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn init_rejects_bad_input() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    let cases: [(&[&str], &str); 5] = [
        (&["init", "--dir", d], "--template is required"),
        (
            &["init", "--template", "nope", "--dir", d],
            "unknown template \"nope\"",
        ),
        (
            &["init", "--template", "demo-search", "--dir", d],
            "is a demo, not a template",
        ),
        (
            &[
                "init",
                "--template",
                "memory",
                "--dir",
                d,
                "--direction",
                "sideways",
            ],
            "--direction must be",
        ),
        (
            &[
                "init",
                "--template",
                "memory",
                "--dir",
                d,
                "--value-usd",
                "-3",
            ],
            "--value-usd must be a positive number",
        ),
    ];
    for (args, message) in cases {
        let o = ratchet(args).await;
        assert_eq!(o.code, 1, "{args:?}");
        assert!(o.stderr.contains(message), "{args:?}: {}", o.stderr);
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn init_pack_is_a_legacy_alias_and_says_so() {
    let perf = tmp();
    let o = ratchet(&[
        "init",
        "--pack",
        "perf",
        "--dir",
        perf.path().to_str().unwrap(),
    ])
    .await;
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("ratchet init --template cli-cold-start (--pack perf is now the cli-cold-start template)"), "{}", o.stdout);
    assert!(perf.path().join("bench/cold-start.mjs").exists());
    let fc = tmp();
    assert_eq!(
        ratchet(&[
            "init",
            "--pack",
            "forecasting",
            "--dir",
            fc.path().to_str().unwrap()
        ])
        .await
        .code,
        0
    );
    assert!(fc.path().join("eval/backtest.mjs").exists());
    assert!(fc.path().join("src/forecast.mjs").exists());
}

#[tokio::test]
async fn init_template_wins_over_pack_and_a_template_name_works_as_a_pack() {
    let dir = tmp();
    assert_eq!(
        ratchet(&[
            "init",
            "--pack",
            "perf",
            "--template",
            "memory",
            "--dir",
            dir.path().to_str().unwrap()
        ])
        .await
        .code,
        0
    );
    assert!(dir.path().join("bench/memory.mjs").exists());
    let dir2 = tmp();
    assert_eq!(
        ratchet(&[
            "init",
            "--pack",
            "llm-eval",
            "--dir",
            dir2.path().to_str().unwrap()
        ])
        .await
        .code,
        0
    );
    assert!(dir2.path().join("eval/llm-eval.mjs").exists());
}

#[tokio::test]
async fn the_files_a_template_writes_actually_run() {
    if !has_node() {
        eprintln!("skipped: node is not installed");
        return;
    }
    let dir = tmp();
    ratchet(&[
        "init",
        "--template",
        "cli-cold-start",
        "--dir",
        dir.path().to_str().unwrap(),
    ])
    .await;
    let out = std::process::Command::new("node")
        .arg("bench/cold-start.mjs")
        .current_dir(dir.path())
        .env("BENCH_RUNS", "3")
        .env("BENCH_WARMUP", "0")
        .env("BENCH_NO_HYPERFINE", "1")
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["cold_start_ms"]["p50"].as_f64().unwrap() > 0.0);
    assert_eq!(json["error_rate"], 0);

    // forecast-backtest: deterministic, and the baseline model scores a plausible WAPE
    let fc = tmp();
    ratchet(&[
        "init",
        "--template",
        "forecast-backtest",
        "--dir",
        fc.path().to_str().unwrap(),
    ])
    .await;
    let run = || {
        std::process::Command::new("node")
            .arg("eval/backtest.mjs")
            .current_dir(fc.path())
            .output()
            .unwrap()
            .stdout
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b, "deterministic");
    let j: serde_json::Value = serde_json::from_slice(&a).unwrap();
    let wape = j["wape"].as_f64().unwrap();
    assert!(wape > 1.0 && wape < 100.0, "{wape}");
    assert_eq!(j["leakage_violations"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_runs_a_generated_config_end_to_end_and_explains_the_calibration() {
    if !has_node() {
        eprintln!("skipped: node is not installed");
        return;
    }
    let dir = tmp();
    ratchet(&[
        "init",
        "--template",
        "forecast-backtest",
        "--dir",
        dir.path().to_str().unwrap(),
    ])
    .await;
    let cfg = dir.path().join("ratchet.yaml");
    let o = ratchet(&["baseline", cfg.to_str().unwrap(), "--timeout", "60000"]).await;
    assert_eq!(o.stderr, "");
    assert_eq!(o.code, 0);
    assert!(
        o.stdout
            .contains("Baseline: running `node eval/backtest.mjs` 5x"),
        "{}",
        o.stdout
    );
    let re_ok = o
        .stdout
        .contains("calibration this metric is stable (\u{b1}0%); wins must beat 1%");
    assert!(re_ok, "{}", o.stdout);
}

#[tokio::test]
async fn init_program_wiring() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    assert_eq!(
        ratchet(&[
            "init",
            "--template",
            "http-latency",
            "--dir",
            d,
            "--value-usd",
            "5"
        ])
        .await
        .code,
        0
    );
    assert_eq!(
        ratchet(&[
            "validate",
            dir.path().join("ratchet.yaml").to_str().unwrap()
        ])
        .await
        .code,
        0
    );
    assert_eq!(
        ratchet(&["init", "--template", "http-latency", "--dir", d])
            .await
            .code,
        1
    );
    assert_ne!(
        ratchet(&["init", "--template", "http-latency", "--value-usd", "abc"])
            .await
            .code,
        0
    );
    let pack = tmp();
    assert_eq!(
        ratchet(&[
            "init",
            "--pack",
            "perf",
            "--dir",
            pack.path().to_str().unwrap()
        ])
        .await
        .code,
        0
    );
}

// ---- baseline (baseline.test.ts) --------------------------------------------------------------------------------

fn s(p: &std::path::Path) -> String {
    p.to_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_runs_the_repeats_and_prints_stats_and_the_noise_floor() {
    let repo = make_repo(RepoOpts::new(&[100.0, 102.0, 98.0]));
    let o = ratchet(&["baseline", &s(&repo.config)]).await;
    assert_eq!(o.stderr, "");
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("run 1/3: lat.p95 = 100"), "{}", o.stdout);
    assert!(o.stdout.contains("run 3/3: lat.p95 = 98"));
    let has = |label: &str, value: &str| {
        o.stdout.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with(label) && l[label.len()..].split_whitespace().next() == Some(value)
        })
    };
    assert!(has("mean", "100"), "{}", o.stdout);
    assert!(has("median", "100"));
    assert!(has("stddev", "2"));
    assert!(o.stdout.contains("noise floor 4 "), "{}", o.stdout);
    assert!(
        o.stdout.contains("required gain 4 "),
        "max(4, 0.03 * 100 = 3): {}",
        o.stdout
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_repeats_override_the_config() {
    let repo = make_repo(RepoOpts::new(&[10.0, 20.0]));
    let o = ratchet(&["baseline", &s(&repo.config), "--repeats", "4", "--json"]).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    let v = o.json();
    assert_eq!(v["samples"], serde_json::json!([10.0, 20.0, 10.0, 20.0]));
    assert_eq!(v["repeats"], 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_json_is_a_single_document_and_nothing_else_on_stdout() {
    let repo = make_repo(RepoOpts::new(&[100.0, 102.0, 98.0]));
    let o = ratchet(&["baseline", &s(&repo.config), "--json"]).await;
    assert_eq!(o.code, 0);
    let v = o.json();
    assert_eq!(v["metric"], "lat.p95");
    assert_eq!(v["direction"], "minimize");
    assert_eq!(v["repeats"], 3);
    assert_eq!(v["samples"], serde_json::json!([100.0, 102.0, 98.0]));
    assert_eq!(v["noiseFloor"], 4.0);
    assert_eq!(
        v["noiseFloorFormula"],
        serde_json::json!({"k": 2.0, "minAbsolute": 0.0})
    );
    assert_eq!(v["minGain"], 0.03);
    assert_eq!(v["requiredGain"], 4.0);
    let sm = &v["summary"];
    assert_eq!(
        (
            sm["n"].as_u64(),
            sm["mean"].as_f64(),
            sm["median"].as_f64(),
            sm["stddev"].as_f64()
        ),
        (Some(3), Some(100.0), Some(100.0), Some(2.0))
    );
    assert_eq!(
        (sm["min"].as_f64(), sm["max"].as_f64()),
        (Some(98.0), Some(102.0))
    );
    assert!((sm["cv"].as_f64().unwrap() - 0.02).abs() < 1e-12);
    assert!(v["calibration"]["explanation"].is_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_honours_k_and_min_absolute() {
    let repo = make_repo(RepoOpts::new(&[100.0, 102.0, 98.0]));
    let o = ratchet(&["baseline", &s(&repo.config), "--json", "--k", "3"]).await;
    assert_eq!(o.json()["noiseFloor"], 6.0);
    let o = ratchet(&[
        "baseline",
        &s(&repo.config),
        "--json",
        "--min-absolute",
        "10",
    ])
    .await;
    assert_eq!(o.json()["noiseFloor"], 10.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_min_gain_dominates_required_gain_when_runs_are_stable() {
    let repo = make_repo(RepoOpts::new(&[200.0, 200.0, 200.0]));
    let v = ratchet(&["baseline", &s(&repo.config), "--json"])
        .await
        .json();
    assert_eq!(v["noiseFloor"], 0.0);
    assert_eq!(v["requiredGain"], 6.0, "0.03 * 200");
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_runs_in_the_config_directory_and_uses_the_last_json_object() {
    // bench.sh prints a non-JSON line with braces first; counter.txt must land next to the config
    let repo = make_repo(RepoOpts::new(&[5.0]));
    let o = ratchet(&["baseline", &s(&repo.config), "--repeats", "1"]).await;
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(repo.path().join("counter.txt").exists());
    assert!(o.stdout.contains("warning: fewer than 3 runs"));
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_fails_when_the_metric_is_missing_not_a_number_or_there_is_no_json() {
    let repo = make_repo(RepoOpts::new(&[1.0]).metric("nope.p99"));
    let o = ratchet(&["baseline", &s(&repo.config)]).await;
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("run 1/3: metric \"nope.p99\" not found"),
        "{}",
        o.stderr
    );

    let repo = make_repo(RepoOpts::new(&[1.0]).template(r#"{"lat":{"p95":"$V"}}"#));
    let o = ratchet(&["baseline", &s(&repo.config)]).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("not a finite number"), "{}", o.stderr);

    let repo = make_repo(RepoOpts::new(&[1.0]).run("echo hi"));
    let o = ratchet(&["baseline", &s(&repo.config)]).await;
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("no JSON object found on stdout"),
        "{}",
        o.stderr
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_fails_with_the_stderr_tail_when_the_bench_exits_non_zero() {
    let repo = make_repo(RepoOpts::new(&[1.0]).run("echo boom >&2; exit 3"));
    let o = ratchet(&["baseline", &s(&repo.config)]).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("exited with status 3"), "{}", o.stderr);
    assert!(o.stderr.contains("boom"));
}

#[tokio::test(flavor = "multi_thread")]
async fn baseline_times_out_a_hung_run_and_kills_what_it_started() {
    let repo = make_repo(RepoOpts::new(&[1.0]).run("sleep 30"));
    let t = std::time::Instant::now();
    let o = ratchet(&["baseline", &s(&repo.config), "--timeout", "300"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("timed out after 300ms"), "{}", o.stderr);
    assert!(
        t.elapsed() < std::time::Duration::from_secs(10),
        "the hung process was killed, not waited for"
    );
}

#[tokio::test]
async fn baseline_rejects_an_invalid_config_before_running_anything() {
    let o = ratchet(&["baseline", "/nonexistent/ratchet.yaml"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("cannot find"), "{}", o.stderr);
}

#[tokio::test]
async fn baseline_rejects_invalid_repeats() {
    // through the library entry point (clap already refuses 0 on the command line)
    let repo = make_repo(RepoOpts::new(&[1.0]));
    let io = ratchet_cli::io::BufferIo::new();
    let args = ratchet_cli::cli::BaselineArgs {
        path: repo.config.clone(),
        repeats: Some(0),
        k: None,
        min_absolute: None,
        timeout: None,
    };
    assert_eq!(
        ratchet_cli::commands::baseline::run_baseline(&args, false, &io).await,
        1
    );
    assert!(io.stderr().contains("--repeats must be a positive integer"));
    let o = ratchet(&["baseline", &s(&repo.config), "--repeats", "0"]).await;
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("--repeats"), "{}", o.stderr);
}

// ---- program (program.test.ts) -----------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn program_validate_and_baseline_wiring() {
    let repo = make_repo(RepoOpts::new(&[1.0, 2.0, 3.0]));
    let o = ratchet(&["validate", &s(&repo.config)]).await;
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("OK"));
    let o = ratchet(&["baseline", &s(&repo.config), "--repeats", "3", "--json"]).await;
    assert_eq!(o.code, 0);
    assert_eq!(o.json()["samples"], serde_json::json!([1.0, 2.0, 3.0]));
    let bad = make_repo(RepoOpts::new(&[1.0]).metric("missing"));
    assert_eq!(ratchet(&["baseline", &s(&bad.config)]).await.code, 1);
}

#[tokio::test]
async fn program_rejects_bad_option_values_and_unknown_commands() {
    let repo = make_repo(RepoOpts::new(&[1.0]));
    let o = ratchet(&["baseline", &s(&repo.config), "--repeats", "abc"]).await;
    assert_ne!(o.code, 0);
    assert!(o.stderr.contains("--repeats"), "{}", o.stderr);
    assert_ne!(ratchet(&["frobnicate"]).await.code, 0);
    let help = ratchet(&["--help"]).await;
    assert_eq!(help.code, 0);
    for cmd in [
        "validate", "baseline", "init", "run", "status", "stop", "report", "events",
    ] {
        assert!(
            help.stdout.contains(cmd),
            "--help does not list {cmd}:\n{}",
            help.stdout
        );
    }
    assert!(help.stdout.contains("--json"));
    let v = ratchet(&["--version"]).await;
    assert_eq!(v.code, 0);
    assert!(v.stdout.starts_with("ratchet "));
}
