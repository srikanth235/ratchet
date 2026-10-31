//! `packages/engine/src/simplified.test.ts`

use std::sync::atomic::Ordering;

use ratchet_core::bench::CalibrationSource;
use ratchet_core::bench::calibrate::calibration as CAL;
use ratchet_core::enums::Direction;
use ratchet_core::ports::{BenchJobPhase, GitHost as _, NoticeLevel, ProposeTemplateInput};
use ratchet_core::protected_set::ProtectedSource;
use ratchet_engine::EngineErrorCode as C;

use crate::common::*;

const AGENT: &str = "agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.xlarge]
";

/// No repeats, no min_gain, no protected list: everything is derived.
fn auto_yaml() -> String {
    format!(
        "version: 0
objective: {{ metric: latency_ms, direction: minimize }}
evaluator:
  run: node eval/score.mjs
  hardware: c7i.2xlarge
  holdout: s3://sealed/trace.jsonl
guardrails:
  - run: pytest -q
{AGENT}"
    )
}

fn template_yaml() -> String {
    format!(
        "version: 0
objective: {{ metric: http_req_duration.p95, direction: minimize }}
evaluator:
  template: http-latency
  holdout: s3://sealed/trace.jsonl
{AGENT}"
    )
}

const BRANCH: &str = "ratchet/opt-test/claude-code";

fn with_yaml(yaml: String) -> HarnessOptions {
    HarnessOptions {
        config_yaml: Some(yaml),
        ..Default::default()
    }
}

fn cc_only(baseline: Option<Vec<f64>>) -> BeginOptions {
    BeginOptions {
        agents: Some(vec![ClaudeCode]),
        baseline,
        ..Default::default()
    }
}

fn candidate(
    h: &Harness,
    sha: &str,
    files: &[&str],
    bench: Option<&[f64]>,
    confirm: Option<&[f64]>,
) {
    h.git.push(BRANCH, sha, files);
    if let Some(b) = bench {
        h.bench.set(
            sha,
            "bench",
            ScriptedBench::samples(b).with_guardrails(vec![passed("pytest -q")]),
        );
    }
    if let Some(c) = confirm {
        h.bench.set(sha, "confirm", ScriptedBench::samples(c));
    }
}

async fn reason(h: &Harness, id: &str, sha: &str) -> String {
    candidates(h, id)
        .await
        .into_iter()
        .find(|c| c.candidate.sha == sha)
        .unwrap()
        .candidate
        .reasons[0]
        .clone()
}

fn close(a: f64, b: f64, eps: f64) {
    assert!((a - b).abs() < eps, "{a} != {b}");
}

// ---------------------------------------------------------------------------------- auto-calibrated statistics

mod auto_calibrated_statistics {
    use super::*;

