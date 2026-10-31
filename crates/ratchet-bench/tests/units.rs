//! Port of `packages/bench/src/units.test.ts`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use ratchet_bench::exec::{OnData, Stream};
use ratchet_bench::pipeline::{evaluate_thresholds, log_key, parse_bench_output};
use ratchet_bench::remote_scripts::{
    BENCH_USER, CANDIDATE_LAUNCHER, CheckoutScriptInput, HOLDOUT_USER, Isolation, Layout,
    StagedArchive, StepKind, StepScriptInput, checkout_script, protected_pcre, step_script,
    wrap_step,
};
use ratchet_bench::shell::sq;
use ratchet_bench::testkit::{alive, instant_clock, tmp};
use ratchet_bench::{
    BenchLog, ProcessOptions, apply_overlay, flatten_numeric, list_files, median_metrics,
    resolve_metric, restore_protected, run_process,
};
use ratchet_core::config::{Comparator, ThresholdGuardrail};
use ratchet_core::ports::{BaselineMetrics, BenchJob, BenchJobPhase, BenchRun};
use ratchet_core::{CandidateId, OptimizationId, matches_glob};
use serde_json::json;

fn map(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

mod flatten {
    use super::*;

    #[test]
    fn flattens_numeric_leaves_to_dotted_keys_skipping_arrays_strings_booleans_and_non_finite() {
        let v = json!({"a": {"b": 1, "c": {"d": 2}}, "e": 3, "s": "x", "t": true, "n": null, "arr": [1]});
        assert_eq!(
            flatten_numeric(&v),
            map(&[("a.b", 1.0), ("a.c.d", 2.0), ("e", 3.0)])
        );
    }

    #[test]
    fn median_metrics_aggregates_per_key_over_the_runs_that_report_it() {
        let runs = [
            map(&[("a", 1.0), ("b", 10.0)]),
            map(&[("a", 3.0)]),
            map(&[("a", 2.0), ("b", 20.0)]),
        ];
        assert_eq!(median_metrics(runs.iter()), map(&[("a", 2.0), ("b", 15.0)]));
    }

    #[test]
    fn resolve_metric_prefers_the_exact_key_then_a_unit_suffix() {
        assert_eq!(
            resolve_metric(&map(&[("peak_rss_mb", 5.0)]), "peak_rss"),
            Some(("peak_rss_mb".into(), 5.0))
        );
        assert_eq!(
            resolve_metric(&map(&[("peak_rss", 1.0), ("peak_rss_mb", 5.0)]), "peak_rss"),
            Some(("peak_rss".into(), 1.0))
        );
        assert_eq!(resolve_metric(&map(&[("x", 1.0)]), "peak_rss"), None);
        assert_eq!(resolve_metric(&map(&[]), "toString"), None);
    }
}

mod parse_bench_output_and_evaluate_thresholds {
    use super::*;

    #[test]
    fn uses_the_last_json_object_the_objective_by_path_and_flattens_everything() {
        let r = parse_bench_output(
            "log\n{\"x\":1}\nmore\n{\"lat\":{\"p95\":4.5},\"rps\":10,\"note\":\"hi\"}\n",
            "lat.p95",
        )
        .expect("ok");
        assert_eq!(r.value, 4.5);
        assert_eq!(r.metrics, map(&[("lat.p95", 4.5), ("rps", 10.0)]));
    }

    #[test]
    fn explains_what_is_wrong() {
        assert!(parse_bench_output("nope", "a").is_err());
        let e = parse_bench_output("{\"a\":\"x\"}", "a").expect_err("not a number");
        assert!(e.contains("not a finite number"), "{e}");
    }

    #[test]
    fn evaluates_against_run_medians_not_means_or_maxima() {
        let g = ThresholdGuardrail {
            metric: "error_rate".into(),
            op: Comparator::Lt,
            value: 0.01,
            relative: false,
            raw: "< 0.01".into(),
        };
        let runs: Vec<BenchRun> = [0.5, 0.001, 0.002]
            .iter()
            .map(|e| BenchRun {
                value: 1.0,
                metrics: map(&[("error_rate", *e)]),
                duration_ms: 1,
            })
            .collect();
        let none: Option<&BaselineMetrics> = None;
        assert!(evaluate_thresholds(&[g], &runs, none, BenchJobPhase::Bench)[0].passed); // median 0.002
    }
}

#[test]
fn log_key_follows_bench_opt_candidate_or_baseline_phase_log_and_sanitizes() {
    let job = |opt: &str, cand: Option<&str>, sha: &str, phase| BenchJob {
        optimization_id: OptimizationId::new(opt),
        candidate_id: cand.map(CandidateId::new),
        sha: sha.into(),
        phase,
        repeats: 1,
        trace: None,
    };
    assert_eq!(
        log_key(&job("o", Some("c1"), "s", BenchJobPhase::Confirm)),
        "bench/o/c1/confirm.log"
    );
    assert_eq!(
        log_key(&job("o", None, "abc", BenchJobPhase::Bench)),
        "bench/o/abc/bench.log"
    );
    assert_eq!(
        log_key(&job("o", Some("x"), "s", BenchJobPhase::Baseline)),
        "bench/o/baseline/baseline.log"
    );
    assert_eq!(
        log_key(&job("../o", Some("../../x"), "s", BenchJobPhase::Bench)),
        "bench/.._o/.._.._x/bench.log"
    );
}

#[test]
fn bench_log_keeps_head_and_tail_when_over_the_cap() {
    let log = BenchLog::with_cap(instant_clock(), 100);
    log.write(&format!(
        "{}{}{}",
        "H".repeat(50),
        "M".repeat(1000),
        "T".repeat(50)
    ));
    let s = log.to_string();
    assert!(s.starts_with(&"H".repeat(50)));
    assert!(s.ends_with(&"T".repeat(50)));
    assert!(s.contains("characters truncated"));
    assert!(s.len() < 300);
}

mod run_process_tests {
    use super::*;

    #[tokio::test]
    async fn captures_stdout_streams_both_and_reports_exit_codes() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = seen.clone();
        let on_data: OnData = Arc::new(move |c, s| {
            sink.lock().expect("lock").push(format!(
                "{}:{}",
                if s == Stream::Stdout {
                    "stdout"
                } else {
                    "stderr"
                },
                c.trim()
            ));
        });
        let mut o = ProcessOptions::new("/");
        o.on_data = Some(on_data);
        let r = run_process("echo out; echo err >&2; exit 3", &o).await;
        assert_eq!(
            (r.exit_code, r.stdout.as_str(), r.timed_out, r.aborted),
            (Some(3), "out\n", false, false)
        );
        let mut got = seen.lock().expect("lock").clone();
        got.sort();
        assert_eq!(got, ["stderr:err", "stdout:out"]);
    }

    #[tokio::test]
    async fn kills_the_whole_process_group_on_timeout_including_grandchildren() {
        let dir = tmp("ratchet-exec-");
        let mut o = ProcessOptions::new(dir.path());
        o.timeout_ms = Some(500);
        let r = run_process(
            &format!(
                "sh -c 'echo $$ > {}/pid; sleep 30' & wait",
                dir.path().display()
            ),
            &o,
        )
        .await;
        assert!(r.timed_out);
        let pid: i32 = fs::read_to_string(dir.path().join("pid"))
            .expect("pid")
            .trim()
            .parse()
            .expect("number");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!alive(pid));
    }

    #[tokio::test]
    async fn clears_stragglers_left_behind_by_a_successful_command() {
        let dir = tmp("ratchet-exec-");
        let o = ProcessOptions::new(dir.path());
        let r = run_process(
            &format!(
                "(sleep 30 & echo $! > {}/pid) ; echo done",
                dir.path().display()
            ),
            &o,
        )
        .await;
        assert_eq!(r.exit_code, Some(0));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let pid: i32 = fs::read_to_string(dir.path().join("pid"))
            .expect("pid")
            .trim()
            .parse()
            .expect("number");
        assert!(!alive(pid));
    }

    #[tokio::test]
    async fn reports_a_missing_cwd_as_a_spawn_error_rather_than_failing() {
        let r = run_process("true", &ProcessOptions::new("/definitely/not/here")).await;
        assert!(r.spawn_error.is_some());
    }
}

