//! `AgentEnvironmentDrafter` (ports of `packages/engine/src/environment/drafter.test.ts`).

#![allow(clippy::await_holding_lock)]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::*;
use ratchet_core::enums::EnvironmentScope::{self, Full, None as NoScope, Single};
use ratchet_core::ports::{AgentAdapter, PortError, PortResult, PriceBook};
use ratchet_env::{
    AgentAnswer, AgentEnvironmentDrafter, AgentEnvironmentDrafterOptions,
    ApproveEnvironmentOptions, EnvErrorKind, EnvironmentApprovalMode, EnvironmentChecker,
    EnvironmentDraftPhase, EnvironmentDraftRecord, EnvironmentDraftStatus, EnvironmentDrafter,
    EnvironmentPlanInput, EnvironmentPlanOutcome, MOCK_CONFIG_YAML, MessageFrom, fixed_git,
    format_agent_answer,
};

struct Checker {
    calls: AtomicUsize,
    outcome: Mutex<PortResult<EnvironmentPlanOutcome>>,
}

impl Checker {
    fn passing() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            outcome: Mutex::new(Ok(EnvironmentPlanOutcome {
                ok: true,
                detail: "tofu plan: 14 to add".into(),
            })),
        })
    }
    fn with(outcome: PortResult<EnvironmentPlanOutcome>) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            outcome: Mutex::new(outcome),
        })
    }
}

#[async_trait]
impl EnvironmentChecker for Checker {
    async fn plan(&self, _input: &EnvironmentPlanInput) -> PortResult<EnvironmentPlanOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcome.lock().unwrap().clone()
    }
}

struct Prices;

#[async_trait]
impl PriceBook for Prices {
    async fn hourly_usd(&self, t: &str) -> PortResult<f64> {
        Ok(if t == "c7i.2xlarge" { 10.0 } else { 0.01 })
    }
}

struct Setup {
    drafter: AgentEnvironmentDrafter,
    git: Arc<FakeGit>,
    errors: Arc<Mutex<Vec<String>>>,
}

#[derive(Default)]
struct Over {
    checker: Option<Arc<Checker>>,
    prices: bool,
    config_yaml: Option<String>,
}

fn setup(agent: Arc<ScriptedAgent>, over: Over) -> Setup {
    let clock = clock();
    let git = FakeGit::new(&[
        (
            "ratchet.yaml",
            over.config_yaml.as_deref().unwrap_or(MOCK_CONFIG_YAML),
        ),
        ("src/app.ts", "export {};\n"),
    ]);
    let errors = Arc::new(Mutex::new(Vec::new()));
    let adapter: Arc<dyn AgentAdapter> = agent;
    let mut o = AgentEnvironmentDrafterOptions::new(
        fixed_git(git.clone()),
        Arc::new(move |_| Ok(adapter.clone())),
        dyn_clock(&clock),
    );
    o.new_id = Some(ids());
    if let Some(c) = over.checker {
        o.checker = Some(c);
    }
    if over.prices {
        o.prices = Some(Arc::new(Prices));
    }
    let e = errors.clone();
    o.on_error = Some(Arc::new(move |err, _| {
        e.lock().unwrap().push(err.message.clone())
    }));
    Setup {
        drafter: AgentEnvironmentDrafter::new(o),
        git,
        errors,
    }
}

fn plan_check(rec: &EnvironmentDraftRecord) -> ratchet_env::EnvironmentCheck {
    rec.draft
        .as_ref()
        .unwrap()
        .checks
        .iter()
        .find(|c| c.id == "plan")
        .unwrap()
        .clone()
}

// ---------------------------------------------------------------------------------------------------------------
// AgentEnvironmentDrafter.draft