    #[tokio::test]
    async fn derives_repeats_and_the_win_threshold_from_the_baseline_and_says_so_in_plain_words() {
        let mut h = Harness::new(with_yaml(auto_yaml())).await;
        let b = h
            .begin(cc_only(Some(vec![100.0, 102.0, 98.0])))
            .await
            .unwrap();
        let id = &b.id;

        // the baseline had no repeats to go by: the default number of runs
        let jobs = h.bench.jobs();
        assert_eq!(
            jobs.iter()
                .find(|j| j.phase == BenchJobPhase::Baseline)
                .unwrap()
                .repeats,
            CAL::BASELINE_REPEATS
        );
        // CV = 2% -> noise +-4%, win threshold 4%, 5 runs per candidate
        let cal = h.engine.calibration(id).await.unwrap().unwrap();
        assert_eq!(cal.baseline_runs, 3);
        assert_eq!(cal.repeats, 5);
        assert_eq!(cal.repeats_source, CalibrationSource::Auto);
        assert_eq!(cal.min_gain, 0.04);
        assert_eq!(cal.min_gain_source, CalibrationSource::Auto);
        assert!(!cal.repeats_capped);
        close(cal.cv.unwrap(), 0.02, 1e-6);
        assert_eq!(
            cal.explanation,
            "this metric is noisy (±4%); wins must beat 4%; each candidate is measured 5 times"
        );

        // the UI hears about it
        let events = h.events();
        let notice = events
            .iter()
            .find_map(|e| match e {
                EngineEvent::Notice {
                    optimization_id,
                    level,
                    text,
                } if text.contains("Calibrated") => {
                    Some((optimization_id.clone(), *level, text.clone()))
                }
                _ => None,
            })
            .expect("a calibration notice");
        assert_eq!(notice.0, oid(id));
        assert_eq!(notice.1, NoticeLevel::Info);
        assert!(notice.2.contains(&cal.explanation));

        // candidates and the confirm run use the calibrated repeats; the ledger row keeps a provisional threshold
        candidate(
            &h,
            "sha-1",
            &["src/search.ts"],
            Some(&[90.0, 90.0, 90.0]),
            Some(&[91.0, 91.0, 91.0]),
        );
        h.until(async || all_decided(&h, id, 1).await, 60).await;
        let jobs = h.bench.jobs();
        let cand_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| j.candidate_id.is_some())
            .map(|j| (j.phase, j.repeats))
            .collect();
        assert_eq!(
            cand_jobs,
            [(BenchJobPhase::Bench, 5), (BenchJobPhase::Confirm, 5)]
        );
        // the baseline's own confirm run
        assert_eq!(
            jobs.iter()
                .find(|j| j.phase == BenchJobPhase::Confirm && j.candidate_id.is_none())
                .unwrap()
                .repeats,
            5
        );
        let opt = h.ledger.get_optimization(&oid(id)).await.unwrap().unwrap();
        assert_eq!(opt.min_gain, CAL::FALLBACK_GAIN);
    }

    #[tokio::test]
    async fn the_calibrated_threshold_decides_verdicts_noise_floor_4_and_4_percent_of_100_is_4_a_gain_of_exactly_4_is_not_a_win()
     {
        let h = Harness::new(with_yaml(auto_yaml())).await;
        let b = h
            .begin(cc_only(Some(vec![100.0, 102.0, 98.0])))
            .await
            .unwrap();
        let id = &b.id;
        assert_eq!(
            h.ledger
                .get_baseline(&oid(id))
                .await
                .unwrap()
                .unwrap()
                .noise_floor,
            4.0
        );
        candidate(
            &h,
            "sha-tie",
            &["src/search.ts"],
            Some(&[96.0, 96.0, 96.0]),
            None,
        );
        candidate(
            &h,
            "sha-win",
            &["src/search.ts"],
            Some(&[95.0, 95.0, 95.0]),
            Some(&[94.0, 94.0, 94.0]),
        );
        h.until(async || all_decided(&h, id, 2).await, 60).await;
        assert_eq!(
            h.verdicts(id).await,
            v(&[("sha-tie", "rejected"), ("sha-win", "verified")])
        );
        assert!(
            reason(&h, id, "sha-tie")
                .await
                .contains("does not exceed required gain 4")
        );
    }

    #[tokio::test]
    async fn a_noisy_baseline_raises_the_threshold_instead_of_crowning_noise() {
        let h = Harness::new(with_yaml(auto_yaml())).await;
        let b = h
            .begin(cc_only(Some(vec![100.0, 130.0, 70.0, 120.0, 80.0])))
            .await
            .unwrap();
        let id = &b.id;
        let cal = h.engine.calibration(id).await.unwrap().unwrap();
        close(cal.cv.unwrap(), 0.255, 5e-4);
        assert_eq!(cal.min_gain, 0.55);
        assert_eq!(cal.repeats, 5);
        assert!(
            cal.explanation
                .starts_with("this metric is very noisy (±51%); wins must beat 55%"),
            "{}",
            cal.explanation
        );
        candidate(
            &h,
            "sha-1",
            &["src/search.ts"],
            Some(&[60.0, 60.0, 60.0]),
            None,
        ); // 40% better than 100, still inside the noise
        h.until(async || all_decided(&h, id, 1).await, 60).await;
        assert_eq!(h.verdicts(id).await, v(&[("sha-1", "rejected")]));
    }

    #[tokio::test]
    async fn explicit_repeats_and_min_gain_in_ratchet_yaml_are_respected_as_they_were() {
        let h = Harness::new(Default::default()).await; // CONFIG_YAML: repeats 3, min_gain 0.03
        let b = h.begin(cc_only(None)).await.unwrap();
        let cal = h.engine.calibration(&b.id).await.unwrap().unwrap();
        assert_eq!(cal.repeats, 3);
        assert_eq!(cal.repeats_source, CalibrationSource::Config);
        assert_eq!(cal.min_gain, 0.03);
        assert_eq!(cal.min_gain_source, CalibrationSource::Config);
        assert!(cal.explanation.contains("(set in ratchet.yaml)"));
        assert!(h.bench.jobs().iter().all(|j| j.repeats == 3));
    }

    #[tokio::test]
    async fn noise_factor_in_ratchet_yaml_sets_the_k_of_the_noise_floor() {
        let yaml = auto_yaml().replace(
            "  hardware: c7i.2xlarge",
            "  hardware: c7i.2xlarge\n  noise_factor: 3",
        );
        let h = Harness::new(with_yaml(yaml)).await;
        let b = h
            .begin(cc_only(Some(vec![100.0, 102.0, 98.0])))
            .await
            .unwrap();
        assert_eq!(
            h.ledger
                .get_baseline(&oid(&b.id))
                .await
                .unwrap()
                .unwrap()
                .noise_floor,
            6.0
        );
        let cal = h.engine.calibration(&b.id).await.unwrap().unwrap();
        assert_eq!(cal.noise_factor, 3.0);
        assert_eq!(cal.noise_factor_source, CalibrationSource::Config);
    }

    #[tokio::test]
    async fn survives_a_restart_and_is_re_derived_from_the_ledger_when_the_remembered_copy_is_missing()
     {
        let mut h = Harness::new(with_yaml(auto_yaml())).await;
        let b = h
            .begin(cc_only(Some(vec![100.0, 102.0, 98.0])))
            .await
            .unwrap();
        let before = h.engine.calibration(&b.id).await.unwrap();
        assert!(before.is_some());
        let e2 = h.restart(false).await;
        assert_eq!(e2.calibration(&b.id).await.unwrap(), before);
        h.ledger
            .kv_delete(&format!("opt:{}:calibration", b.id))
            .await
            .unwrap();
        let e3 = h.restart(false).await;
        assert_eq!(e3.calibration(&b.id).await.unwrap(), before);
    }

    #[tokio::test]
    async fn is_null_before_the_baseline_has_run_and_unknown_ids_are_refused() {
        let h = Harness::new(with_yaml(auto_yaml())).await;
        let id = h
            .engine
            .create_optimization(CreateOptimizationInput {
                repo_url: h.git.remote_url(),
                base_ref: "main".into(),
                agents: vec![ClaudeCode],
                budget_usd: 10.0,
                time_limit_seconds: 3600,
                plateau_evals: 5,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(h.engine.calibration(&id).await.unwrap().is_none());
        assert!(h.engine.calibration("nope").await.is_err());
    }
}

// ---------------------------------------------------------------------------------- automatic protected paths

mod automatic_protected_paths {
    use super::*;

    #[tokio::test]
    async fn blocks_lockfile_and_install_config_changes_without_any_protected_list_before_anything_is_benched()
     {
        let h = Harness::new(with_yaml(auto_yaml())).await;
        let b = h.begin(cc_only(None)).await.unwrap();
        let id = &b.id;
        candidate(&h, "sha-lock", &["src/a.ts", "pnpm-lock.yaml"], None, None);
        candidate(&h, "sha-npmrc", &["packages/api/.npmrc"], None, None);
        candidate(&h, "sha-cfg", &["ratchet.yaml"], None, None);
        candidate(&h, "sha-eval", &["eval/score.mjs"], None, None); // referenced by evaluator.run
        candidate(
            &h,
            "sha-ok",
            &["src/search.ts"],
            Some(&[101.0, 101.0, 101.0]),
            None,
        );
        h.until(async || all_decided(&h, id, 5).await, 60).await;
        assert_eq!(
            h.verdicts(id).await,
            v(&[
                ("sha-lock", "blocked"),
                ("sha-npmrc", "blocked"),
                ("sha-cfg", "blocked"),
                ("sha-eval", "blocked"),
                ("sha-ok", "rejected")
            ])
        );
        assert_eq!(
            reason(&h, id, "sha-lock").await,
            "protected path changed: pnpm-lock.yaml (matches **/pnpm-lock.yaml)"
        );
        assert_eq!(
            reason(&h, id, "sha-eval").await,
            "protected path changed: eval/score.mjs (matches eval/**)"
        );
        // the blocked ones never cost a bench run
        let benched: Vec<_> = h
            .bench
            .jobs()
            .iter()
            .filter(|j| j.candidate_id.is_some())
            .map(|j| j.sha.clone())
            .collect();
        assert_eq!(benched, ["sha-ok"]);
    }

    #[tokio::test]
    async fn a_templates_evaluator_files_are_protected_and_the_users_list_still_adds_to_them() {
        let h = Harness::new(with_yaml(format!(
            "{}protected:\n  - tests/contract/**\n",
            template_yaml()
        )))
        .await;
        let b = h.begin(cc_only(None)).await.unwrap();
        let id = &b.id;
        candidate(&h, "sha-1", &["bench/http-latency.config.json"], None, None);
        candidate(&h, "sha-2", &["tests/contract/api.test.ts"], None, None);
        h.until(async || all_decided(&h, id, 2).await, 60).await;
        assert_eq!(
            h.verdicts(id).await,
            v(&[("sha-1", "blocked"), ("sha-2", "blocked")])
        );
        assert!(reason(&h, id, "sha-1").await.contains("matches bench/**"));
    }

    #[tokio::test]
    async fn protected_paths_id_lists_the_effective_set_with_the_reason_for_each_entry() {
        let h = Harness::new(with_yaml(format!(
            "{}protected:\n  - extra/**\n",
            template_yaml()
        )))
        .await;
        let b = h.begin(cc_only(None)).await.unwrap();
        let p = h.engine.protected_paths(&b.id).await.unwrap();
        for g in [
            "ratchet.yaml",
            "bench/**",
            "**/pnpm-lock.yaml",
            "**/.npmrc",
            "**/pnpm-workspace.yaml",
            "**/.pnpmfile.cjs",
            "**/package-lock.json",
            "**/npm-shrinkwrap.json",
            "extra/**",
        ] {
            assert!(
                p.globs.iter().any(|x| x == g),
                "missing {g} in {:?}",
                p.globs
            );
        }
        assert_eq!(
            p.entries
                .iter()
                .find(|e| e.glob == "extra/**")
                .unwrap()
                .source,
            ProtectedSource::User
        );
        assert_eq!(
            p.entries
                .iter()
                .find(|e| e.glob == "bench/**")
                .unwrap()
                .source,
            ProtectedSource::Evaluator
        );
    }
}

// ------------------------------------------------------------------------------ listTemplates and proposeTemplate

mod templates {
    use super::*;

    fn base(template_id: &str) -> ProposeTemplateInput {
        ProposeTemplateInput {
            repo_url: "https://example.test/acme/repo.git".into(),
            base_ref: "main".into(),
            template_id: template_id.into(),
            metric: None,
            direction: None,
            value_usd: None,
            overwrite: None,
        }
    }

    fn empty_repo(h: &Harness) {
        h.git.files.lock().unwrap().clear();
        h.git.set_file("README.md", "hello\n");
    }

    async fn code(h: &Harness, input: ProposeTemplateInput) -> C {
        h.engine
            .propose_template(&input)
            .await
            .expect_err("expected an error")
            .code
    }

    #[tokio::test]
    async fn lists_the_gallery_the_seven_templates_then_the_demos() {
        let h = Harness::new(Default::default()).await;
        let ids: Vec<_> = h
            .engine
            .list_templates()
            .await
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(
            ids,
            [
                "http-latency",
                "cli-cold-start",
                "test-suite-time",
                "build-size",
                "memory",
                "forecast-backtest",
                "llm-eval",
                "demo-search",
                "demo-cli",
                "demo-forecast"
            ]
        );
    }

    #[tokio::test]
    async fn commits_the_template_on_a_new_branch_and_opens_a_pr_that_says_what_was_added_and_how_to_run_a_baseline()
     {
        let h = Harness::new(Default::default()).await;
        empty_repo(&h);
        let res = h
            .engine
            .propose_template(&base("http-latency"))
            .await
            .unwrap();

        assert_eq!(res.branch, "ratchet/template-http-latency");
        assert_eq!(res.sha, "files-1");
        assert_eq!(res.template_id, "http-latency");
        assert_eq!(res.pr_url, "https://example.test/pr/1");
        assert_eq!(
            res.files,
            [
                "ratchet.yaml",
                "RATCHET.md",
                "bench/http-latency.config.json",
                "bench/http-latency.mjs"
            ]
        );

        let commit = h.git.files_committed.lock().unwrap()[0].0.clone();
        assert_eq!(commit.base_ref, "main");
        assert_eq!(commit.branch, "ratchet/template-http-latency");
        let paths: Vec<_> = commit.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, res.files);
        assert!(commit.files[0].content.contains("template: http-latency"));

        let pr = h.git.pr(0);
        assert_eq!(pr.base, "main");
        assert_eq!(pr.head, "ratchet/template-http-latency");
        assert!(pr.shas.is_empty());
        assert!(pr.title.contains("HTTP latency"), "{}", pr.title);
        assert!(pr.body.contains("What was added"));
        assert!(pr.body.contains("`bench/http-latency.mjs`"));
        assert!(pr.body.contains("Run a baseline"));
        assert!(pr.body.contains("ratchet baseline"));
        let log: Vec<_> = h
            .log
            .all()
            .into_iter()
            .filter(|l| l.starts_with("git.commitFiles") || l == "git.openPullRequest")
            .collect();
        assert_eq!(
            log,
            [
                "git.commitFiles:ratchet/template-http-latency",
                "git.openPullRequest"
            ]
        );
    }

    #[tokio::test]
    async fn passes_the_objective_options_through_and_accepts_a_legacy_pack_name() {
        let h = Harness::new(Default::default()).await;
        empty_repo(&h);
        let mut input = base("forecasting");
        input.metric = Some("mape".into());
        input.direction = Some(Direction::Minimize);
        input.value_usd = Some(250.0);
        let res = h.engine.propose_template(&input).await.unwrap();
        assert_eq!(res.template_id, "forecast-backtest");
        let yaml = h.git.files_committed.lock().unwrap()[0].0.files[0]
            .content
            .clone();
        assert!(yaml.contains("metric: mape"));
        assert!(yaml.contains("usd_per_unit: 250"));
    }

    #[tokio::test]
    async fn refuses_when_files_of_the_template_already_exist_unless_overwrite_is_given() {
        let h = Harness::new(Default::default()).await; // the repo has ratchet.yaml and RATCHET.md
        let err = h
            .engine
            .propose_template(&base("cli-cold-start"))
            .await
            .unwrap_err();
        assert_eq!(err.code, C::InvalidState);
        assert!(
            err.message
                .contains("ratchet.yaml, RATCHET.md already exist on main"),
            "{}",
            err.message
        );
        assert!(h.git.files_committed.lock().unwrap().is_empty());
        assert_eq!(h.git.pr_count(), 0);

        let mut input = base("cli-cold-start");
        input.overwrite = Some(true);
        let ok = h.engine.propose_template(&input).await.unwrap();
        assert!(!ok.pr_url.is_empty());
    }

    #[tokio::test]
    async fn takes_another_branch_name_when_the_first_is_taken_and_gives_up_with_the_original_error_when_that_fails_too()
     {
        let h = Harness::new(Default::default()).await;
        empty_repo(&h);
        h.git
            .existing_branches
            .lock()
            .unwrap()
            .insert("ratchet/template-memory".into());
        let res = h.engine.propose_template(&base("memory")).await.unwrap();
        let suffix = res
            .branch
            .strip_prefix("ratchet/template-memory-")
            .expect("suffixed branch");
        assert!(
            !suffix.is_empty()
                && suffix
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        assert_eq!(h.git.pr(0).head, res.branch);

        h.git.fail_commit_files.store(true, Ordering::SeqCst);
        let err = h
            .engine
            .propose_template(&base("memory"))
            .await
            .unwrap_err();
        assert!(err.message.contains("already exists"), "{}", err.message);
        assert_eq!(h.git.pr_count(), 1);
    }

    #[tokio::test]
    async fn rejects_demos_unknown_templates_bad_options_and_a_host_that_cannot_commit_files() {
        let h = Harness::new(Default::default()).await;
        empty_repo(&h);
        assert_eq!(code(&h, base("demo-search")).await, C::InvalidInput);
        assert_eq!(code(&h, base("nope")).await, C::InvalidInput);
        let mut neg = base("llm-eval");
        neg.value_usd = Some(-1.0);
        assert_eq!(code(&h, neg).await, C::InvalidInput);
        // an unknown direction cannot be expressed in Rust: it is refused when parsed
        assert!(serde_json::from_str::<Direction>("\"sideways\"").is_err());
        let mut no_repo = base("llm-eval");
        no_repo.repo_url = String::new();
        assert_eq!(code(&h, no_repo).await, C::InvalidInput);
        let mut no_ref = base("llm-eval");
        no_ref.base_ref = String::new();
        assert_eq!(code(&h, no_ref).await, C::InvalidInput);

        h.git.can_commit_files.store(false, Ordering::SeqCst);
        assert_eq!(code(&h, base("llm-eval")).await, C::Unavailable);
        assert_eq!(h.git.pr_count(), 0);
    }
}
