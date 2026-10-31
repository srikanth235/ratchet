//! Port of `packages/github/src/report.test.ts`. The TS tests seeded a SQLite ledger; here the same data is the
//! `ReportInput` the engine will build from the ledger. The expected Markdown and JSON are the TS snapshots, byte for byte
//! (`tests/fixtures/report.md`, `report.json`, extracted from `__snapshots__/report.test.ts.snap`).

use ratchet_core::enums::{
    AgentKind, CandidateVerdict, Direction, OptimizationStatus, SpendCategory,
};
use ratchet_core::ids::AgentId;
use ratchet_core::{BenchStats, gain_percent};
use ratchet_github::build_result_report;
use ratchet_github::report::{
    AgentInfo, BaselineInfo, CandidateInfo, OptimizationInfo, RatchetMove, ReportInput, SpendEvent,
    build_squash_plan_from_report_input,
};

const BENCH_RUN: &str = "k6 run bench/k6/search.js";

fn sha(c: char) -> String {
    c.to_string().repeat(40)
}

/// `value` first, then the other statistics in the given order.
fn stats(pairs: &[(&'static str, f64)]) -> BenchStats {
    let mut it = pairs.iter();
    let (k, v) = it.next().expect("a value");
    assert_eq!(*k, "value");
    it.fold(BenchStats::new(*v), |s, (k, v)| s.with(k, *v))
}

#[allow(clippy::too_many_arguments)]
fn cand(
    id: &str,
    agent: &str,
    branch: &str,
    message: &str,
    verdict: CandidateVerdict,
    bench: Option<f64>,
    confirm: Option<f64>,
    st: Option<BenchStats>,
) -> CandidateInfo {
    CandidateInfo {
        id: id.into(),
        agent_id: agent.into(),
        agent_branch: branch.into(),
        message: message.into(),
        verdict,
        bench_value: bench,
        confirm_value: confirm,
        bench_stats: st,
    }
}

fn ev(
    agent: Option<&str>,
    category: SpendCategory,
    usd: f64,
    estimated: bool,
    at: i64,
) -> SpendEvent {
    SpendEvent {
        agent_id: agent.map(AgentId::from),
        category,
        usd,
        estimated,
        at,
    }
}

/// The `seed()` of the TS test.
fn seed() -> ReportInput {
    let (claude, codex) = ("ratchet/opt1/claude", "ratchet/opt1/codex");
    ReportInput {
        optimization: OptimizationInfo {
            id: "opt1".into(),
            name: Some("Demo search p95".into()),
            repo: "acme/search".into(),
            base_ref: "main".into(),
            metric: "http_req_duration.p95".into(),
            direction: Direction::Minimize,
            status: OptimizationStatus::Draft,
            bench_run: Some(BENCH_RUN.into()),
        },
        best_value: Some(180.0),
        gain_pct: gain_percent(Direction::Minimize, 412.0, 180.0),
        baseline: Some(BaselineInfo {
            value: 412.0,
            noise_floor: 0.02,
            stats: stats(&[
                ("value", 412.0),
                ("p50", 240.0),
                ("p95", 412.0),
                ("p99", 690.0),
                ("mean", 260.0),
            ]),
        }),
        candidates: vec![
            cand(
                "c1",
                "a1",
                claude,
                "Cache tokenizer\n\nbody",
                CandidateVerdict::Verified,
                Some(300.0),
                Some(305.0),
                Some(stats(&[
                    ("value", 300.0),
                    ("p50", 200.0),
                    ("p95", 300.0),
                    ("p99", 500.0),
                    ("mean", 210.0),
                ])),
            ),
            cand(
                "c2",
                "a2",
                codex,
                "Index postings | fast",
                CandidateVerdict::Verified,
                Some(180.0),
                Some(185.0),
                Some(stats(&[
                    ("value", 180.0),
                    ("p50", 150.0),
                    ("p95", 180.0),
                    ("p99", 300.0),
                    ("mean", 160.0),
                ])),
            ),
            cand(
                "c3",
                "a1",
                claude,
                "Tweak",
                CandidateVerdict::Rejected,
                Some(178.0),
                None,
                Some(stats(&[("value", 178.0)])),
            ),
            cand(
                "c4",
                "a1",
                claude,
                "Touch bench",
                CandidateVerdict::Blocked,
                None,
                None,
                None,
            ),
            cand(
                "c5",
                "a2",
                codex,
                "Overfit",
                CandidateVerdict::FailedConfirm,
                None,
                None,
                None,
            ),
        ],
        history: vec![
            RatchetMove {
                candidate_id: "c1".into(),
                agent_id: "a1".into(),
                agent_kind: AgentKind::ClaudeCode,
                sha: sha('a'),
                from_value: 412.0,
                to_value: 300.0,
                gain_pct: gain_percent(Direction::Minimize, 412.0, 300.0).unwrap(),
                at: 211,
            },
            RatchetMove {
                candidate_id: "c2".into(),
                agent_id: "a2".into(),
                agent_kind: AgentKind::Codex,
                sha: sha('b'),
                from_value: 300.0,
                to_value: 180.0,
                gain_pct: gain_percent(Direction::Minimize, 300.0, 180.0).unwrap(),
                at: 411,
            },
        ],
        agents: vec![
            AgentInfo {
                id: "a1".into(),
                kind: AgentKind::ClaudeCode,
                branch: claude.into(),
            },
            AgentInfo {
                id: "a2".into(),
                kind: AgentKind::Codex,
                branch: codex.into(),
            },
        ],
        spend_events: vec![
            // spend before the first win
            ev(Some("a1"), SpendCategory::Tokens, 1.5, false, 100),
            ev(Some("a2"), SpendCategory::Tokens, 0.75, true, 101),
            ev(Some("a1"), SpendCategory::Experiment, 2.25, true, 300),
            ev(Some("a2"), SpendCategory::AgentRunner, 3.0, true, 301),
            ev(None, SpendCategory::BenchRunner, 0.5, true, 600),
        ],
    }
}

#[test]
fn summarizes_the_run_as_data() {
    let data = build_result_report(&seed()).unwrap().data;
    assert_eq!(data.baseline.as_ref().unwrap().value, 412.0);
    assert_eq!(data.best.value, 180.0);
    assert_eq!(data.best.sha, Some(sha('b')));
    assert!((data.gain_pct.unwrap() - 56.31).abs() < 0.05);
    let wins: Vec<_> = data
        .wins
        .iter()
        .map(|w| (w.agent_kind, w.bench_value, w.confirm_value, w.cost_usd))
        .collect();
    assert_eq!(
        wins,
        [
            (AgentKind::ClaudeCode, Some(300.0), Some(305.0), 1.5),
            (AgentKind::Codex, Some(180.0), Some(185.0), 3.75)
        ]
    );
    let v = data.verdicts;
    assert_eq!(
        (
            v.pending,
            v.verified,
            v.rejected,
            v.failed_confirm,
            v.blocked
        ),
        (0, 2, 1, 1, 1)
    );
    assert!((data.spend.total - 8.0).abs() < 1e-5);
    assert!((data.cost_per_verified_win_usd.unwrap() - 4.0).abs() < 1e-5);
    let agents: Vec<_> = data
        .agents
        .iter()
        .map(|a| (a.kind, a.candidates, a.verified))
        .collect();
    assert_eq!(
        agents,
        [(AgentKind::ClaudeCode, 3, 1), (AgentKind::Codex, 2, 1)]
    );
}

#[test]
fn renders_markdown() {
    // the TS snapshot, verbatim
    assert_eq!(
        build_result_report(&seed()).unwrap().markdown,
        include_str!("fixtures/report.md")
    );
}

#[test]
fn renders_json_that_round_trips() {
    let r = build_result_report(&seed()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&r.json).unwrap();
    assert_eq!(parsed, serde_json::to_value(&r.data).unwrap());
    // the TS snapshot, verbatim (key order, number formatting, indentation)
    assert_eq!(r.json, include_str!("fixtures/report.json"));
}

#[test]
fn handles_an_optimization_with_no_wins() {
    let input = ReportInput {
        optimization: OptimizationInfo {
            id: "empty".into(),
            name: None,
            repo: "acme/x".into(),
            base_ref: "main".into(),
            metric: "m".into(),
            direction: Direction::Maximize,
            status: OptimizationStatus::Draft,
            bench_run: None, // the TS config was "not: valid"
        },
        best_value: None,
        gain_pct: None,
        baseline: Some(BaselineInfo {
            value: 10.0,
            noise_floor: 0.0,
            stats: stats(&[("value", 10.0)]),
        }),
        candidates: vec![],
        history: vec![],
        agents: vec![],
        spend_events: vec![],
    };
    let r = build_result_report(&input).unwrap();
    assert!(r.data.wins.is_empty());
    assert_eq!(r.data.cost_per_verified_win_usd, None);
    assert_eq!(r.data.best.value, 10.0); // falls back to the baseline
    assert!(r.markdown.contains("_None._"));
    assert!(r.markdown.contains("git checkout main"));
    assert!(r.markdown.contains("# Ratchet result: empty"));
}

#[test]
fn renders_a_report_without_a_baseline() {
    let mut input = seed();
    input.baseline = None;
    let md = build_result_report(&input).unwrap().markdown;
    assert!(md.contains("_No baseline recorded yet._"));
    assert!(!md.contains("## Percentiles"));
}

#[test]
fn a_move_for_an_unknown_candidate_is_an_error() {
    let mut input = seed();
    input.history[0].candidate_id = "ghost".into();
    assert!(
        build_result_report(&input)
            .unwrap_err()
            .to_string()
            .contains("unknown candidate")
    );
}

#[test]
fn squash_plan_uses_verified_wins_in_ratchet_order() {
    let plan = build_squash_plan_from_report_input(&seed()).unwrap();
    assert_eq!(plan.shas, [sha('a'), sha('b')]);
    assert_eq!(plan.final_sha, sha('b'));
    assert_eq!(
        plan.title,
        "ratchet: http_req_duration.p95 412 → 180 (56.3%, 2 verified wins)"
    );
    assert!(
        plan.message
            .contains("Co-authored-by: Codex <noreply@openai.com>")
    );
}

#[test]
fn renders_a_throughput_gain_as_plus_percent() {
    let input = ReportInput {
        optimization: OptimizationInfo {
            id: "tp".into(),
            name: None,
            repo: "a/b".into(),
            base_ref: "main".into(),
            metric: "rps".into(),
            direction: Direction::Maximize,
            status: OptimizationStatus::Draft,
            bench_run: Some(BENCH_RUN.into()),
        },
        best_value: Some(110.0),
        gain_pct: gain_percent(Direction::Maximize, 100.0, 110.0),
        baseline: Some(BaselineInfo {
            value: 100.0,
            noise_floor: 0.0,
            stats: stats(&[("value", 100.0)]),
        }),
        candidates: vec![cand(
            "c",
            "a",
            "b",
            "x",
            CandidateVerdict::Verified,
            Some(110.0),
            None,
            Some(stats(&[("value", 110.0)])),
        )],
        history: vec![RatchetMove {
            candidate_id: "c".into(),
            agent_id: "a".into(),
            agent_kind: AgentKind::Mock,
            sha: sha('f'),
            from_value: 100.0,
            to_value: 110.0,
            gain_pct: 10.0,
            at: 1,
        }],
        agents: vec![AgentInfo {
            id: "a".into(),
            kind: AgentKind::Mock,
            branch: "b".into(),
        }],
        spend_events: vec![],
    };
    assert!(
        build_result_report(&input)
            .unwrap()
            .markdown
            .contains("rps: 100 → 110 (+10.0%)")
    );
}