mod overlay {
    use super::*;

    fn put(root: &std::path::Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }

    #[test]
    fn never_writes_through_a_candidate_symlink_to_outside_the_checkout() {
        let dir = tmp("ratchet-overlay-");
        let (code, bench, outside) = (
            dir.path().join("code"),
            dir.path().join("bench"),
            dir.path().join("outside"),
        );
        fs::create_dir(&outside).expect("mkdir");
        fs::write(outside.join("precious.txt"), "keep").expect("write");
        put(&bench, "bench/precious.txt", "trusted");
        put(&code, "src/a.txt", "a");
        symlink(&outside, code.join("bench")).expect("symlink");
        let res = apply_overlay(&code, &bench, &["bench/**"], true).expect("overlay");
        assert_eq!(res.copied, ["bench/precious.txt"]);
        assert_eq!(
            fs::read_to_string(outside.join("precious.txt")).expect("read"),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(code.join("bench/precious.txt")).expect("read"),
            "trusted"
        );
    }

    #[test]
    fn restore_protected_detects_and_repairs_edits_deletions_and_symlink_swaps() {
        let dir = tmp("ratchet-overlay-");
        let (code, bench) = (dir.path().join("code2"), dir.path().join("bench2"));
        put(&bench, "bench/a.js", "A");
        put(&bench, "bench/sub/b.js", "B");
        put(&bench, "tests/t.js", "T");
        fs::create_dir(&code).expect("mkdir");
        let globs = ["bench/**", "tests/**"];
        let copied = apply_overlay(&code, &bench, &globs, true)
            .expect("overlay")
            .copied;
        assert!(
            restore_protected(&code, &bench, &copied)
                .expect("ok")
                .is_empty()
        );
        fs::write(code.join("bench/a.js"), "evil").expect("write");
        fs::remove_dir_all(code.join("bench/sub")).expect("rm");
        fs::remove_dir_all(code.join("tests")).expect("rm");
        symlink(dir.path(), code.join("tests")).expect("symlink");
        let mut restored = restore_protected(&code, &bench, &copied).expect("ok");
        restored.sort();
        assert_eq!(restored, ["bench/a.js", "bench/sub/b.js", "tests/t.js"]);
        assert_eq!(
            fs::read_to_string(code.join("bench/a.js")).expect("read"),
            "A"
        );
        assert!(code.join("bench/sub/b.js").exists());
        assert!(
            restore_protected(&code, &bench, &copied)
                .expect("ok")
                .is_empty()
        );
    }

