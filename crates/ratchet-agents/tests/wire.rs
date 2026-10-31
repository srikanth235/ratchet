//! What the runner reads and writes, as `ratchet-core` types: the JSON shapes of the engine messages and events, and what the
//! agent is told is protected. (These were unit tests of the local stand-ins in `domain.rs` before ratchet-core landed.)

mod common;

use ratchet_agents::ContextInput;
use ratchet_agents::build_context;
use ratchet_core::enums::CandidateVerdict;
use ratchet_core::ports::{AgentEvent, AgentEventType, AgentUsage, RunnerConfig, RunnerMessage};
use ratchet_core::protected_set::evaluator_dirs_from_run;

#[test]
fn evaluator_dirs() {
    assert_eq!(
        evaluator_dirs_from_run("node dist/bench/x.js"),
        vec!["dist/bench/**", "bench/**"]
    );
    assert_eq!(
        evaluator_dirs_from_run("k6 run bench/k6/search.js"),
        vec!["bench/**"]
    );
    assert_eq!(
        evaluator_dirs_from_run("./eval/run.sh && python eval/x.py"),
        vec!["eval/**"]
    );
    assert!(evaluator_dirs_from_run("node ../bench/x.js").is_empty());
    assert!(evaluator_dirs_from_run("echo benchmark/x").is_empty());
}

#[test]
fn runner_message_wire_shape() {
    let m: RunnerMessage = serde_json::from_str(
        r#"{"type":"verdict","sha":"abc","verdict":"failed_confirm","reasons":["x"],"currentBest":1.5}"#,
    )
    .unwrap();
    assert_eq!(
        m,
        RunnerMessage::Verdict {
            sha: "abc".into(),
            verdict: CandidateVerdict::FailedConfirm,
            reasons: vec!["x".into()],
            current_best: 1.5
        }
    );
    assert_eq!(
        serde_json::to_string(&RunnerMessage::Stop).unwrap(),
        r#"{"type":"stop"}"#
    );
    assert_eq!(
        serde_json::from_str::<RunnerMessage>(r#"{"type":"from-the-future"}"#).unwrap(),
        RunnerMessage::Unknown
    );
}

#[test]
fn event_wire_shape_matches_ts() {
    let e = AgentEvent::new(5, AgentEventType::Usage).with_usage(AgentUsage {
        input_tokens: 1,
        output_tokens: 2,
        cost_usd: None,
    });
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"at":5,"type":"usage","usage":{"inputTokens":1,"outputTokens":2}}"#
    );
    // `at` is optional on read (older writers omit it)
    let e: AgentEvent = serde_json::from_str(r#"{"type":"status","text":"one"}"#).unwrap();
    assert_eq!((e.at, e.text.as_deref()), (0, Some("one")));
}

fn context_of(config: &RunnerConfig) -> String {
    build_context(&ContextInput {
        config,
        best_value: 20.0,
        history: &[],
        now: 0,
        spend_usd: 0.0,
        turn: 1,
        best_sha: None,
    })
}

#[test]
fn the_context_lists_the_protect_globs_of_the_evaluator_template() {
    // `data/**` is protected only because the forecast template says so: nothing in `evaluator.run` or `protected:` names it
    let mut plain = common::base_config_with("/nowhere.git", |c| {
        c["config"]["protected"] = serde_json::json!([]);
        c["config"]["evaluator"]["run"] = "node eval/backtest.mjs".into();
    });
    assert!(!context_of(&plain).contains("`data/**`"));

    plain.config.evaluator.template = Some("forecast-backtest".into());
    let text = context_of(&plain);
    let protected = text
        .lines()
        .find(|l| l.starts_with("- Protected paths"))
        .expect("a protected paths line");
    assert!(protected.contains("`ratchet.yaml`"), "{protected}");
    assert!(protected.contains("`eval/**`"), "{protected}");
    assert!(protected.contains("`data/**`"), "{protected}");
}
