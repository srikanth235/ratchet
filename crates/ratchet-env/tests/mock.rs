//! `MockEnvironmentDrafter` and the pull request text (ports of `packages/engine/src/environment/mock.test.ts`).

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use ratchet_core::enums::EnvironmentScope::{self, Full, None as NoScope, Single};
use ratchet_env::{
    ApproveEnvironmentOptions, EnvErrorKind, EnvironmentApprovalMode, EnvironmentDraftPhase,
    EnvironmentDraftStatus, EnvironmentDrafter, MOCK_CONFIG_YAML, MessageFrom,
    MockEnvironmentDrafter, MockEnvironmentDrafterOptions, ScopeChoice,
    build_environment_pull_request, draft_passed, fixed_git, scope_from_goal,
};

fn mock(scope: Option<EnvironmentScope>) -> MockEnvironmentDrafter {
    MockEnvironmentDrafter::new(MockEnvironmentDrafterOptions {
        scope: scope.map(ScopeChoice::Fixed),
        new_id: Some(ids()),
        ..Default::default()
    })
}

const NO_APPROVE: ApproveEnvironmentOptions = ApproveEnvironmentOptions { mode: None };

// "MockEnvironmentDrafter: one deterministic draft per scope" (3 cases)
#[tokio::test]
async fn one_deterministic_draft_per_scope() {
    for (scope, goal, machines) in [
        (NoScope, "make the unit tests faster", 1usize),
        (Single, "shrink the bundle", 1),
        (Full, "cut checkout p95 latency of the api", 4),
    ] {
        let rec = mock(None).draft(input(goal)).await.unwrap();
        assert_eq!(
            (rec.id.as_str(), rec.status, rec.revision),
            ("env-0001", EnvironmentDraftStatus::Ready, 1)
        );
        let draft = rec.draft.unwrap();
        assert_eq!(draft.scope, scope);
        assert_eq!(draft.agent.as_str(), "claude-code");
        assert_eq!(draft.resources.len(), machines);
        assert!(draft_passed(&draft), "{:?}", draft.checks); // every check ok, including the simulated plan
        assert!(draft.checks.iter().any(|c| c.id == "plan"));
        // ratchet.yaml is always part of the draft and carries the scope
        assert_eq!(draft.files[0].path, "ratchet.yaml");
        assert!(draft.files[0].content.contains(&format!("scope: {scope}")));
        // cost is computed from the resources, not canned
        let sum: f64 = draft.resources.iter().map(|r| r.usd_per_hour).sum();
        assert!((draft.cost.usd_per_hour - sum).abs() < 0.005);
        if scope == NoScope {
            assert_eq!(draft.resources[0].name, "Evaluator");
            assert_eq!(draft.resources[0].role.as_str(), "evaluator");
        }
        assert!(draft.cost.estimate_for_run_usd > 0.0);
    }
}

// "is deterministic: the same request yields byte-identical files and cost"
#[tokio::test]
async fn is_deterministic() {
    let a = mock(None)
        .draft(input("cut p95 latency"))
        .await
        .unwrap()
        .draft
        .unwrap();
    let b = mock(None)
        .draft(input("cut p95 latency"))
        .await
        .unwrap()
        .draft
        .unwrap();
    assert_eq!(a.files, b.files);
    assert_eq!(a.cost, b.cost);
}

// "an explicit scope (value or function) overrides the goal heuristic"
#[tokio::test]
async fn an_explicit_scope_overrides_the_goal_heuristic() {
    assert_eq!(
        mock(Some(NoScope))
            .draft(input("p95 latency api"))
            .await
            .unwrap()
            .draft
            .unwrap()
            .scope,
        NoScope
    );
    let f = MockEnvironmentDrafter::new(MockEnvironmentDrafterOptions {
        scope: Some(ScopeChoice::With(Arc::new(|i| {
            if i.goal.contains("db") { Full } else { Single }
        }))),
        ..Default::default()
    });
    assert_eq!(
        f.draft(input("db thing"))
            .await
            .unwrap()
            .draft
            .unwrap()
            .scope,
        Full
    );
    assert_eq!(
        f.draft(input("other")).await.unwrap().draft.unwrap().scope,
        Single
    );
}