    #[test]
    fn skips_git_and_node_modules_when_listing() {
        let dir = tmp("ratchet-overlay-");
        let root = dir.path().join("list");
        put(&root, "a.txt", "");
        put(&root, "node_modules/x/y.js", "");
        put(&root, ".git/config", "");
        put(&root, "d/e.txt", "");
        assert_eq!(list_files(&root).expect("list"), ["a.txt", "d/e.txt"]);
    }
}

#[cfg(target_os = "linux")] // `grep -P` is GNU
mod remote_script_helpers {
    use super::*;

    #[test]
    fn sq_quotes_anything() {
        assert_eq!(sq("it's"), "'it'\\''s'");
        let nasty = "a'b\"c$d`e\\f";
        let out = Command::new("bash")
            .args(["-c", &format!("printf %s {}", sq(nasty))])
            .output()
            .expect("bash");
        assert_eq!(String::from_utf8_lossy(&out.stdout), nasty);
    }

    #[test]
    fn wrap_step_refuses_a_body_that_contains_the_delimiter() {
        assert!(wrap_step("x\n__RATCHET_STEP_EOF__\ny").is_err());
    }

    #[test]
    fn protected_pcre_grep_p_agrees_with_core_matches_glob_on_the_same_paths() {
        let globs = [
            "bench/**",
            "ratchet.yaml",
            "tests/contract/**",
            "**/*.snap",
            "docs/*.md",
            "src/{a,b}/x?.ts",
            "./lead/",
            "SOLUTIONS.md",
            "[ab]c/**",
        ];
        let paths = [
            "bench/k6/search.js",
            "bench",
            "benchmark/x",
            "ratchet.yaml",
            "sub/ratchet.yaml",
            "tests/contract/a/b.ts",
            "tests/unit/a.ts",
            "x/y/z.snap",
            "z.snap",
            "docs/a.md",
            "docs/sub/a.md",
            "src/a/x1.ts",
            "src/b/x12.ts",
            "src/c/x1.ts",
            "lead/f",
            "lead",
            "SOLUTIONS.md",
            "ac/q",
            "bc/q/r",
            "cc/q",
            "a.b/c",
            "weird name/bench/x",
            ".hidden/bench/x",
            "bench/.dot",
        ];
        let grep = |re: &str, input: &str| -> (bool, Vec<String>) {
            let mut child = Command::new("grep")
                .args(["-zP", re])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("grep");
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(input.as_bytes())
                .expect("write");
            let out = child.wait_with_output().expect("grep output");
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout)
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            )
        };
        let input = format!("{}\0", paths.join("\0"));
        let (_, matched) = grep(&protected_pcre(&globs), &input);
        let expected: Vec<&str> = paths
            .iter()
            .copied()
            .filter(|p| globs.iter().any(|g| matches_glob(p, g)))
            .collect();
        assert_eq!(matched, expected);
        // no globs: matches nothing
        let (ok, _) = grep(&protected_pcre::<&str>(&[]), "a\0");
        assert!(!ok);
    }
}

