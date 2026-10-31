//! `packages/runtime/src/simulation.e2e.test.ts`, plus the same pipeline on the other two demos.
//!
//! Full simulated optimizations on `examples/demo-*`: real git, real bench runs (a built demo and `node`), real runner loop with
//! scripted agents. They take minutes, so — like the TypeScript — they only run on request:
//!
//! ```text
//! pnpm install && (cd examples/demo-search && ../../node_modules/.bin/tsc -p tsconfig.build.json)   # build each demo
//! RATCHET_E2E=1 cargo test -p ratchet-engine --test engine runtime_simulation -- --nocapture
//! ```

use std::sync::{Arc, Mutex};

use ratchet_core::enums::{AgentKind, CandidateVerdict, OptimizationStatus};
use ratchet_core::ports::{AgentEventType, CloudCompute};
use ratchet_core::rows::CandidateWithResults;
use ratchet_engine::EngineSettings;
use ratchet_engine::runtime::simulation::SimulationProject;
use ratchet_engine::runtime::{SimulateOptions, SimulateResult, simulate};

fn enabled() -> bool {
    let on = std::env::var("RATCHET_E2E").as_deref() == Ok("1");
    if !on {
        eprintln!(
            "skipped: set RATCHET_E2E=1 to run the full simulation (needs `pnpm install` and a built demo)"
        );
    }
    on
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn keep_dirs() -> bool {
    std::env::var("RATCHET_E2E_KEEP").is_ok()
}

async fn run(
    project: SimulationProject,
    agents: Vec<AgentKind>,
    attempts: Option<usize>,
    duration: u32,
    repeats: u32,
    minutes: u64,
) -> (SimulateResult, Vec<String>) {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    let mut o = SimulateOptions::new(repo_root());
    o.project = project;
    o.agents = agents;
    o.attempts = attempts;
    o.duration_sec = duration;
    o.repeats = repeats;
    o.minutes = minutes;
    // A small noise multiplier: these tests exercise the pipeline, not the statistics.
    o.settings = EngineSettings {
        noise: ratchet_core::NoiseFloorOptions {
            k: Some(0.25),
            ..Default::default()
        },
        ..Default::default()
    };
    o.log = Some(Arc::new(move |s| {
        eprintln!("{s}");
        sink.lock().unwrap().push(s.to_owned());
    }));
    let r = simulate(o).await.expect("simulate");
    let l = lines.lock().unwrap().clone();
    (r, l)
}

fn dump(lines: &[String]) -> String {
    format!("\n{}", lines.join("\n"))
}

fn cleanup(r: &SimulateResult) {
    if !keep_dirs() {
        let _ = std::fs::remove_dir_all(&r.work_root);
    }
}

/// Everything that must hold on every run, whatever the machine's noise.
async fn assert_invariants(r: &SimulateResult, lines: &[String]) {
    // ended cleanly (teardown moves `stopped` on to `completed`)
    assert!(
        matches!(
            r.summary.status,
            OptimizationStatus::Stopped | OptimizationStatus::Completed
        ),
        "{:?}{}",
        r.summary.status,
        dump(lines)
    );
    // every submitted candidate has a final verdict (the graceful stop drained the queue)
    let pending: Vec<_> = r
        .candidates
        .iter()
        .filter(|c| c.candidate.verdict == CandidateVerdict::Pending)
        .map(|c| format!("{}:{}", c.candidate.agent_id, &c.candidate.sha[..8]))
        .collect();
    assert!(pending.is_empty(), "pending {pending:?}{}", dump(lines));
    // candidates are keyed per (agent, sha): no duplicate pair, even if two agents produced the same commit
    let keys: std::collections::HashSet<_> = r
        .candidates
        .iter()
        .map(|c| format!("{}:{}", c.candidate.agent_id, c.candidate.sha))
        .collect();
    assert_eq!(keys.len(), r.candidates.len());
    // teardown: no tagged instance of this optimization is left alive in the mock cloud
    let oid = ratchet_core::ids::OptimizationId::new(&r.id);
    let live: Vec<_> = r
        .compute
        .list_tagged(Some(&oid))
        .await
        .unwrap()
        .into_iter()
        .filter(|i| i.state != ratchet_core::ports::InstanceState::Terminated)
        .collect();
    assert!(live.is_empty(), "{live:?}");
    let report = r
        .engine
        .teardown_report(&r.id)
        .await
        .unwrap()
        .expect("teardown report");
    assert!(
        report.orphans.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
}

fn is_protected_attempt(c: &CandidateWithResults) -> bool {
    let text = format!(
        "{} {}",
        c.candidate.message.clone(),
        c.candidate.reasons.join(" ")
    )
    .to_lowercase();
    text.contains("protected path") || text.contains("bench load")
}

#[tokio::test]
async fn runs_two_agents_x_2_attempts_to_a_verified_win_drains_every_candidate_opens_a_pr_and_leaves_nothing_running()
 {
    if !enabled() {
        return;
    }
    // Whether a scripted fix beats the noise floor depends on how busy the machine is (2 s load runs, 2 repeats). Everything else
    // is checked on every run; only "no verified win at all" is retried.
    const MAX_RUNS: usize = 3;
    let mut history = Vec::new();
    for attempt in 1..=MAX_RUNS {
        let (r, lines) = run(
            SimulationProject::Search,
            vec![AgentKind::ClaudeCode, AgentKind::Codex],
            Some(2),
            2,
            2,
            12,
        )
        .await;
        assert_invariants(&r, &lines).await;
        assert!(r.candidates.len() >= 2, "{}", dump(&lines));

        // the scripted protected-path attempt (claude-code attempt 2 edits bench/local.ts) is blocked, never benched
        let protected: Vec<_> = r
            .candidates
            .iter()
            .filter(|c| is_protected_attempt(c))
            .collect();
        assert!(!protected.is_empty(), "{}", dump(&lines));
        for c in protected {
            assert_eq!(c.candidate.verdict, CandidateVerdict::Blocked);
            assert!(c.results.is_empty());
        }

        if r.summary.verified_wins < 1 {
            history.push(format!(
                "run {attempt}: no verified win: {}",
                r.candidates
                    .iter()
                    .map(|c| format!(
                        "{}({})",
                        c.candidate.verdict,
                        c.candidate.reasons.first().cloned().unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            cleanup(&r);
            continue;
        }

        // the ratchet only ever moves forward (minimize: strictly lower), each move starting where the last ended
        assert!(!r.history.is_empty());
        assert_eq!(Some(r.history[0].from_value), r.summary.baseline_value);
        for (i, m) in r.history.iter().enumerate() {
            assert!(m.to_value < m.from_value, "move {i}");
            if i > 0 {
                assert_eq!(m.from_value, r.history[i - 1].to_value);
            }
        }
        assert_eq!(
            r.summary.best_value,
            Some(r.history.last().unwrap().to_value)
        );
        assert_eq!(
            r.candidates
                .iter()
                .filter(|c| c.candidate.verdict == CandidateVerdict::Verified)
                .count(),
            r.history.len()
        );

        // PR written to disk
        let pr =
            r.pr.clone()
                .unwrap_or_else(|| panic!("no PR{}", dump(&lines)));
        let path = ratchet_engine::runtime::pr_to_path(&pr);
        assert!(std::path::Path::new(&path).exists(), "{path}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("Ratchet"));
        cleanup(&r);
        return;
    }
    panic!(
        "no verified win in {MAX_RUNS} runs (machine too noisy?)\n{}",
        history.join("\n")
    );
}

// ------------------------------------------------------------------------------------------------ the demos, one agent

/// What `crates/ratchet-agents/scripts/demo-*/README.md` (and the TypeScript scripts it is generated from) say a scripted
/// attempt ends as. Attempts whose gain can sit near the noise floor are `Soft`: they may be verified or rejected depending on
/// the machine, and must be decided either way.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    Verified,
    Blocked,
    Rejected,
    FailedConfirm,
    /// verified or rejected
    Soft,
}

use Expect::*;

impl Expect {
    fn holds(self, v: CandidateVerdict) -> bool {
        match self {
            Verified => v == CandidateVerdict::Verified,
            Blocked => v == CandidateVerdict::Blocked,
            Rejected => v == CandidateVerdict::Rejected,
            FailedConfirm => v == CandidateVerdict::FailedConfirm,
            Soft => matches!(v, CandidateVerdict::Verified | CandidateVerdict::Rejected),
        }
    }
}

/// Run the demo with one agent playing its whole script and check the per-attempt verdicts in submission order.
async fn demo_sequence(project: SimulationProject, kind: AgentKind, expected: &[Expect]) {
    if !enabled() {
        return;
    }
    let (r, lines) = run(project, vec![kind], None, 3, 2, 25).await;
    assert_invariants(&r, &lines).await;
    let got: Vec<CandidateVerdict> = r.candidates.iter().map(|c| c.candidate.verdict).collect();
    eprintln!("{project:?} {kind:?}: {got:?}");
    // every bad attempt is a candidate of its own; attempts that landed no commit (wrap-up) are not candidates
    assert_eq!(got.len(), expected.len(), "{got:?}{}", dump(&lines));
    for (i, (v, e)) in got.iter().zip(expected).enumerate() {
        assert!(
            e.holds(*v),
            "candidate {i}: expected {e:?}, got {v:?}{}",
            dump(&lines)
        );
    }
    assert!(r.summary.verified_wins >= 1, "{}", dump(&lines));
    let _ = AgentEventType::Commit;
    cleanup(&r);
}

// demo-search / cc: 01 deep-clone, protected-path edit + memo, memoize, no-op, top-k, contract break, indexes, prefix (05), tokenizer (06)
#[tokio::test]
async fn demo_search_claude_code_verdicts() {
    demo_sequence(
        SimulationProject::Search,
        AgentKind::ClaudeCode,
        &[
            Verified, Blocked, Verified, Rejected, Verified, Blocked, Verified, Soft, Soft,
        ],
    )
    .await;
}

// demo-search / cx: memoize, deep-clone, contract break, top-k, no-op, prefix (05), protected-path edit + tokenizer, indexes, tokenizer
#[tokio::test]
async fn demo_search_codex_verdicts() {
    demo_sequence(
        SimulationProject::Search,
        AgentKind::Codex,
        &[
            Verified, Verified, Blocked, Verified, Rejected, Soft, Blocked, Verified, Soft,
        ],
    )
    .await;
}

// demo-cli / cc: tz, protected-path edit + tput, catalogs, no-op, dictionary, overfit, config scan, tput
#[tokio::test]
async fn demo_cli_claude_code_verdicts() {
    demo_sequence(
        SimulationProject::Cli,
        AgentKind::ClaudeCode,
        &[
            Soft,
            Blocked,
            Soft,
            Rejected,
            Soft,
            FailedConfirm,
            Soft,
            Soft,
        ],
    )
    .await;
}

// demo-forecast / cc: seasonal naive, protected-path edit + calendar, smoothed level, no-op, promo, leakage, stockout, overfit, pooled
#[tokio::test]
async fn demo_forecast_claude_code_verdicts() {
    demo_sequence(
        SimulationProject::Forecast,
        AgentKind::ClaudeCode,
        &[
            Verified,
            Blocked,
            Verified,
            Rejected,
            Verified,
            Blocked,
            Soft,
            FailedConfirm,
            Soft,
        ],
    )
    .await;
}