// "prices the estimate for the run time limit it is given"
#[tokio::test]
async fn prices_the_estimate_for_the_run_time_limit() {
    let d = mock(Some(Full));
    let mut short = input("x");
    short.time_limit_seconds = Some(3600.0);
    let mut long = input("x");
    long.time_limit_seconds = Some(4.0 * 3600.0);
    let short = d.draft(short).await.unwrap().draft.unwrap().cost;
    let long = d.draft(long).await.unwrap().draft.unwrap().cost;
    assert!(long.estimate_for_run_usd > short.estimate_for_run_usd);
    assert_eq!(long.usd_per_hour, short.usd_per_hour);
}

// "reports progress phases and rejects an empty goal / repo"
#[tokio::test]
async fn reports_progress_phases_and_rejects_an_empty_goal_or_repo() {
    let phases = Arc::new(Mutex::new(Vec::new()));
    let p = phases.clone();
    let mut inp = input("goal");
    inp.on_progress = Some(Arc::new(move |e| p.lock().unwrap().push(e.phase)));
    MockEnvironmentDrafter::new(Default::default())
        .draft(inp)
        .await
        .unwrap();
    assert_eq!(
        *phases.lock().unwrap(),
        [
            EnvironmentDraftPhase::Reading,
            EnvironmentDraftPhase::Drafting,
            EnvironmentDraftPhase::Validating,
            EnvironmentDraftPhase::Done
        ]
    );
    let d = MockEnvironmentDrafter::new(Default::default());
    assert_eq!(
        d.draft(input("  ")).await.unwrap_err().kind,
        EnvErrorKind::InvalidInput
    );
    let mut no_repo = input("g");
    no_repo.repo = String::new();
    assert_eq!(
        d.draft(no_repo).await.unwrap_err().kind,
        EnvErrorKind::InvalidInput
    );
}

// "scopeFromGoal"
#[test]
fn scope_from_goal_reads_the_words() {
    assert_eq!(scope_from_goal("reduce p95 of checkout"), Full);
    assert_eq!(scope_from_goal("speed up unit tests"), NoScope);
    assert_eq!(scope_from_goal("smaller bundle"), Single);
}

// "MockEnvironmentDrafter: revise" -- "smaller" steps the scope down, "bigger" steps it up, ...
#[tokio::test]
async fn revise_steps_the_scope_and_accumulates_effort() {
    let d = mock(Some(Full));
    let first = d.draft(input("x")).await.unwrap();
    let spent1 = first.draft.as_ref().unwrap().usd_spent;
    let smaller = d
        .revise("env-0001", "make it smaller and cheaper")
        .await
        .unwrap();
    assert_eq!(
        (smaller.revision, smaller.status),
        (2, EnvironmentDraftStatus::Ready)
    );
    assert_eq!(smaller.draft.as_ref().unwrap().scope, Single);
    assert!(smaller.draft.as_ref().unwrap().usd_spent > spent1);
    assert_eq!(
        d.revise("env-0001", "even simpler")
            .await
            .unwrap()
            .draft
            .unwrap()
            .scope,
        NoScope
    );
    assert!(
        d.revise("env-0001", "even simpler")
            .await
            .unwrap()
            .draft
            .unwrap()
            .reason
            .contains("as small as it gets")
    );
    let bigger = d
        .revise("env-0001", "add a database and a load generator")
        .await
        .unwrap();
    assert_eq!(bigger.draft.as_ref().unwrap().scope, Single);
    assert!(draft_passed(bigger.draft.as_ref().unwrap()));
    assert_eq!(
        bigger
            .messages
            .iter()
            .filter(|m| m.from == MessageFrom::User)
            .count(),
        5
    );
}

// "explains why a managed database (RDS) is not used inside the isolated sandbox"
#[tokio::test]
async fn explains_why_rds_is_not_used() {
    let d = mock(Some(Full));
    let rec = d.draft(input("x")).await.unwrap();
    let after = d
        .revise(&rec.id, "use RDS instead of the container")
        .await
        .unwrap()
        .draft
        .unwrap();
    assert_eq!(after.scope, Full);
    assert!(after.reason.contains("RDS"));
    assert!(after.reason.contains("no route out"));
}