mod generated_step_scripts_candidate_holdout_split {
    use super::*;

    const HOLDOUT_URI: &str = "s3://sealed/holdout/trace.json";

    fn layout() -> Layout {
        Layout::new("/var/tmp/work/job1")
    }

    fn base(kind: StepKind) -> StepScriptInput {
        let mut i = StepScriptInput::new(
            layout(),
            kind,
            "bench run 1/3",
            "k6 run bench/k6/search.js",
            60_000,
        );
        i.env.insert("RATCHET_PHASE".into(), "confirm".into());
        i
    }

    fn staged(n: &str) -> StagedArchive {
        StagedArchive::new(
            format!("s3://artifacts/bench-inputs/opt/job/{n}.tgz"),
            "a".repeat(64),
            "b".repeat(40),
        )
    }

    fn checkout(isolation: Isolation) -> String {
        let mut i = CheckoutScriptInput::new(
            layout(),
            staged("code"),
            staged("bench"),
            vec!["bench/**".into()],
        );
        i.isolation = isolation;
        checkout_script(&i).expect("script")
    }

    fn confirm() -> String {
        let mut i = base(StepKind::Bench);
        i.holdout = Some(HOLDOUT_URI.into());
        step_script(&i).expect("script")
    }

    fn at(s: &str, needle: &str) -> usize {
        s.find(needle)
            .unwrap_or_else(|| panic!("script contains {needle}"))
    }