// "runs the setup agent on a checkout, validates what it wrote, plans it in the account, prices it"
#[tokio::test]
async fn runs_the_setup_agent_validates_plans_and_prices() {
    let agent = ScriptedAgent::new(vec![writes(Full)]);
    let checker = Checker::passing();
    let s = setup(
        agent.clone(),
        Over {
            checker: Some(checker.clone()),
            ..Default::default()
        },
    );
    let phases = Arc::new(Mutex::new(Vec::new()));
    let p = phases.clone();
    let mut inp = input("cut checkout p95");
    inp.time_limit_seconds = Some(3600.0);
    inp.on_progress = Some(Arc::new(move |e| p.lock().unwrap().push(e.phase)));
    let rec = s.drafter.draft(inp).await.unwrap();
    assert_eq!(rec.status, EnvironmentDraftStatus::Ready);
    assert_eq!(rec.revision, 1);
    assert_eq!(rec.base_ref, "main");
    let d = rec.draft.as_ref().unwrap();
    assert_eq!(d.scope, Full);
    assert_eq!(d.reason, "reason for full");
    assert_eq!(d.agent.as_str(), "claude-code");
    assert_eq!(d.usd_spent, 0.5);
    assert_eq!(d.files[0].path, "ratchet.yaml");
    assert!(d.files.iter().any(|f| f.path == "infra/ratchet/main.tf"));
    assert_eq!(d.resources.len(), 4);
    assert!(d.checks.iter().all(|c| c.ok), "{:?}", d.checks);
    let plan = plan_check(&rec);
    assert!(plan.ok);
    assert_eq!(plan.detail, "tofu plan: 14 to add");
    assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *phases.lock().unwrap(),
        [
            EnvironmentDraftPhase::Reading,
            EnvironmentDraftPhase::Drafting,
            EnvironmentDraftPhase::Drafting,
            EnvironmentDraftPhase::Validating,
            EnvironmentDraftPhase::Done
        ]
    );
    // the brief carries the goal and the contract
    let prompts = agent.prompts.lock().unwrap();
    assert!(prompts[0].contains("cut checkout p95"));
    assert!(prompts[0].contains("infra/ratchet"));
}

// "scope none / single with no OpenTofu need no plan: the standard sandbox is used"
#[tokio::test]
async fn scope_none_and_single_need_no_plan() {
    for scope in [NoScope, Single] {
        let checker = Checker::passing();
        let s = setup(
            ScriptedAgent::new(vec![writes(scope)]),
            Over {
                checker: Some(checker.clone()),
                ..Default::default()
            },
        );
        let rec = s.drafter.draft(input("cut checkout p95")).await.unwrap();
        assert_eq!(rec.draft.as_ref().unwrap().scope, scope);
        assert!(plan_check(&rec).detail.contains("no OpenTofu"));
        assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
        assert!(rec.draft.as_ref().unwrap().checks.iter().all(|c| c.ok));
    }
}

// "does not claim a plan it did not run: no checker -> the plan check fails; a failing validator skips the plan"
#[tokio::test]
async fn does_not_claim_a_plan_it_did_not_run() {
    let s = setup(ScriptedAgent::new(vec![writes(Full)]), Over::default());
    let rec = s.drafter.draft(input("cut checkout p95")).await.unwrap();
    let plan = plan_check(&rec);
    assert!(!plan.ok);
    assert!(plan.detail.contains("connect AWS"));

    let checker = Checker::passing();
    let bad = writes_with(Full, "x", |dir| {
        let p = dir.join("infra/ratchet/main.tf");
        let mut t = std::fs::read_to_string(&p).unwrap();
        t.push_str("\nresource \"aws_security_group_rule\" \"out\" {\n  cidr_blocks = [\"0.0.0.0/0\"]\n  type = \"egress\"\n}\n");
        std::fs::write(p, t).unwrap();
    });
    let r2 = setup(
        ScriptedAgent::new(vec![bad]),
        Over {
            checker: Some(checker.clone()),
            ..Default::default()
        },
    )
    .drafter
    .draft(input("cut checkout p95"))
    .await
    .unwrap();
    assert!(
        r2.draft
            .as_ref()
            .unwrap()
            .checks
            .iter()
            .any(|c| !c.ok && c.id != "plan")
    );
    let plan = plan_check(&r2);
    assert!(!plan.ok);
    assert!(plan.detail.contains("fix the failed checks first"));
    assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
}

