mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    Sandbox, base_config, copy_demo_tree, every_attempt_has_realistic_usage, git_apply,
    patch_order, repo_root, sh,
};
use ratchet_agents::demo_scripts::demo_search_script_dir;
use ratchet_agents::domain::{
    AgentAdapter, AgentEvent, AgentEventType, CandidateVerdict, Clock, RunnerControl, RunnerHandle,
    RunnerMessage,
};
use ratchet_agents::workdir::{PrepareWorkdirOptions, prepare_workdir};
use ratchet_agents::{
    AcceleratedClock, DemoVariant, LocalRunnerControl, LocalRunnerControlOptions, LoopTuning,
    MemoryInbox, RunnerLoopOptions, RunnerStopReason, ScriptStep, ScriptedAgent,
    ScriptedAgentOptions, load_demo_search_script, run_runner_loop,
};

const PROTECTED: [&str; 4] = ["bench/", "ratchet.yaml", "tests/contract/", "SOLUTIONS.md"];

fn demo_dir() -> PathBuf {
    repo_root().join("examples/demo-search")
}

/// Copy examples/demo-search into its own git remote (main + one branch per agent + ratchet/best).
fn make_demo_sandbox(agents: &[&str]) -> Sandbox {
    let tmp = tempfile::Builder::new()
        .prefix("ratchet-demo-")
        .tempdir()
        .unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let remote = dir.join("remote.git");
    let seed = dir.join("seed");
    copy_demo_tree(&demo_dir(), &seed);
    assert!(
        Command::new("git")
            .args(["init", "--quiet", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    sh(&seed, &["init", "--quiet", "--initial-branch=main"]);
    sh(&seed, &["add", "-A"]);
    sh(&seed, &["commit", "--quiet", "-m", "demo-search baseline"]);
    sh(
        &seed,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    let mut push = vec![
        "push".to_string(),
        "--quiet".into(),
        "origin".into(),
        "main".into(),
        "main:refs/heads/ratchet/best".into(),
    ];
    push.extend(
        agents
            .iter()
            .map(|a| format!("main:refs/heads/ratchet/{a}")),
    );
    sh(&seed, &push.iter().map(String::as_str).collect::<Vec<_>>());
    let engine = dir.join("engine");
    assert!(
        Command::new("git")
            .args(["clone", "--quiet"])
            .arg(&remote)
            .arg(&engine)
            .status()
            .unwrap()
            .success()
    );
    Sandbox {
        tmp,
        dir,
        remote,
        engine,
    }
}

fn demo_config(sb: &Sandbox, agent_id: &str) -> ratchet_agents::RunnerConfig {
    let brief = std::fs::read_to_string(demo_dir().join("RATCHET.md")).unwrap();
    let agent = agent_id.to_string();
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 365 * 24 * 3_600_000;
    base_config(sb, move |c| {
        c["agentId"] = agent.clone().into();
        c["branch"] = format!("ratchet/{agent}").into();
        c["kind"] = "mock".into();
        c["deadlineAt"] = deadline.into();
        c["brief"] = brief.into();
        c["config"]["protected"] = serde_json::json!([
            "bench/**",
            "ratchet.yaml",
            "tests/contract/**",
            "SOLUTIONS.md"
        ]);
    })
}

#[derive(Debug, Clone)]
struct Decision {
    verdict: CandidateVerdict,
    reasons: Vec<String>,
}

/// A fake engine for the demo: protected-path check, a structural guardrail, a noise rule, and a shared ratchet best on the remote
/// (`ratchet/best`). A verified candidate that does not descend from the best is cherry-picked onto it.
struct DemoEngine {
    engine: PathBuf,
    state: Mutex<(Vec<Decision>, f64)>,
}

impl DemoEngine {
    fn new(sb: &Sandbox) -> Arc<Self> {
        sh(&sb.engine, &["config", "user.name", "engine"]);
        sh(
            &sb.engine,
            &["config", "user.email", "engine@ratchet.local"],
        );
        Arc::new(Self {
            engine: sb.engine.clone(),
            state: Mutex::new((Vec::new(), 20.0)),
        })
    }

    fn decisions(&self) -> Vec<Decision> {
        self.state.lock().unwrap().0.clone()
    }

    /// An `emit` handler for one agent that answers its commit events via `push`.
    fn handler(
        self: &Arc<Self>,
        agent_id: &str,
        push: impl Fn(RunnerMessage) + Send + Sync + 'static,
    ) -> Box<dyn Fn(&AgentEvent) + Send + Sync> {
        let (engine, agent_id) = (self.clone(), agent_id.to_string());
        Box::new(move |e: &AgentEvent| {
            if e.kind != AgentEventType::Commit {
                return;
            }
            let sha = e.data_str("sha").unwrap().to_string();
            let files: Vec<String> = e.data.as_ref().unwrap()["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f.as_str().unwrap().to_string())
                .collect();
            // serialize decisions: they move the shared best
            let mut state = engine.state.lock().unwrap();
            let (decision, best) = engine.decide(&agent_id, &sha, &files, state.1);
            state.1 = best;
            state.0.push(decision.clone());
            push(RunnerMessage::Verdict {
                sha,
                verdict: decision.verdict,
                reasons: decision.reasons,
                current_best: best,
            });
        })
    }

    fn decide(
        &self,
        _agent_id: &str,
        sha: &str,
        files: &[String],
        best_value: f64,
    ) -> (Decision, f64) {
        let make = |verdict, reasons: Vec<String>| (Decision { verdict, reasons }, best_value);
        let touched: Vec<&String> = files
            .iter()
            .filter(|f| {
                PROTECTED
                    .iter()
                    .any(|p| f.as_str() == *p || f.starts_with(p))
            })
            .collect();
        if !touched.is_empty() {
            return make(
                CandidateVerdict::Blocked,
                vec![format!(
                    "protected path changed: {}",
                    touched
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )],
            );
        }
        sh(&self.engine, &["fetch", "--quiet", "origin"]);
        let search = sh(&self.engine, &["show", &format!("{sha}:src/search.ts")]);
        // structural guardrail (fast): the contract test would fail when the tag weight is zeroed
        if search.contains("TAG_WEIGHT = 0;") {
            return make(
                CandidateVerdict::Blocked,
                vec!["guardrail failed: pnpm test:contract".into()],
            );
        }
        // a candidate whose only change is the equivalent filter(Boolean) rewrite is below the noise floor
        let diff = sh(&self.engine, &["diff", &format!("{sha}~1"), sha]);
        let noise = regex::Regex::new(
            r"\+\s*return q\.toLowerCase\(\)\.split\(.*\)\.filter\(\(t\) => t\.length > 0\)",
        )
        .unwrap();
        if noise.is_match(&diff) && files.len() == 1 {
            return make(
                CandidateVerdict::Rejected,
                vec!["gain 0.6% is below the noise floor (4.1%)".into()],
            );
        }
        // verified: move the shared best (cherry-pick when the candidate does not descend from it)
        let best = sh(&self.engine, &["rev-parse", "origin/ratchet/best"]);
        let new_best = if Command::new("git")
            .args(["merge-base", "--is-ancestor", &best, sha])
            .current_dir(&self.engine)
            .status()
            .unwrap()
            .success()
        {
            sha.to_string()
        } else {
            sh(&self.engine, &["checkout", "--quiet", "--detach", &best]);
            let picked = Command::new("git")
                .args([
                    "cherry-pick",
                    "--allow-empty",
                    "--keep-redundant-commits",
                    sha,
                ])
                .current_dir(&self.engine)
                .output()
                .unwrap();
            if !picked.status.success() {
                sh(&self.engine, &["cherry-pick", "--abort"]);
                return make(
                    CandidateVerdict::Rejected,
                    vec!["does not apply on top of the current best".into()],
                );
            }
            sh(&self.engine, &["rev-parse", "HEAD"])
        };
        sh(
            &self.engine,
            &[
                "push",
                "--quiet",
                "--force",
                "origin",
                &format!("{new_best}:refs/heads/ratchet/best"),
            ],
        );
        let new_value = ((best_value * 0.72 * 100.0).round() / 100.0).max(1.0);
        (
            Decision {
                verdict: CandidateVerdict::Verified,
                reasons: vec![format!("p95 improved; best is now {new_value}")],
            },
            new_value,
        )
    }
}

fn patch_files() -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(demo_search_script_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".patch"))
        .collect();
    files.sort();
    files
}

#[test]
fn demo_search_ships_the_six_real_fixes() {
    assert_eq!(
        patch_files(),
        [
            "01-search-no-deep-clone.patch",
            "02-search-topk-collation-rank.patch",
            "03-filter-memoize-parse.patch",
            "04-search-filter-indexes.patch",
            "05-search-prefix-binary-search.patch",
            "06-filter-tokenizer-no-regex.patch",
        ]
    );
}

#[test]
fn demo_search_patches_01_03_05_06_apply_to_the_current_examples_tree_git_apply_check() {
    for file in patch_files()
        .into_iter()
        .filter(|f| !(f.starts_with("02-") || f.starts_with("04-")))
    {
        let ok = Command::new("git")
            .args(["apply", "--check", "--directory=examples/demo-search"])
            .arg(demo_search_script_dir().join(&file))
            .current_dir(repo_root())
            .status()
            .unwrap()
            .success();
        assert!(ok, "{file} does not apply to examples/demo-search");
    }
}

#[test]
fn demo_search_02_and_04_build_on_01_and_02_so_they_are_checked_cumulatively_in_a_scratch_tree() {
    // apply 01 -> check 02 -> apply 02 -> check 04 on a throwaway copy of the tree
    let sb = make_demo_sandbox(&[]);
    let apply =
        |f: &str, check: bool| git_apply(&sb.engine, &demo_search_script_dir().join(f), check);
    assert!(!apply("02-search-topk-collation-rank.patch", true)); // needs 01 first
    assert!(apply("01-search-no-deep-clone.patch", false));
    assert!(apply("02-search-topk-collation-rank.patch", true));
    assert!(apply("02-search-topk-collation-rank.patch", false));
    assert!(apply("04-search-filter-indexes.patch", true));
}

#[test]
fn demo_search_cc_and_cx_scripts_use_the_same_fixes_in_different_orders_and_include_all_three_kinds_of_bad_attempt()
 {
    let cc = load_demo_search_script(DemoVariant::Cc, None).unwrap();
    let cx = load_demo_search_script(DemoVariant::Cx, None).unwrap();
    for s in [&cc, &cx] {
        let names: std::collections::HashSet<String> = patch_order(s).into_iter().collect();
        assert!(names.len() >= 6, "{names:?}");
        let edits: Vec<(&String, &String)> = s
            .attempts
            .iter()
            .flat_map(|a| a.steps.iter())
            .filter_map(|st| {
                if let ScriptStep::Edit { path, find, .. } = st {
                    Some((path, find))
                } else {
                    None
                }
            })
            .collect();
        assert!(edits.iter().any(|(p, _)| p.starts_with("bench/"))); // protected path
        assert!(
            edits
                .iter()
                .any(|(_, f)| f.as_str() == "const TAG_WEIGHT = 1;")
        ); // contract break
        assert!(edits.iter().any(|(_, f)| f.as_str() == ".filter(Boolean)")); // noise-level no-op
        assert_eq!(s.attempts.last().unwrap().name, "wrap-up");
        assert!(every_attempt_has_realistic_usage(s));
    }
    assert_ne!(patch_order(&cc), patch_order(&cx));
}

async fn play_variant(
    variant: DemoVariant,
) -> (
    ratchet_agents::RunnerLoopResult,
    Arc<DemoEngine>,
    PathBuf,
    Sandbox,
) {
    let name = variant.as_str();
    let sb = make_demo_sandbox(&[name]);
    let config = demo_config(&sb, name);
    let workdir = sb.dir.join("work").join(name);
    prepare_workdir(PrepareWorkdirOptions {
        repo_url: config.repo_url.clone(),
        branch: config.branch.clone(),
        base_ref: config.base_ref.clone(),
        dir: workdir.clone(),
        git: None,
        env: None,
    })
    .await
    .unwrap();
    let inbox = Arc::new(MemoryInbox::new());
    let engine = DemoEngine::new(&sb);
    let answer = {
        let inbox = inbox.clone();
        engine.handler(name, move |m| inbox.push(m))
    };
    let clock: Arc<dyn Clock> = Arc::new(AcceleratedClock::new(5000.0));
    let agent: Arc<dyn AgentAdapter> = Arc::new(ScriptedAgent::with_options(
        load_demo_search_script(variant, None).unwrap(),
        ScriptedAgentOptions {
            clock: Some(clock.clone()),
            ..Default::default()
        },
    ));
    let res = run_runner_loop(RunnerLoopOptions {
        config,
        adapter: agent,
        workdir: workdir.clone(),
        inbox,
        emit: Arc::new(move |e| answer(&e)),
        clock,
        signal: None,
        tuning: LoopTuning {
            max_idle_turns: Some(1),
            ..LoopTuning::default()
        },
    })
    .await;
    (res, engine, workdir, sb)
}

fn verdicts(engine: &DemoEngine) -> Vec<CandidateVerdict> {
    engine.decisions().iter().map(|d| d.verdict).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_search_cc_every_good_attempt_applies_and_the_three_bad_attempts_are_blocked_or_rejected()
 {
    use CandidateVerdict::*;
    let (res, engine, workdir, _sb) = play_variant(DemoVariant::Cc).await;
    assert_eq!(
        verdicts(&engine),
        vec![
            Verified, Blocked, Verified, Rejected, Verified, Blocked, Verified, Verified, Verified
        ]
    );
    let decisions = engine.decisions();
    assert!(
        decisions[1].reasons[0].contains("bench/local.ts"),
        "{:?}",
        decisions[1]
    );
    assert!(
        decisions[5].reasons[0].contains("pnpm test"),
        "{:?}",
        decisions[5]
    );
    assert_eq!(res.reason, RunnerStopReason::Idle);
    // the agent's final tree: all six fixes, bad attempts gone
    let search = std::fs::read_to_string(workdir.join("src/search.ts")).unwrap();
    let filter = std::fs::read_to_string(workdir.join("src/filter.ts")).unwrap();
    for marker in [
        "class TopK",
        "candidatePositions",
        "sortedWordsCache",
        "TAG_WEIGHT = 1;",
    ] {
        assert!(
            search.contains(marker),
            "{marker} missing from src/search.ts"
        );
    }
    assert!(!search.contains("JSON.parse(JSON.stringify"));
    assert!(filter.contains("parseCache") && filter.contains("const CONDITION"));
    assert_eq!(sh(&workdir, &["diff", "--stat", "main", "--", "bench"]), "");
    assert!(res.spend_usd > 0.5, "{}", res.spend_usd);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_search_cx_different_order_same_outcome() {
    use CandidateVerdict::*;
    let (res, engine, workdir, _sb) = play_variant(DemoVariant::Cx).await;
    assert_eq!(
        verdicts(&engine),
        vec![
            Verified, Verified, Blocked, Verified, Rejected, Verified, Blocked, Verified, Verified
        ]
    );
    assert!(
        engine.decisions()[6].reasons[0].contains("bench/k6/search.js"),
        "{:?}",
        engine.decisions()[6]
    );
    assert_eq!(res.reason, RunnerStopReason::Idle);
    assert_eq!(sh(&workdir, &["diff", "--stat", "main", "--", "bench"]), "");
    assert!(
        std::fs::read_to_string(workdir.join("src/search.ts"))
            .unwrap()
            .contains("class TopK")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_search_both_scripted_agents_in_parallel_through_local_runner_control_share_one_ratchet_best()
 {
    let sb = make_demo_sandbox(&["cc", "cx"]);
    let engine = DemoEngine::new(&sb); // structural guardrail only: fast
    let clock: Arc<dyn Clock> = Arc::new(AcceleratedClock::new(5000.0));
    let adapter_clock = clock.clone();
    let mut options = LocalRunnerControlOptions::new(
        sb.dir.join("runners"),
        Arc::new(move |config, _| {
            let variant = if config.agent_id == "cc" {
                DemoVariant::Cc
            } else {
                DemoVariant::Cx
            };
            let script = load_demo_search_script(variant, None).map_err(|e| e.to_string())?;
            Ok(Arc::new(ScriptedAgent::with_options(
                script,
                ScriptedAgentOptions {
                    clock: Some(adapter_clock.clone()),
                    ..Default::default()
                },
            )) as Arc<dyn AgentAdapter>)
        }),
    );
    options.clock = Some(clock);
    options.loop_tuning.max_idle_turns = Some(2);
    options.loop_tuning.retry_delay = Some(Duration::from_millis(1));
    let control = Arc::new(LocalRunnerControl::new(options));
    let handles: Vec<RunnerHandle> = ["cc", "cx"]
        .iter()
        .map(|a| RunnerHandle {
            optimization_id: "opt1".into(),
            agent_id: a.to_string(),
            instance_id: format!("i-{a}"),
        })
        .collect();
    for h in &handles {
        control
            .start(h, &demo_config(&sb, &h.agent_id))
            .await
            .unwrap();
    }
    // engine loop: poll events, answer commits
    let answers: Vec<_> = handles
        .iter()
        .map(|h| {
            let (control, h) = (control.clone(), h.clone());
            engine.handler(&h.agent_id.clone(), move |m| {
                let (control, h) = (control.clone(), h.clone());
                tokio::spawn(async move { control.send(&h, m).await.unwrap() });
            })
        })
        .collect();
    let mut cursors: Vec<Option<String>> = vec![None; handles.len()];
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (control, handles, finished) = (control.clone(), handles.clone(), finished.clone());
        tokio::spawn(async move {
            for h in &handles {
                let _ = control.result(h).await;
            }
            finished.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }
    loop {
        let done = finished.load(std::sync::atomic::Ordering::SeqCst);
        for (i, h) in handles.iter().enumerate() {
            let r = control.poll(h, cursors[i].as_deref()).await.unwrap();
            cursors[i] = Some(r.cursor);
            r.events.iter().for_each(|e| answers[i](e));
        }
        if done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for h in &handles {
        assert_eq!(
            control.result(h).await.unwrap().reason,
            RunnerStopReason::Idle
        );
    }

    let verdicts = verdicts(&engine);
    assert_eq!(
        verdicts
            .iter()
            .filter(|v| **v == CandidateVerdict::Blocked)
            .count(),
        4
    ); // 2 protected-path edits + 2 contract breaks
    assert!(
        verdicts
            .iter()
            .filter(|v| **v == CandidateVerdict::Verified)
            .count()
            >= 6
    );

    // the final ratchet best: every fix present, bad attempts absent
    let final_dir = sb.dir.join("final/best");
    assert!(
        Command::new("git")
            .args(["clone", "--quiet", "--branch", "ratchet/best"])
            .arg(&sb.remote)
            .arg(&final_dir)
            .status()
            .unwrap()
            .success()
    );
    let search = std::fs::read_to_string(final_dir.join("src/search.ts")).unwrap();
    let filter = std::fs::read_to_string(final_dir.join("src/filter.ts")).unwrap();
    for marker in ["class TopK", "candidatePositions", "sortedWordsCache"] {
        assert!(
            search.contains(marker),
            "{marker} missing from the final best"
        );
    }
    assert!(!search.contains("JSON.parse(JSON.stringify") && !search.contains("TAG_WEIGHT = 0"));
    assert!(filter.contains("parseCache") && filter.contains("const CONDITION"));
    assert_eq!(
        sh(
            &final_dir,
            &[
                "diff",
                "--stat",
                "origin/main",
                "--",
                "bench",
                "tests",
                "ratchet.yaml"
            ]
        ),
        ""
    );
    let _ = Path::new("");
}