    fn syntax(s: &str) {
        let mut child = Command::new("bash")
            .arg("-n")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("bash");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(s.as_bytes())
            .expect("write");
        let out = child.wait_with_output().expect("wait");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn every_variant_is_syntactically_valid_bash() {
        syntax(&confirm());
        syntax(&step_script(&base(StepKind::Bench)).expect("script"));
        let mut i = base(StepKind::Bench);
        i.isolation = Isolation::None;
        i.holdout = Some(HOLDOUT_URI.into());
        syntax(&step_script(&i).expect("script"));
        syntax(&step_script(&base(StepKind::Setup)).expect("script"));
        let mut i = base(StepKind::Guardrail);
        i.isolation = Isolation::None;
        syntax(&step_script(&i).expect("script"));
        syntax(&checkout(Isolation::User));
    }

    #[test]
    fn confirm_bench_step_order_kill_verify_snapshot_then_fetch_the_holdout_run_the_harness_kill_wipe()
     {
        let s = confirm();
        let order = [
            "kill_users\n",
            "apply check",
            "snapshot()",
            "aws s3 cp",
            "chown \"$HU:\" \"$TRACE\"",
            "chmod 600 \"$TRACE\"",
            "export RATCHET_TRACE=\"$TRACE\"",
            "setpriv --reuid=\"$HU\"",
            "\nwipe\n",
        ];
        let positions: Vec<usize> = order.iter().map(|n| at(&s, n)).collect();
        let mut sorted = positions.clone();
        sorted.sort_unstable();
        assert_eq!(positions, sorted);
        assert!(
            s.contains("trap wipe EXIT"),
            "also wiped when the script is killed or fails midway"
        );
        assert!(at(&s, "trap wipe EXIT") < at(&s, "aws s3 cp"));
    }

    #[test]
    fn the_holdout_lives_in_the_0700_holdout_dir_and_is_never_owned_by_or_readable_to_the_candidate_user()
     {
        let s = confirm();
        assert!(s.contains("TRACE='/var/tmp/work/job1/holdout/trace.json'"));
        let lines_with = |cmd: &str| -> Vec<&str> {
            s.lines()
                .filter(|l| l.starts_with(cmd) && l.contains("TRACE"))
                .collect()
        };
        assert_eq!(
            lines_with("chown "),
            ["chown \"$HU:\" \"$TRACE\" || exit 42"]
        );
        assert_eq!(lines_with("chmod "), ["chmod 600 \"$TRACE\" || exit 42"]);
        assert!(!s.contains("chgrp"));
        assert!(s.contains("shred -u"));
    }

    #[test]
    fn the_harness_runs_as_the_holdout_user_from_the_snapshot_and_gets_the_launcher() {
        let s = confirm();
        assert!(s.contains("cd \"$H\""));
        let re = regex::Regex::new(
            r#"setpriv --reuid="\$HU" --regid="\$HU" --clear-groups -- env -u AWS_ACCESS_KEY_ID .*RATCHET_CANDIDATE_EXEC='/usr/local/bin/ratchet-candidate' bash -c 'k6 run bench/k6/search\.js'"#,
        )
        .expect("re");
        assert!(re.is_match(&s));
        // sudo needs setuid: no --no-new-privs for the harness (trusted code); candidate steps keep it
        assert!(!s.contains("--no-new-privs"));
        let mut i = base(StepKind::Setup);
        i.command = "pnpm build".into();
        let setup = step_script(&i).expect("script");
        assert!(
            setup.contains("setpriv --reuid=\"$BU\" --regid=\"$BU\" --clear-groups --no-new-privs")
        );
        assert!(setup.contains("cd \"$CODE\""));
        assert!(!setup.contains("$HU\" --regid"));
    }

    #[test]
    fn the_snapshot_is_root_owned_read_only_setuid_free_and_made_once() {
        let s = confirm();
        for needle in [
            "cp -a",
            "-type f -perm /6000 -exec chmod ug-s",
            "chown -hR root:root",
            "chmod -R go-w,a+rX",
            "[ -d \"$H\" ] || snapshot",
        ] {
            assert!(s.contains(needle), "{needle}");
        }
    }

    #[test]
    fn only_a_confirm_bench_step_carries_the_holdout_location_and_fetch() {
        let mut none_iso = base(StepKind::Bench);
        none_iso.isolation = Isolation::None;
        let others = [
            step_script(&base(StepKind::Bench)).expect("script"),
            step_script(&base(StepKind::Setup)).expect("script"),
            step_script(&base(StepKind::Guardrail)).expect("script"),
            step_script(&none_iso).expect("script"),
            checkout(Isolation::User),
            checkout(Isolation::None),
        ];
        for s in &others {
            for needle in ["RATCHET_TRACE", "sealed", "trace.json", "shred"] {
                assert!(!s.contains(needle), "{needle}");
            }
        }
        // the only s3 reads in the checkout are the two staged archives (the bench-inputs prefix of the artifacts bucket)
        let re = regex::Regex::new(r#"s3://[^'" ]+"#).expect("re");
        let uris: Vec<&str> = re.find_iter(&others[4]).map(|m| m.as_str()).collect();
        assert_eq!(uris, [staged("code").uri, staged("bench").uri]);
    }

    #[test]
    fn refuses_a_holdout_on_a_non_bench_step_and_a_caller_supplied_ratchet_trace() {
        let mut i = base(StepKind::Setup);
        i.holdout = Some(HOLDOUT_URI.into());
        assert!(
            step_script(&i)
                .expect_err("refused")
                .to_string()
                .contains("only ever given to bench steps")
        );
        let mut i = base(StepKind::Guardrail);
        i.holdout = Some(HOLDOUT_URI.into());
        assert!(
            step_script(&i)
                .expect_err("refused")
                .to_string()
                .contains("only ever given")
        );
        let mut i = base(StepKind::Bench);
        i.env.clear();
        i.env.insert("RATCHET_TRACE".into(), "/x".into());
        assert!(
            step_script(&i)
                .expect_err("refused")
                .to_string()
                .contains("RATCHET_TRACE is set by the step script")
        );
    }

    #[test]
    fn quotes_a_hostile_holdout_uri_and_a_hostile_launcher_path() {
        let mut i = base(StepKind::Bench);
        i.holdout = Some("s3://b/k'; touch /pwned; '.json".into());
        i.candidate_launcher = Some("/x'; touch /pwned2; '".into());
        let s = step_script(&i).expect("script");
        syntax(&s);
        assert!(!s.lines().any(|l| l.starts_with("touch /pwned")));
        assert!(s.contains(&format!("URI={}", sq("s3://b/k'; touch /pwned; '.json"))));
        assert!(s.contains(&format!(
            "RATCHET_CANDIDATE_EXEC={}",
            sq("/x'; touch /pwned2; '")
        )));
    }

    #[test]
    fn the_checkout_verifies_both_users_both_imds_rules_the_holdout_permissions_and_the_launcher() {
        let co = checkout(Isolation::User);
        for needle in [
            "id -u \"$BU\"",
            "id -u \"$HU\"",
            "iptables -C OUTPUT -m owner --uid-owner \"$u\" -d 169.254.169.254 -j REJECT",
            "the holdout directory is readable by $BU",
            "chmod 700 \"$W/holdout\"",
            "does not run commands as $BU",
        ] {
            assert!(co.contains(needle), "{needle}");
        }
        assert!(co.contains("LAUNCHER='/usr/local/bin/ratchet-candidate'"));
        // the sealed bucket is not touched while checking out
        assert!(
            !regex::Regex::new(r"sealed|holdout/trace")
                .expect("re")
                .is_match(&co)
        );
    }

    #[test]
    fn constants_line_up_with_the_instance_user_data() {
        assert_eq!(
            [BENCH_USER, HOLDOUT_USER, CANDIDATE_LAUNCHER],
            [
                "ratchet-bench",
                "ratchet-holdout",
                "/usr/local/bin/ratchet-candidate"
            ]
        );
    }
}