// "a checker that throws or rejects the plan fails the plan check (not the draft call)"
#[tokio::test]
async fn a_checker_that_errors_or_rejects_fails_the_plan_check() {
    let throws = Checker::with(Err(PortError::failed("no credentials")));
    let rec = setup(
        ScriptedAgent::new(vec![writes(Full)]),
        Over {
            checker: Some(throws),
            ..Default::default()
        },
    )
    .drafter
    .draft(input("cut checkout p95"))
    .await
    .unwrap();
    assert_eq!(rec.status, EnvironmentDraftStatus::Ready);
    let plan = plan_check(&rec);
    assert!(!plan.ok);
    assert_eq!(plan.detail, "could not run tofu: no credentials");

    let rejects = Checker::with(Ok(EnvironmentPlanOutcome {
        ok: false,
        detail: "Error: invalid subnet".into(),
    }));
    let r2 = setup(
        ScriptedAgent::new(vec![writes(Full)]),
        Over {
            checker: Some(rejects),
            ..Default::default()
        },
    )
    .drafter
    .draft(input("cut checkout p95"))
    .await
    .unwrap();
    let plan = plan_check(&r2);
    assert!(!plan.ok);
    assert_eq!(plan.detail, "Error: invalid subnet");
}

// "a draft whose ratchet.yaml changed anything outside environment: fails the config check"
#[tokio::test]
async fn a_config_change_outside_environment_fails_the_config_check() {
    let agent = ScriptedAgent::new(vec![Box::new(|dir| {
        put(
            dir,
            &sample_files(NoScope, &MOCK_CONFIG_YAML.replace("minimize", "maximize")),
        );
        Ok(format_agent_answer(&AgentAnswer {
            scope: NoScope,
            reason: "r".into(),
        }))
    })]);
    let rec = setup(agent, Over::default())
        .drafter
        .draft(input("g"))
        .await
        .unwrap();
    let c = rec
        .draft
        .unwrap()
        .checks
        .into_iter()
        .find(|c| c.id == "config")
        .unwrap();
    assert!(!c.ok);
    assert!(c.detail.contains("objective"), "{}", c.detail);
}

// "writes the decision itself when the agent forgot environment.scope in ratchet.yaml"
#[tokio::test]
async fn writes_the_decision_itself_when_the_agent_forgot_the_scope() {
    let agent = ScriptedAgent::new(vec![Box::new(|_| {
        Ok(format_agent_answer(&AgentAnswer {
            scope: NoScope,
            reason: "tests only".into(),
        }))
    })]);
    let rec = setup(agent, Over::default())
        .drafter
        .draft(input("g"))
        .await
        .unwrap();
    let d = rec.draft.unwrap();
    assert!(
        d.files[0].content.contains("scope: none"),
        "{}",
        d.files[0].content
    );
    assert!(d.checks.iter().all(|c| c.ok), "{:?}", d.checks);
}

// "changes outside ratchet.yaml and infra/ratchet/ are reported and never part of the draft"
#[tokio::test]
async fn strays_are_reported_and_never_part_of_the_draft() {
    let agent = ScriptedAgent::new(vec![writes_with(Single, "r", |dir| {
        std::fs::write(dir.join("src/app.ts"), "export const x = 1;\n").unwrap();
    })]);
    let rec = setup(agent, Over::default())
        .drafter
        .draft(input("g"))
        .await
        .unwrap();
    assert!(
        !rec.draft
            .as_ref()
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == "src/app.ts")
    );
    assert!(
        rec.messages
            .last()
            .unwrap()
            .text
            .contains("also changed src/app.ts")
    );
}

// "prefers live prices from the price book over the static table"
#[tokio::test]
async fn prefers_live_prices_over_the_static_table() {
    let live = setup(
        ScriptedAgent::new(vec![writes(Full)]),
        Over {
            prices: true,
            checker: Some(Checker::passing()),
            ..Default::default()
        },
    )
    .drafter
    .draft(input("g"))
    .await
    .unwrap();
    let fixed = setup(
        ScriptedAgent::new(vec![writes(Full)]),
        Over {
            checker: Some(Checker::passing()),
            ..Default::default()
        },
    )
    .drafter
    .draft(input("g"))
    .await
    .unwrap();
    assert!(live.draft.unwrap().cost.usd_per_hour > 10.0);
    assert!(fixed.draft.unwrap().cost.usd_per_hour < 5.0);
}

