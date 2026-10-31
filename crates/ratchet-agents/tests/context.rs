mod common;

use ratchet_agents::domain::{CandidateVerdict, Direction};
use ratchet_agents::{
    ContextInput, HistoryEntry, ResumeInput, build_context, build_initial_prompt,
    build_resume_message, format_duration, improvement_pct,
};

fn config() -> ratchet_agents::RunnerConfig {
    common::base_config_with("/nowhere.git", |c| {
        c["deadlineAt"] = (10_000_000i64 + 3 * 3_600_000 + 25 * 60_000).into();
        c["budgetUsd"] = 20.into();
    })
}

fn entry(over: impl FnOnce(&mut HistoryEntry)) -> HistoryEntry {
    let mut e = HistoryEntry {
        n: 1,
        sha: "abcdef1234567890".into(),
        verdict: CandidateVerdict::Rejected,
        reasons: vec!["gain 0.8% < noise floor 4%".into(), "a|b".into()],
        best: 20.0,
        at: 0,
    };
    over(&mut e);
    e
}

#[test]
fn format_duration_formats_hours_and_minutes_never_negative() {
    assert_eq!(format_duration(3 * 3_600_000 + 25 * 60_000), "3h 25m");
    assert_eq!(format_duration(59 * 60_000), "59m");
    assert_eq!(format_duration(-5), "0m");
}

#[test]
fn improvement_pct_is_positive_when_the_metric_moved_in_the_objective_direction() {
    let c = config();
    assert_eq!(improvement_pct(&c, 20.0, 5.0), "75.0%");
    assert_eq!(improvement_pct(&c, 20.0, 25.0), "-25.0%");
    let mut maximize = c.clone();
    maximize.config.objective.direction = Direction::Maximize;
    assert_eq!(improvement_pct(&maximize, 100.0, 150.0), "50.0%");
    assert_eq!(improvement_pct(&c, 0.0, 5.0), "n/a");
}

fn context_text() -> String {
    let c = config();
    let history = [
        entry(|_| {}),
        entry(|e| {
            e.n = 2;
            e.verdict = CandidateVerdict::Verified;
            e.reasons = vec![];
            e.best = 12.5;
            e.sha = "1111111122222222".into();
        }),
    ];
    build_context(&ContextInput {
        config: &c,
        best_value: 12.5,
        history: &history,
        now: 10_000_000,
        spend_usd: 4.5,
        turn: 3,
        best_sha: Some("11111111"),
    })
}

#[test]
fn build_context_states_the_objective_baseline_and_current_best() {
    let text = context_text();
    assert!(text.contains("`http_req_duration.p95` (minimize)"));
    assert!(text.contains("more than 3.0%"));
    assert!(text.contains("Baseline: 20"));
    assert!(
        text.contains("Current ratchet best: 12.5 (commit 11111111); improvement so far 37.5%")
    );
}

#[test]
fn build_context_shows_remaining_time_and_budget() {
    let text = context_text();
    assert!(text.contains("Time remaining: 3h 25m"));
    assert!(text.contains("Budget remaining: $15.50 of $20.00"));
}

#[test]
fn build_context_lists_the_verdict_history_as_a_table_escaping_pipes() {
    let text = context_text();
    assert!(text.contains("| 1 | abcdef12 | rejected | 20 | gain 0.8% < noise floor 4%; a/b |"));
    assert!(text.contains("| 2 | 11111111 | verified | 12.5 | - |"));
}

#[test]
fn build_context_lists_rules_protected_paths_and_guardrails() {
    let text = context_text();
    assert!(text.contains("`ratchet.yaml`, `bench/**`"));
    assert!(text.contains("lockfiles / install config")); // the automatic protected set, not only `protected:`
    assert!(text.contains("`pnpm test` must pass; `error_rate` < 0.001"));
}

#[test]
fn build_context_tells_the_agent_that_dependencies_are_fixed_unless_the_evaluator_declares_none() {
    let text = context_text();
    assert!(text.contains("Dependencies are fixed"));
    assert!(text.contains("lockfile"));
    let mut none = config();
    none.config.evaluator.deps = ratchet_agents::domain::DepsMode::None;
    let no_deps = build_context(&ContextInput {
        config: &none,
        best_value: 20.0,
        history: &[],
        now: 0,
        spend_usd: 0.0,
        turn: 1,
        best_sha: None,
    });
    assert!(!no_deps.contains("Dependencies are fixed"));
}

#[test]
fn build_context_says_so_when_there_is_no_history_yet() {
    let c = config();
    let text = build_context(&ContextInput {
        config: &c,
        best_value: 20.0,
        history: &[],
        now: 0,
        spend_usd: 0.0,
        turn: 1,
        best_sha: None,
    });
    assert!(text.contains("None yet."));
}

#[test]
fn initial_prompt_is_the_brief_plus_the_context() {
    let p = build_initial_prompt(&config(), "CTX");
    assert!(p.starts_with("# Brief"));
    assert!(p.contains(".ratchet/context.md"));
    assert!(p.trim_end().ends_with("CTX"));
}

#[test]
fn resume_message_for_a_verified_rejected_and_no_change_turn() {
    let c = config();
    let notes = vec!["NOTE".to_string()];
    let users = vec!["hi".to_string()];
    fn input<'a>(
        entry: Option<&'a HistoryEntry>,
        no_change: bool,
        c: &'a ratchet_agents::RunnerConfig,
        notes: &'a [String],
        users: &'a [String],
    ) -> ResumeInput<'a> {
        ResumeInput {
            entry,
            best_value: 12.0,
            config: c,
            now: 10_000_000,
            spend_usd: 1.0,
            notes,
            user_messages: users,
            no_change,
        }
    }
    let verified_entry = entry(|e| {
        e.verdict = CandidateVerdict::Verified;
        e.reasons = vec!["p95 -31%".into()];
    });
    let verified = build_resume_message(&input(Some(&verified_entry), false, &c, &notes, &users));
    assert!(verified.contains("VERIFIED"));
    assert!(verified.contains("- p95 -31%"));
    assert!(verified.contains("The ratchet moved to your commit."));
    assert!(verified.contains("NOTE"));
    assert!(verified.contains("Message from the user: hi"));
    assert!(
        build_resume_message(&input(Some(&entry(|_| {})), false, &c, &notes, &users))
            .contains("refs/ratchet/accepted")
    );
    assert!(
        build_resume_message(&input(None, true, &c, &notes, &users))
            .contains("No change to the repository")
    );
}

#[test]
fn numbers_are_formatted_like_the_ts_helpers() {
    // exact ties round away from zero like Number.prototype.toFixed, not to even
    let c = common::base_config_with("/x", |c| c["budgetUsd"] = 0.625.into());
    let text = build_context(&ContextInput {
        config: &c,
        best_value: 12.3456,
        history: &[],
        now: 0,
        spend_usd: 0.0,
        turn: 1,
        best_sha: None,
    });
    assert!(text.contains("Budget remaining: $0.63 of $0.63"), "{text}");
    assert!(text.contains("Current ratchet best: 12.346;"), "{text}");
}
