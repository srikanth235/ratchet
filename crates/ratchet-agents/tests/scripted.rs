mod common;

use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, Clock, PortError,
};
use std::sync::Arc;
use std::time::Duration;

use common::{collect, sh, texts, write_file};

use ratchet_agents::scripted::SkipWhen;
use ratchet_agents::{
    AcceleratedClock, AgentScript, ManualClock, ScriptStep, ScriptedAgent, ScriptedAgentOptions,
    StepCondition, price_usage,
};

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("scripted-")
        .tempdir()
        .unwrap();
    for (k, v) in files {
        write_file(dir.path(), k, v);
    }
    sh(dir.path(), &["init", "-q"]);
    dir
}

fn script(attempts: Vec<Vec<ScriptStep>>) -> AgentScript {
    AgentScript::new("s", attempts)
}

fn spec(dir: &tempfile::TempDir) -> AgentSpec {
    AgentSpec {
        workdir: dir.path().to_string_lossy().into_owned(),
        prompt: "p".into(),
        ..AgentSpec::default()
    }
}

const PATCH: &str = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
const A: [(&str, &str); 1] = [("a.txt", "one\ntwo\nthree\n")];

fn message(text: &str) -> ScriptStep {
    ScriptStep::Message { text: text.into() }
}

fn patch(name: &str, skip: Option<StepCondition>) -> ScriptStep {
    ScriptStep::Patch {
        name: Some(name.into()),
        diff: PATCH.into(),
        skip,
    }
}

fn skip_when_two_present() -> StepCondition {
    StepCondition {
        file: "a.txt".into(),
        contains: "TWO".into(),
        skip_when: SkipWhen::Present,
    }
}

fn messages(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.event_type == AgentEventType::Message)
        .map(|e| e.text.clone().unwrap())
        .collect()
}

#[tokio::test]
async fn start_plays_attempt_zero_each_resume_the_next_then_runs_out_of_ideas() {
    let dir = repo(&A);
    let agent = ScriptedAgent::new(script(vec![
        vec![message("first")],
        vec![message("second")],
    ]));
    let cancel = CancellationToken::new();
    let (a, on_a) = collect();
    let mut session = agent
        .start(&spec(&dir), &on_a, Some(&cancel))
        .await
        .unwrap();
    assert_eq!(session.kind, AgentKind::Mock);
    let (b, on_b) = collect();
    agent
        .resume(&mut session, &spec(&dir), "verdict A", &on_b, Some(&cancel))
        .await
        .unwrap();
    let (c, on_c) = collect();
    agent
        .resume(&mut session, &spec(&dir), "verdict B", &on_c, Some(&cancel))
        .await
        .unwrap();
    assert_eq!(messages(&a.lock().unwrap()), vec!["first"]);
    assert_eq!(messages(&b.lock().unwrap()), vec!["second"]);
    assert!(messages(&c.lock().unwrap())[0].contains("run out of ideas"));
    assert_eq!(agent.received(), vec!["verdict A", "verdict B"]);
    let mut unknown = AgentSession {
        id: "nope".into(),
        kind: AgentKind::Mock,
    };
    let err = agent
        .resume(&mut unknown, &spec(&dir), "x", &|_| {}, Some(&cancel))
        .await
        .unwrap_err();
    assert!(matches!(err, PortError::AgentFailed { .. }));
}

#[tokio::test]
async fn applies_patches_with_git_apply_and_reports_one_that_does_not_apply_without_failing() {
    let dir = repo(&A);
    let agent = ScriptedAgent::new(script(vec![vec![patch("p1", None), patch("p2", None)]]));
    let (events, on_event) = collect();
    agent
        .start(&spec(&dir), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    let events = events.lock().unwrap();
    let errors: Vec<&AgentEvent> = events
        .iter()
        .filter(|e| e.event_type == AgentEventType::Error)
        .collect();
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .text
            .as_deref()
            .unwrap()
            .contains("p2 does not apply")
    );
}