// "fails cleanly (status failed, scratch released, error recorded) when the agent cannot run, there is no config, or no scope can be told"
#[tokio::test]
async fn fails_cleanly() {
    let unavailable = ScriptedAgent::new(vec![writes(NoScope)]);
    unavailable
        .preflight_problems
        .lock()
        .unwrap()
        .push("claude is not logged in".into());
    let a = setup(unavailable, Over::default());
    let rec = a.drafter.draft(input("g")).await.unwrap();
    assert_eq!(rec.status, EnvironmentDraftStatus::Failed);
    assert!(rec.draft.is_none());
    assert!(
        rec.error
            .as_deref()
            .unwrap()
            .contains("cannot draft here: claude is not logged in")
    );
    assert_eq!(a.errors.lock().unwrap().len(), 1);

    let no_config = setup(ScriptedAgent::new(vec![writes(NoScope)]), Over::default());
    no_config.git.files.lock().unwrap().remove("ratchet.yaml");
    let rec = no_config.drafter.draft(input("g")).await.unwrap();
    assert!(
        rec.error
            .as_deref()
            .unwrap()
            .contains("ratchet.yaml not found")
    );

    let silent = setup(
        ScriptedAgent::new(vec![Box::new(|_| Ok("I looked around.".into()))]),
        Over::default(),
    );
    let rec = silent.drafter.draft(input("g")).await.unwrap();
    assert!(
        rec.error
            .as_deref()
            .unwrap()
            .contains("did not say which scope")
    );

    let err = setup(ScriptedAgent::new(vec![writes(NoScope)]), Over::default())
        .drafter
        .draft(input(" "))
        .await
        .unwrap_err();
    assert_eq!(err.kind, EnvErrorKind::InvalidInput);
}

// ---------------------------------------------------------------------------------------------------------------
// AgentEnvironmentDrafter.revise

// "resumes the same agent session with the user message, re-validates, accumulates effort, keeps the old draft when the agent fails"
#[tokio::test]
async fn revise_resumes_the_session_accumulates_effort_and_keeps_the_old_draft_on_failure() {
    let agent = ScriptedAgent::new(vec![
        writes(Full),
        writes_with(Single, "smaller now", |_| {}),
        Box::new(|_| Err("agent crashed".into())),
    ]);
    let s = setup(
        agent.clone(),
        Over {
            checker: Some(Checker::passing()),
            ..Default::default()
        },
    );
    let first = s.drafter.draft(input("g")).await.unwrap();
    let second = s
        .drafter
        .revise(&first.id, "make it one machine")
        .await
        .unwrap();
    assert_eq!(agent.resumed.lock().unwrap().len(), 1);
    assert_eq!(agent.resumed.lock().unwrap()[0].id, "sess-1");
    assert!(agent.prompts.lock().unwrap()[1].contains("make it one machine"));
    assert_eq!(second.status, EnvironmentDraftStatus::Ready);
    assert_eq!(second.revision, 2);
    let d = second.draft.as_ref().unwrap();
    assert_eq!(d.scope, Single);
    assert_eq!(d.reason, "smaller now");
    assert_eq!(d.usd_spent, 1.0);
    let who: Vec<_> = second.messages.iter().map(|m| m.from).collect();
    assert_eq!(
        who,
        [
            MessageFrom::User,
            MessageFrom::Agent,
            MessageFrom::User,
            MessageFrom::Agent
        ]
    );

    let third = s.drafter.revise(&first.id, "break").await.unwrap();
    assert_eq!(third.status, EnvironmentDraftStatus::Ready);
    assert_eq!(third.revision, 2);
    assert!(third.error.as_deref().unwrap().contains("agent crashed"));
    assert_eq!(third.draft.as_ref().unwrap().scope, Single); // the earlier draft is still there
    assert!(
        third
            .messages
            .last()
            .unwrap()
            .text
            .contains("could not make that change: ")
    );
    assert!(
        third
            .messages
            .last()
            .unwrap()
            .text
            .contains("agent crashed")
    );
}

