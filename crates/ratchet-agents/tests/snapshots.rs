//! Snapshots of the text and JSON this crate produces: what the agent reads, what goes on the wire, what the UI shows.
//! Review changes with `cargo insta review` (or `INSTA_UPDATE=always cargo test` and read the diff).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::{FakeCompute, MemoryStore, base_config_with, fixture};
use ratchet_agents::domain::{CandidateVerdict, RunnerControl, RunnerHandle};
use ratchet_agents::{
    ClaudeStreamParser, CodexStreamParser, ContextInput, DetectOptions, HistoryEntry, ResumeInput,
    SsmRunnerControl, SsmRunnerControlOptions, build_context, build_initial_prompt,
    build_resume_message, detect_harnesses,
};

fn config() -> ratchet_agents::RunnerConfig {
    base_config_with("/nowhere.git", |c| {
        c["deadlineAt"] = 13_500_000.into();
        c["budgetUsd"] = 20.into();
    })
}

fn history() -> Vec<HistoryEntry> {
    vec![
        HistoryEntry {
            n: 1,
            sha: "abcdef1234567890".into(),
            verdict: CandidateVerdict::Blocked,
            reasons: vec!["touches protected path bench/run.sh".into()],
            best: 20.0,
            at: 0,
        },
        HistoryEntry {
            n: 2,
            sha: "1111111122222222".into(),
            verdict: CandidateVerdict::Verified,
            reasons: vec![],
            best: 12.5,
            at: 0,
        },
    ]
}

#[test]
fn context_md() {
    let c = config();
    let text = build_context(&ContextInput {
        config: &c,
        best_value: 12.5,
        history: &history(),
        now: 10_000_000,
        spend_usd: 4.5,
        turn: 3,
        best_sha: Some("11111111"),
    });
    insta::assert_snapshot!(text);
}

#[test]
fn initial_prompt() {
    let c = config();
    let context = build_context(&ContextInput {
        config: &c,
        best_value: 20.0,
        history: &[],
        now: 10_000_000,
        spend_usd: 0.0,
        turn: 1,
        best_sha: None,
    });
    insta::assert_snapshot!(build_initial_prompt(&c, &context));
}

#[test]
fn resume_message_after_a_rejected_candidate() {
    let c = config();
    let h = history();
    let entry = HistoryEntry {
        verdict: CandidateVerdict::Rejected,
        reasons: vec!["gain 0.4% below the noise floor 4%".into()],
        ..h[0].clone()
    };
    let notes = vec!["The ratchet best moved (now 11111111).".to_string()];
    let users = vec!["focus on the filter parser".to_string()];
    insta::assert_snapshot!(build_resume_message(&ResumeInput {
        entry: Some(&entry),
        best_value: 12.5,
        config: &c,
        now: 10_000_000,
        spend_usd: 4.5,
        notes: &notes,
        user_messages: &users,
        no_change: false,
    }));
}

#[tokio::test]
async fn ssm_launch_script() {
    let compute = Arc::new(FakeCompute::default());
    let mut runner_env = BTreeMap::new();
    runner_env.insert(
        "RATCHET_ARTIFACTS_BUCKET".to_string(),
        "my-bucket".to_string(),
    );
    runner_env.insert("AWS_REGION".to_string(), "us-east-1".to_string());
    let control = SsmRunnerControl::new(SsmRunnerControlOptions {
        compute: compute.clone(),
        store: Arc::new(MemoryStore::default()),
        runner_command: None,
        runner_env,
        state_dir: None,
    });
    let handle = RunnerHandle {
        optimization_id: "opt1".into(),
        agent_id: "cc".into(),
        instance_id: "i-0abc".into(),
    };
    control.start(&handle, &config()).await.unwrap();
    insta::assert_snapshot!(compute.commands.lock().unwrap()[0].1.script);
}

#[test]
fn runner_config_wire_shape() {
    insta::assert_json_snapshot!(config());
}

#[test]
fn claude_transcript_as_agent_events() {
    let mut parser = ClaudeStreamParser::new(|| 1_000);
    let mut events = parser.push(&fixture("claude-success.jsonl"));
    events.extend(parser.end());
    insta::assert_json_snapshot!(events);
}

#[test]
fn codex_transcript_as_agent_events() {
    let mut parser = CodexStreamParser::new(|| 1_000, None);
    let mut events = parser.push(&fixture("codex-success.jsonl"));
    events.extend(parser.end());
    insta::assert_json_snapshot!(events);
}

#[tokio::test]
async fn harness_hints_for_the_settings_screen() {
    // nothing installed: what the Settings > Agents row says
    let found = detect_harnesses(&DetectOptions {
        env: Some(common::env_of(&[("PATH", "/nonexistent")])),
        home: Some("/nonexistent/home".into()),
        shell: Some("/nonexistent/shell".into()),
        platform: Some(ratchet_agents::Platform::Linux),
        login_path: Some("/nonexistent".into()),
        ..DetectOptions::default()
    })
    .await;
    insta::assert_json_snapshot!(found);
}