#[tokio::test]
async fn skip_conditions_make_patches_idempotent() {
    let dir = repo(&[("a.txt", "one\nTWO\nthree\n")]);
    let agent = ScriptedAgent::new(script(vec![vec![patch(
        "p",
        Some(skip_when_two_present()),
    )]]));
    let (events, on_event) = collect();
    agent
        .start(&spec(&dir), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap();
    let events = events.lock().unwrap();
    assert!(!events.iter().any(|e| e.event_type == AgentEventType::Error));
    assert!(events.iter().any(|e| {
        e.text
            .as_deref()
            .unwrap_or("")
            .contains("already in the tree")
    }));
}

#[tokio::test]
async fn an_attempt_whose_patches_are_all_in_the_tree_is_skipped_within_the_turn_and_leading_git_steps_still_run()
 {
    let dir = repo(&[("a.txt", "one\nTWO\nthree\n")]);
    let agent = ScriptedAgent::new(script(vec![
        vec![
            ScriptStep::Git {
                args: vec!["status".into()],
            },
            patch("p", Some(skip_when_two_present())),
            message("should not be played"),
        ],
        vec![ScriptStep::Write {
            path: "b.txt".into(),
            content: "b".into(),
        }],
    ]));
    let (events, on_event) = collect();
    agent
        .start(&spec(&dir), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap();
    assert!(
        !texts(&events.lock().unwrap())
            .join("|")
            .contains("should not be played")
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "b"
    ); // the next idea ran in the same turn
}

#[tokio::test]
async fn write_edit_delete_with_edit_requiring_exactly_one_match_and_paths_confined_to_the_workdir()
{
    let dir = repo(&[("a.txt", "x x\n"), ("gone.txt", "bye")]);
    let agent = ScriptedAgent::new(script(vec![vec![
        ScriptStep::Write {
            path: "sub/new.txt".into(),
            content: "hello".into(),
        },
        ScriptStep::Edit {
            path: "a.txt".into(),
            find: "x".into(),
            replace: "y".into(),
        }, // two matches -> error, no change
        ScriptStep::Delete {
            path: "gone.txt".into(),
        },
    ]]));
    let (events, on_event) = collect();
    agent
        .start(&spec(&dir), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sub/new.txt")).unwrap(),
        "hello"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "x x\n"
    );
    assert!(!dir.path().join("gone.txt").exists());
    let found_two = events
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.event_type == AgentEventType::Error)
        .and_then(|e| e.text.clone())
        .unwrap_or_default();
    assert!(found_two.contains("found 2"), "{found_two}");
    let escape = ScriptedAgent::new(script(vec![vec![ScriptStep::Write {
        path: "../evil.txt".into(),
        content: "x".into(),
    }]]));
    let err = escape
        .start(&spec(&dir), &|_| {}, Some(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("escapes the workdir"), "{err}");
}

#[tokio::test]
async fn usage_events_carry_the_scripted_tokens_and_a_believable_cost() {
    let dir = repo(&A);
    let agent = ScriptedAgent::new(script(vec![vec![
        ScriptStep::Usage {
            input_tokens: 80_000,
            output_tokens: 4_000,
            cost_usd: None,
        },
        ScriptStep::Usage {
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: Some(9.0),
        },
    ]]));
    let (events, on_event) = collect();
    agent
        .start(&spec(&dir), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap();
    let usage: Vec<AgentUsage> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| e.usage)
        .collect();
    assert_eq!(
        usage[0],
        AgentUsage {
            input_tokens: 80_000,
            output_tokens: 4_000,
            cost_usd: Some(price_usage(80_000, 4_000))
        }
    );
    assert!((price_usage(80_000, 4_000) - 0.3).abs() < 0.01);
    assert_eq!(usage[1].cost_usd, Some(9.0));
}

#[tokio::test]
async fn sleeps_go_through_the_injected_clock_and_abort_interrupts_a_sleep() {
    let dir = repo(&A);
    let clock = Arc::new(ManualClock::new(1000));
    let agent = Arc::new(ScriptedAgent::with_options(
        script(vec![vec![ScriptStep::Sleep { ms: 5000 }, message("woke")]]),
        ScriptedAgentOptions {
            clock: Some(clock.clone()),
            ..ScriptedAgentOptions::default()
        },
    ));
    let (events, on_event) = collect();
    let task = {
        let (agent, spec) = (agent.clone(), spec(&dir));
        tokio::spawn(async move {
            agent
                .start(&spec, &on_event, Some(&CancellationToken::new()))
                .await
        })
    };
    while clock.pending_sleepers() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.text.as_deref() == Some("woke"))
    );
    clock.advance(4999).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.text.as_deref() == Some("woke"))
    );
    clock.advance(1).await;
    task.await.unwrap().unwrap();
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.text.as_deref() == Some("woke"))
            .unwrap()
            .at,
        6000
    );

    let accelerated: Arc<dyn Clock> = Arc::new(AcceleratedClock::new(1.0));
    let slow = Arc::new(ScriptedAgent::with_options(
        script(vec![vec![ScriptStep::Sleep { ms: 3_600_000 }]]),
        ScriptedAgentOptions {
            clock: Some(accelerated),
            ..ScriptedAgentOptions::default()
        },
    ));
    let cancel = CancellationToken::new();
    let q = {
        let (slow, spec, cancel) = (slow.clone(), spec(&dir), cancel.clone());
        tokio::spawn(async move { slow.start(&spec, &|_| {}, Some(&cancel)).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    assert!(q.await.unwrap().unwrap_err().is_cancelled());
}

#[tokio::test]
async fn a_fail_step_crashes_the_turn_with_an_error_event() {
    let dir = repo(&A);
    let agent = ScriptedAgent::new(script(vec![vec![ScriptStep::Fail {
        message: "boom".into(),
    }]]));
    let (events, on_event) = collect();
    let err = agent
        .start(&spec(&dir), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "boom");
    let events = events.lock().unwrap();
    let last = events.last().unwrap();
    assert_eq!(
        (last.event_type, last.text.as_deref()),
        (AgentEventType::Error, Some("boom"))
    );
}

#[tokio::test]
async fn git_steps_run_in_the_workdir() {
    let dir = repo(&A);
    let agent = ScriptedAgent::new(script(vec![vec![ScriptStep::Git {
        args: vec!["no-such-subcommand".into()],
    }]]));
    let (events, on_event) = collect();
    agent
        .start(&spec(&dir), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap();
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.event_type == AgentEventType::Error)
    );
}