// "refuses an empty message, an unknown draft, a failed draft, and an approved one"
#[tokio::test]
async fn revise_refuses_what_it_cannot_do() {
    let s = setup(ScriptedAgent::new(vec![writes(Single)]), Over::default());
    let rec = s.drafter.draft(input("g")).await.unwrap();
    assert_eq!(
        s.drafter.revise(&rec.id, "  ").await.unwrap_err().kind,
        EnvErrorKind::InvalidInput
    );
    assert_eq!(
        s.drafter.revise("nope", "x").await.unwrap_err().kind,
        EnvErrorKind::NotFound
    );
    s.drafter
        .approve(&rec.id, ApproveEnvironmentOptions::default())
        .await
        .unwrap();
    let err = s.drafter.revise(&rec.id, "x").await.unwrap_err();
    assert_eq!(err.kind, EnvErrorKind::InvalidState);
    assert!(err.message.contains("locked"));
    let failed = setup(
        ScriptedAgent::new(vec![Box::new(|_| Ok("nothing".into()))]),
        Over::default(),
    );
    let f = failed.drafter.draft(input("g")).await.unwrap();
    assert_eq!(
        failed.drafter.revise(&f.id, "x").await.unwrap_err().kind,
        EnvErrorKind::InvalidState
    );
}

// (Rust only) the begin/run halves: a draft is `drafting` between them and a second revision is refused meanwhile
#[tokio::test]
async fn begin_halves_record_a_drafting_status_and_refuse_a_concurrent_revision() {
    let s = setup(
        ScriptedAgent::new(vec![writes(Full), writes(Single)]),
        Over {
            checker: Some(Checker::passing()),
            ..Default::default()
        },
    );
    let begun = s.drafter.begin_draft(input("g")).unwrap();
    assert_eq!(begun.status, EnvironmentDraftStatus::Drafting);
    assert_eq!(begun.revision, 0);
    assert_eq!(
        s.drafter.get(&begun.id).unwrap().status,
        EnvironmentDraftStatus::Drafting
    );
    assert_eq!(
        s.drafter.begin_revise(&begun.id, "x").unwrap_err().kind,
        EnvErrorKind::InvalidState
    );
    let ready = s.drafter.run_draft(&begun.id).await.unwrap();
    assert_eq!(ready.status, EnvironmentDraftStatus::Ready);
    let revising = s.drafter.begin_revise(&ready.id, "smaller").unwrap();
    assert_eq!(revising.status, EnvironmentDraftStatus::Drafting);
    assert_eq!(revising.revision, 1);
    assert_eq!(
        s.drafter.begin_revise(&ready.id, "again").unwrap_err().kind,
        EnvErrorKind::InvalidState
    );
    let done = s.drafter.run_revise(&ready.id).await.unwrap();
    assert_eq!(
        (done.status, done.revision),
        (EnvironmentDraftStatus::Ready, 2)
    );
}

// ---------------------------------------------------------------------------------------------------------------
// AgentEnvironmentDrafter.approve

async fn ready() -> (Setup, EnvironmentDraftRecord) {
    let s = setup(
        ScriptedAgent::new(vec![writes(Full)]),
        Over {
            checker: Some(Checker::passing()),
            ..Default::default()
        },
    );
    let rec = s.drafter.draft(input("g")).await.unwrap();
    (s, rec)
}

// "opens a pull request with the draft files by default, locks the record, and releases the scratch checkout"
#[tokio::test]
async fn approve_opens_a_pull_request_by_default_and_locks() {
    let (s, rec) = ready().await;
    let done = s
        .drafter
        .approve(&rec.id, ApproveEnvironmentOptions::default())
        .await
        .unwrap();
    assert_eq!(done.status, EnvironmentDraftStatus::Approved);
    let a = done.approval.clone().unwrap();
    assert_eq!(a.mode, EnvironmentApprovalMode::Pr);
    assert!(a.locked);
    assert_eq!(a.branch.as_deref(), Some("ratchet/environment-0001"));
    assert!(a.pr_url.as_deref().unwrap().starts_with("https://"));
    let committed = s.git.committed.lock().unwrap();
    let paths: Vec<_> = committed[0]
        .0
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert!(paths.contains(&"ratchet.yaml") && paths.contains(&"infra/ratchet/main.tf"));
    assert!(s.git.prs.lock().unwrap()[0].title.contains("full-stack"));
    drop(committed);
    let again = s
        .drafter
        .approve(&rec.id, ApproveEnvironmentOptions::default())
        .await
        .unwrap(); // idempotent
    assert_eq!(again, done);
    assert_eq!(s.git.prs.lock().unwrap().len(), 1);
}