// "refuses unknown drafts, empty feedback and (after approval) any change"
#[tokio::test]
async fn revise_refuses_unknown_empty_and_approved() {
    let d = mock(Some(Single));
    assert_eq!(
        d.revise("nope", "x").await.unwrap_err().kind,
        EnvErrorKind::NotFound
    );
    let rec = d.draft(input("x")).await.unwrap();
    assert_eq!(
        d.revise(&rec.id, " ").await.unwrap_err().kind,
        EnvErrorKind::InvalidInput
    );
    d.approve(&rec.id, NO_APPROVE).await.unwrap();
    assert_eq!(
        d.revise(&rec.id, "smaller").await.unwrap_err().kind,
        EnvErrorKind::InvalidState
    );
    assert_eq!(
        d.discard(&rec.id).unwrap_err().kind,
        EnvErrorKind::InvalidState
    );
}

// "MockEnvironmentDrafter: approve" -- "without a git host the approval is synthetic: ..."
#[tokio::test]
async fn without_a_git_host_the_approval_is_synthetic() {
    let d = mock(Some(Full));
    let a = d
        .approve(&d.draft(input("x")).await.unwrap().id, NO_APPROVE)
        .await
        .unwrap();
    assert_eq!(a.status, EnvironmentDraftStatus::Approved);
    let ap = a.approval.clone().unwrap();
    assert_eq!((ap.mode, ap.locked), (EnvironmentApprovalMode::Pr, true));
    assert!(ap.pr_url.as_deref().unwrap().starts_with("https://"));
    assert!(ap.files.iter().any(|f| f == "infra/ratchet/main.tf"));
    let b = d
        .approve(
            &d.draft(input("y")).await.unwrap().id,
            ApproveEnvironmentOptions {
                mode: Some(EnvironmentApprovalMode::Commit),
            },
        )
        .await
        .unwrap();
    let bp = b.approval.unwrap();
    assert_eq!(
        (bp.mode, bp.locked),
        (EnvironmentApprovalMode::Commit, true)
    );
    assert!(bp.pr_url.is_none());
    // approving twice is idempotent
    assert_eq!(d.approve(&a.id, NO_APPROVE).await.unwrap(), a);
}

// "with a git host it really commits the draft files: a branch + pull request, or straight onto the base ref"
#[tokio::test]
async fn with_a_git_host_it_really_commits() {
    let git = FakeGit::new(&[("ratchet.yaml", MOCK_CONFIG_YAML)]);
    let d = MockEnvironmentDrafter::new(MockEnvironmentDrafterOptions {
        scope: Some(ScopeChoice::Fixed(Full)),
        git: Some(fixed_git(git.clone())),
        new_id: Some(ids()),
        ..Default::default()
    });
    let pr = d
        .approve(&d.draft(input("x")).await.unwrap().id, NO_APPROVE)
        .await
        .unwrap();
    let a = pr.approval.unwrap();
    assert_eq!(
        (a.mode, a.branch.as_deref(), a.sha.as_str(), a.locked),
        (
            EnvironmentApprovalMode::Pr,
            Some("ratchet/environment-0001"),
            "files-1",
            true
        )
    );
    assert_eq!(git.prs.lock().unwrap().len(), 1);
    assert!(git.prs.lock().unwrap()[0].body.contains("Scope: full"));
    {
        let committed = git.committed.lock().unwrap();
        assert_eq!(committed[0].0.base_ref, "main");
        let paths: Vec<_> = committed[0]
            .0
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        for want in [
            "ratchet.yaml",
            "infra/ratchet/main.tf",
            "infra/ratchet/scripts/api.sh",
        ] {
            assert!(paths.contains(&want), "{want} in {paths:?}");
        }
    }
    let direct = d
        .approve(
            &d.draft(input("y")).await.unwrap().id,
            ApproveEnvironmentOptions {
                mode: Some(EnvironmentApprovalMode::Commit),
            },
        )
        .await
        .unwrap();
    let da = direct.approval.unwrap();
    assert_eq!(
        (da.mode, da.sha.as_str()),
        (EnvironmentApprovalMode::Commit, "files-2")
    );
    let committed = git.committed.lock().unwrap();
    assert_eq!(committed[1].0.branch, "main");
    assert!(committed[1].0.update);
    assert_eq!(git.prs.lock().unwrap().len(), 1); // a direct commit opens no pull request
}