// "mode commit writes straight onto the base ref (no pull request)"
#[tokio::test]
async fn approve_in_commit_mode_writes_onto_the_base_ref() {
    let (s, rec) = ready().await;
    let done = s
        .drafter
        .approve(
            &rec.id,
            ApproveEnvironmentOptions {
                mode: Some(EnvironmentApprovalMode::Commit),
            },
        )
        .await
        .unwrap();
    let a = done.approval.unwrap();
    assert_eq!((a.mode, a.locked), (EnvironmentApprovalMode::Commit, true));
    let committed = s.git.committed.lock().unwrap();
    assert_eq!(committed[0].0.branch, "main");
    assert!(committed[0].0.update);
    assert!(s.git.prs.lock().unwrap().is_empty());
}

// "refuses a draft whose checks did not all pass (no checker -> the plan did not run)"
#[tokio::test]
async fn approve_refuses_a_draft_whose_checks_did_not_pass() {
    let s = setup(ScriptedAgent::new(vec![writes(Full)]), Over::default());
    let rec = s.drafter.draft(input("g")).await.unwrap();
    let err = s
        .drafter
        .approve(&rec.id, ApproveEnvironmentOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, EnvErrorKind::InvalidState);
    assert!(err.message.contains("Plans cleanly"), "{}", err.message);
    assert!(s.git.committed.lock().unwrap().is_empty());
    assert_eq!(
        s.drafter.get(&rec.id).unwrap().status,
        EnvironmentDraftStatus::Ready
    );
}

// "refuses when ratchet.yaml changed on the base ref since the draft was made"
#[tokio::test]
async fn approve_refuses_when_ratchet_yaml_changed_since_the_draft() {
    let (s, rec) = ready().await;
    s.git
        .files
        .lock()
        .unwrap()
        .get_mut("ratchet.yaml")
        .unwrap()
        .push_str("# edited meanwhile\n");
    let err = s
        .drafter
        .approve(&rec.id, ApproveEnvironmentOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, EnvErrorKind::InvalidState);
    assert!(
        err.message
            .contains("changed on main since this environment was drafted"),
        "{}",
        err.message
    );
    assert!(s.git.committed.lock().unwrap().is_empty());
}

// "an approved draft cannot be discarded; a ready one can"
#[tokio::test]
async fn an_approved_draft_cannot_be_discarded() {
    let (s, rec) = ready().await;
    s.drafter.discard(&rec.id).unwrap();
    assert!(s.drafter.get(&rec.id).is_none());
    let r2 = s.drafter.draft(input("g")).await.unwrap();
    s.drafter
        .approve(&r2.id, ApproveEnvironmentOptions::default())
        .await
        .unwrap();
    let err = s.drafter.discard(&r2.id).unwrap_err();
    assert!(err.message.contains("approved"));
}

// (Rust only) a git host that cannot commit files makes approval `unavailable`
#[tokio::test]
async fn approve_on_a_git_host_that_cannot_commit_is_unavailable() {
    let agent: Arc<dyn AgentAdapter> = ScriptedAgent::new(vec![writes(Full)]);
    let git = FakeGit::without_commit(&[("ratchet.yaml", MOCK_CONFIG_YAML)]);
    let mut o = AgentEnvironmentDrafterOptions::new(
        fixed_git(git),
        Arc::new(move |_| Ok(agent.clone())),
        dyn_clock(&clock()),
    );
    o.checker = Some(Checker::passing());
    let d = AgentEnvironmentDrafter::new(o);
    let rec = d.draft(input("g")).await.unwrap();
    let err = d
        .approve(&rec.id, ApproveEnvironmentOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, EnvErrorKind::Unavailable);
    let _ = EnvironmentScope::Full;
}