// "retries once with a fresh branch name when the PR branch exists"
#[tokio::test]
async fn retries_once_with_a_fresh_branch_name() {
    let git = FakeGit::new(&[("ratchet.yaml", MOCK_CONFIG_YAML)]);
    git.existing_branches
        .lock()
        .unwrap()
        .push("ratchet/environment-0001".into());
    let d = MockEnvironmentDrafter::new(MockEnvironmentDrafterOptions {
        scope: Some(ScopeChoice::Fixed(Single)),
        git: Some(fixed_git(git)),
        new_id: Some(ids()),
        ..Default::default()
    });
    let rec = d
        .approve(&d.draft(input("x")).await.unwrap().id, NO_APPROVE)
        .await
        .unwrap();
    assert!(
        rec.approval
            .unwrap()
            .branch
            .unwrap()
            .starts_with("ratchet/environment-0001-")
    );
}

// "refuses a draft with a failing check, or a git host that cannot commit files"
#[tokio::test]
async fn refuses_a_failing_check_or_a_git_host_that_cannot_commit() {
    // the records are copies here, so the "tampered" draft is one whose plan check failed to begin with: use a mock
    // whose validator fails (an unpriced machine type through the sample is not possible), so drive `approve`
    // through the real-drafter path in drafter.rs instead and test the commit-less host here
    let git = FakeGit::without_commit(&[]);
    let d = MockEnvironmentDrafter::new(MockEnvironmentDrafterOptions {
        scope: Some(ScopeChoice::Fixed(NoScope)),
        git: Some(fixed_git(git)),
        config_yaml: Some(MOCK_CONFIG_YAML.into()),
        ..Default::default()
    });
    let rec = d.draft(input("x")).await.unwrap();
    assert_eq!(
        d.approve(&rec.id, NO_APPROVE).await.unwrap_err().kind,
        EnvErrorKind::Unavailable
    );
}

// "list/get/discard manage drafts that are not approved"
#[tokio::test]
async fn list_get_discard() {
    let c = clock();
    let d = MockEnvironmentDrafter::new(MockEnvironmentDrafterOptions {
        clock: Some(dyn_clock(&c)),
        new_id: Some(ids()),
        ..Default::default()
    });
    let a = d.draft(input("one")).await.unwrap();
    c.advance(1000);
    let b = d.draft(input("two")).await.unwrap();
    assert_eq!(
        d.list().iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        [b.id.clone(), a.id.clone()]
    );
    d.discard(&a.id).unwrap();
    assert!(d.get(&a.id).is_none());
}

// "buildEnvironmentPullRequest: says what is created, what it costs, which checks passed and that the files are locked"
#[tokio::test]
async fn the_pull_request_says_what_is_created_what_it_costs_and_that_it_is_locked() {
    let rec = mock(Some(Full)).draft(input("cut p95")).await.unwrap();
    let pr = build_environment_pull_request(&rec.goal, &rec.id, rec.draft.as_ref().unwrap());
    assert!(pr.title.contains("full-stack environment"));
    assert!(pr.body.contains("| "));
    let cost = regex_cost(&pr.body);
    assert!(cost, "{}", pr.body);
    assert!(pr.body.contains("passed: "));
    assert!(pr.body.contains("infra/ratchet/**"));
    assert!(pr.body.contains("blocked"));
}

fn regex_cost(body: &str) -> bool {
    // **Cost:** $<digits>.<2 digits>/h
    body.find("**Cost:** $").is_some_and(|i| {
        let rest = &body[i + "**Cost:** $".len()..];
        let (num, tail) = rest.split_at(rest.find('/').unwrap_or(0));
        tail.starts_with("/h")
            && num.split_once('.').is_some_and(|(a, b)| {
                !a.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && b.len() == 2
                    && b.bytes().all(|c| c.is_ascii_digit())
            })
    })
}
