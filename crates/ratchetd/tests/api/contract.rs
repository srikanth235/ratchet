//! Contract tests: every route answers with the type the OpenAPI document promises. Each response is parsed into
//! its Rust type and serialized again; the result must equal the JSON on the wire (no field lost, no renamed
//! field), and a meta-test fails when a route in the document has no case here.

use crate::common;

use std::collections::BTreeSet;

use common::TestDaemon;
use hyper::StatusCode;
use ratchet_core::ports::{CostReconcileReport, ProposeTemplateResult};
use ratchet_core::templates::TemplateInfo;
use ratchet_core::{Calibration, EffectiveProtected};
use ratchetd::api::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

struct Ctx {
    d: TestDaemon,
    covered: BTreeSet<(String, String)>,
}

impl Ctx {
    /// Call `method path`, expect `status`, and require the body to round-trip through `T`.
    async fn rt<T: DeserializeOwned + Serialize>(
        &mut self,
        method: &str,
        template: &str,
        path: &str,
        body: Option<Value>,
        status: u16,
    ) -> T {
        self.covered
            .insert((method.to_owned(), template.to_owned()));
        let r = self.d.http.send(method, path, body).await;
        assert_eq!(r.status.as_u16(), status, "{method} {path}: {}", r.text());
        let wire = r.json();
        let typed: T = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("{method} {path}: body does not parse: {e}\n{wire}"));
        let back = serde_json::to_value(&typed).unwrap();
        assert_eq!(back, wire, "{method} {path} does not round-trip its type");
        typed
    }

    /// A route that answers `204 No Content`.
    async fn no_content(&mut self, method: &str, template: &str, path: &str, body: Option<Value>) {
        self.covered
            .insert((method.to_owned(), template.to_owned()));
        let r = self.d.http.send(method, path, body).await;
        assert_eq!(
            r.status,
            StatusCode::NO_CONTENT,
            "{method} {path}: {}",
            r.text()
        );
        assert!(r.body.is_empty());
    }

    /// A route that must fail with `status` and an `ApiError` body of class `code`.
    async fn fails(
        &mut self,
        method: &str,
        path: &str,
        body: Option<Value>,
        status: u16,
        code: ErrorCode,
    ) -> ApiError {
        let r = self.d.http.send(method, path, body).await;
        assert_eq!(r.status.as_u16(), status, "{method} {path}: {}", r.text());
        let e: ApiError = serde_json::from_value(r.json()).expect("an ApiError body");
        assert_eq!(e.code, code, "{method} {path}: {e:?}");
        assert!(!e.message.is_empty());
        e
    }
}

#[tokio::test]
async fn every_route_round_trips_its_types() {
    let mut c = Ctx {
        d: TestDaemon::start().await,
        covered: BTreeSet::new(),
    };
    let demo = "opt_demo_search";
    let ingest = "opt_demo_ingest";

    // ---- status, events ------------------------------------------------------------------------------
    let st: DaemonStatus = c.rt("GET", "/v1/status", "/v1/status", None, 200).await;
    assert_eq!(st.api_version, "v1");
    assert_eq!(st.engine.kind, EngineKind::Sim);
    assert_eq!(st.pid, std::process::id());
    c.covered.insert(("GET".into(), "/v1/events".into()));
    let mut sse = c.d.http.events(None).await;
    assert_eq!(sse.next().await.unwrap().event.as_deref(), Some("hello"));
    drop(sse);

    // ---- optimizations ---------------------------------------------------------------------------------
    let snap: OptimizationsSnapshot = c
        .rt("GET", "/v1/optimizations", "/v1/optimizations", None, 200)
        .await;
    assert_eq!(snap.optimizations.len(), 4);
    assert_eq!(snap.featured_id.as_deref(), Some(demo));
    assert!(snap.featured_series.is_some());
    let snap7: OptimizationsSnapshot = c
        .rt(
            "GET",
            "/v1/optimizations",
            "/v1/optimizations?range=7d",
            None,
            200,
        )
        .await;
    assert_eq!(snap7.range, KpiRange::Days7);

    let created: CreatedOptimization = c
        .rt(
            "POST",
            "/v1/optimizations",
            "/v1/optimizations",
            Some(json!({
                "demo": "demo-search", "agents": ["claude-code", "codex"], "budgetUsd": 20,
                "timeLimitSeconds": 3600, "plateauEvals": 25
            })),
            201,
        )
        .await;
    let new_id = created.id;
    let detail: OptimizationDetail = c
        .rt(
            "GET",
            "/v1/optimizations/{id}",
            &format!("/v1/optimizations/{new_id}"),
            None,
            200,
        )
        .await;
    assert_eq!(detail.summary.status, UiStatus::Draft);
    assert_eq!(detail.agents.len(), 2);
    c.no_content(
        "POST",
        "/v1/optimizations/{id}/start",
        &format!("/v1/optimizations/{new_id}/start"),
        None,
    )
    .await;
    for _ in 0..4 {
        c.d.sim.tick(&new_id).await.unwrap();
    }
    let detail: OptimizationDetail =
        c.d.http
            .get_json(&format!("/v1/optimizations/{new_id}"))
            .await
            .pipe_into();
    assert_eq!(detail.summary.status, UiStatus::Running);
    assert!(detail.summary.candidate_count >= 4, "{detail:?}");

    let series: RatchetSeries = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/ratchet",
            &format!("/v1/optimizations/{demo}/ratchet"),
            None,
            200,
        )
        .await;
    assert_eq!(series.metric, "http_req_duration.p95");
    assert!(series.options.len() >= 3, "p50/p95/p99");
    let p99: RatchetSeries = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/ratchet",
            &format!("/v1/optimizations/{demo}/ratchet?metric=http_req_duration.p99"),
            None,
            200,
        )
        .await;
    assert_eq!(p99.metric, "http_req_duration.p99");
    assert!(p99.noise_floor.is_none());
    let wc: WinningChangeVm = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/winning-change",
            &format!("/v1/optimizations/{demo}/winning-change"),
            None,
            200,
        )
        .await;
    assert_eq!(wc.commits, 5);
    assert!(
        wc.unavailable.is_some(),
        "the simulation has no git behind it"
    );
    let cands: Vec<CandidateVm> = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/candidates",
            &format!("/v1/optimizations/{demo}/candidates"),
            None,
            200,
        )
        .await;
    assert_eq!(cands.len(), 11);
    let spend: SpendVm = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/spend",
            &format!("/v1/optimizations/{demo}/spend"),
            None,
            200,
        )
        .await;
    assert!((spend.total - 42.6).abs() < 0.2, "{}", spend.total);
    let rec: CostReconcileReport = c
        .rt(
            "POST",
            "/v1/optimizations/{id}/spend/reconciliations",
            &format!("/v1/optimizations/{ingest}/spend/reconciliations"),
            None,
            200,
        )
        .await;
    assert_eq!(rec.optimization_id.as_str(), ingest);
    let _: Calibration = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/calibration",
            &format!("/v1/optimizations/{demo}/calibration"),
            None,
            200,
        )
        .await;
    let prot: EffectiveProtected = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/protected-paths",
            &format!("/v1/optimizations/{demo}/protected-paths"),
            None,
            200,
        )
        .await;
    assert!(prot.globs.iter().any(|g| g == "ratchet.yaml"));
    let receipt: TeardownReceiptVm = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/receipt",
            &format!("/v1/optimizations/{ingest}/receipt"),
            None,
            200,
        )
        .await;
    assert!(receipt.nothing_running);
    let pr: PullRequestVm = c
        .rt(
            "POST",
            "/v1/optimizations/{id}/pull-request",
            &format!("/v1/optimizations/{demo}/pull-request"),
            None,
            201,
        )
        .await;
    assert!(
        pr.url
            .starts_with("https://github.com/acme/search-api/pull/")
    );
    let report: ReportVm = c
        .rt(
            "GET",
            "/v1/optimizations/{id}/report",
            &format!("/v1/optimizations/{demo}/report"),
            None,
            200,
        )
        .await;
    assert!(
        report
            .markdown
            .starts_with("# Ratchet report: search-api p95 latency")
    );
    let act: Vec<ActivityVm> = c
        .rt("GET", "/v1/activity", "/v1/activity?limit=5", None, 200)
        .await;
    assert_eq!(act.len(), 5);
    let _: Vec<ActivityVm> = c
        .rt(
            "GET",
            "/v1/activity",
            &format!("/v1/activity?optimizationId={ingest}"),
            None,
            200,
        )
        .await;

    // ---- agents ----------------------------------------------------------------------------------------
    let agent = format!("{demo}_agt_0");
    let log: AgentEventsPage = c
        .rt(
            "GET",
            "/v1/agents/{id}/events",
            &format!("/v1/agents/{agent}/events"),
            None,
            200,
        )
        .await;
    assert!(!log.events.is_empty());
    let newer: AgentEventsPage = c
        .rt(
            "GET",
            "/v1/agents/{id}/events",
            &format!("/v1/agents/{agent}/events?cursor={}", log.cursor),
            None,
            200,
        )
        .await;
    assert!(newer.events.is_empty());
    assert_eq!(newer.cursor, log.cursor);
    c.no_content(
        "POST",
        "/v1/agents/{id}/pause",
        &format!("/v1/agents/{agent}/pause"),
        None,
    )
    .await;
    c.no_content(
        "POST",
        "/v1/agents/{id}/resume",
        &format!("/v1/agents/{agent}/resume"),
        None,
    )
    .await;
    c.no_content(
        "POST",
        "/v1/agents/{id}/messages",
        &format!("/v1/agents/{agent}/messages"),
        Some(json!({"text": "try the index first"})),
    )
    .await;

    // ---- config ---------------------------------------------------------------------------------------
    let ok: ValidateConfigResult = c
        .rt(
            "POST",
            "/v1/config/validations",
            "/v1/config/validations",
            Some(json!({"yaml": ratchetd::sim::DEFAULT_CONFIG_YAML})),
            200,
        )
        .await;
    assert!(ok.ok && ok.summary.is_some());
    let bad: ValidateConfigResult = c
        .rt(
            "POST",
            "/v1/config/validations",
            "/v1/config/validations",
            Some(json!({"yaml": "version: 0"})),
            200,
        )
        .await;
    assert!(!bad.ok && !bad.errors.is_empty());
    let pre: Vec<PreflightItem> = c
        .rt(
            "POST",
            "/v1/preflight",
            "/v1/preflight",
            Some(json!({"agents": ["claude-code", "codex"], "agentsWhere": "local", "source": "github"})),
            200,
        )
        .await;
    assert!(
        pre.iter()
            .any(|p| p.id == "agent-codex" && p.state == PreflightState::Fail)
    );

    // ---- settings --------------------------------------------------------------------------------------
    let s: SettingsVm = c.rt("GET", "/v1/settings", "/v1/settings", None, 200).await;
    assert!(s.secrets.anthropic_api_key && !s.secrets.openai_api_key);
    let s: SettingsVm = c
        .rt(
            "PUT",
            "/v1/settings/advanced",
            "/v1/settings/advanced",
            Some(json!({"awsRegion": "eu-west-2", "runnerImageRef": "ami-0123456789abcdef0"})),
            200,
        )
        .await;
    assert_eq!(s.aws_region, "eu-west-2");
    assert_eq!(s.advanced.values.runner_image_ref, "ami-0123456789abcdef0");
    let s: SettingsVm = c
        .rt(
            "PUT",
            "/v1/settings/aws-profile",
            "/v1/settings/aws-profile",
            Some(json!({"profile": "personal-dev"})),
            200,
        )
        .await;
    assert_eq!(s.aws_profile, "personal-dev");
    assert_eq!(s.advanced.values.aws_auth, AwsAuthKind::Profile);
    let s: SettingsVm = c
        .rt(
            "PUT",
            "/v1/settings/secrets/{name}",
            "/v1/settings/secrets/openaiApiKey",
            Some(json!({"value": "sk-proj-abcdefghijklmnop1234"})),
            200,
        )
        .await;
    assert!(s.secrets.openai_api_key);
    let s: SettingsVm = c
        .rt(
            "DELETE",
            "/v1/settings/secrets/{name}",
            "/v1/settings/secrets/openaiApiKey",
            None,
            200,
        )
        .await;
    assert!(!s.secrets.openai_api_key);
    let t: ConnectionResultVm = c
        .rt(
            "POST",
            "/v1/connection-tests",
            "/v1/connection-tests",
            Some(json!({"target": "aws"})),
            200,
        )
        .await;
    assert!(t.ok, "{t:?}");

    // ---- GitHub, repos, AWS, harnesses ------------------------------------------------------------------
    let gh: GithubStatusVm = c
        .rt("GET", "/v1/github/status", "/v1/github/status", None, 200)
        .await;
    assert!(gh.signed_in);
    let repos: Vec<RepoVm> = c
        .rt("GET", "/v1/github/repos", "/v1/github/repos", None, 200)
        .await;
    assert!(repos.len() >= 5);
    let one: Vec<RepoVm> = c
        .rt(
            "GET",
            "/v1/github/repos",
            "/v1/github/repos?query=shop",
            None,
            200,
        )
        .await;
    assert_eq!(one.len(), 1);
    let ins: RepoInspectVm = c
        .rt(
            "POST",
            "/v1/repos/inspections",
            "/v1/repos/inspections",
            Some(json!({"source": {"kind": "github", "repo": "acme-org/shop-api"}})),
            200,
        )
        .await;
    assert!(
        ins.config.is_none() && !ins.templates.is_empty(),
        "no ratchet.yaml: templates are offered"
    );
    let ins2: RepoInspectVm = c
        .rt(
            "POST",
            "/v1/repos/inspections",
            "/v1/repos/inspections",
            Some(json!({"source": {"kind": "github", "repo": "acme/search-api"}})),
            200,
        )
        .await;
    assert!(ins2.config.is_some() && ins2.templates.is_empty());
    let _: AwsProfilesVm = c
        .rt("GET", "/v1/aws/profiles", "/v1/aws/profiles", None, 200)
        .await;
    let h: Vec<HarnessVm> = c
        .rt("GET", "/v1/harnesses", "/v1/harnesses", None, 200)
        .await;
    assert_eq!(h.len(), 2);
    let _: Vec<HarnessVm> = c
        .rt(
            "POST",
            "/v1/harnesses/detections",
            "/v1/harnesses/detections",
            None,
            200,
        )
        .await;

    // ---- templates -------------------------------------------------------------------------------------
    let all: Vec<TemplateInfo> = c
        .rt("GET", "/v1/templates", "/v1/templates", None, 200)
        .await;
    assert_eq!(all.len(), 10, "seven templates and three demos");
    let demos: Vec<TemplateInfo> = c
        .rt("GET", "/v1/templates", "/v1/templates?kind=demo", None, 200)
        .await;
    assert_eq!(demos.len(), 3);
    let _: TemplateInfo = c
        .rt(
            "GET",
            "/v1/templates/{id}",
            "/v1/templates/http-latency",
            None,
            200,
        )
        .await;
    let p: ProposeTemplateResult = c
        .rt(
            "POST",
            "/v1/template-proposals",
            "/v1/template-proposals",
            Some(json!({"repoUrl": "acme-org/shop-api", "baseRef": "main", "templateId": "http-latency"})),
            201,
        )
        .await;
    assert!(p.files.contains(&"ratchet.yaml".to_owned()));

    // ---- environments -----------------------------------------------------------------------------------
    let envs: EnvironmentsVm = c
        .rt("GET", "/v1/environments", "/v1/environments", None, 200)
        .await;
    assert_eq!(envs.environments.len(), 1);
    let sandbox = envs.environments[0].sandbox_id.clone();
    assert!(!envs.environments[0].can_end, "runs are active in it");
    let busy = c
        .fails(
            "POST",
            &format!("/v1/environments/{sandbox}/end"),
            None,
            409,
            ErrorCode::Conflict,
        )
        .await;
    assert!(busy.message.contains("run is still active"));
    c.no_content(
        "POST",
        "/v1/optimizations/{id}/stop",
        &format!("/v1/optimizations/{new_id}/stop"),
        Some(json!({"reason": "test"})),
    )
    .await;
    c.no_content(
        "POST",
        "/v1/optimizations/{id}/stop",
        &format!("/v1/optimizations/{demo}/stop"),
        None,
    )
    .await;
    let after: EnvironmentsVm = c.d.http.get_json("/v1/environments").await.pipe_into();
    assert!(after.environments[0].can_end);
    let ended: EnvironmentsVm = c
        .rt(
            "POST",
            "/v1/environments/{sandboxId}/end",
            &format!("/v1/environments/{sandbox}/end"),
            None,
            200,
        )
        .await;
    assert!(ended.environments.is_empty());
    assert_eq!(
        ended.last_receipt.as_ref().unwrap().trigger,
        ReceiptTrigger::Manual
    );

    // ---- GitHub sign-out (after everything that needs it) -----------------------------------------------
    let out: GithubStatusVm = c
        .rt(
            "DELETE",
            "/v1/github/session",
            "/v1/github/session",
            None,
            200,
        )
        .await;
    assert!(!out.signed_in);
    c.fails("GET", "/v1/github/repos", None, 409, ErrorCode::NotReady)
        .await;
    let flow: GithubStatusVm = c
        .rt(
            "POST",
            "/v1/github/sign-in",
            "/v1/github/sign-in",
            None,
            200,
        )
        .await;
    assert_eq!(flow.flow.as_ref().unwrap().state, GithubFlowState::Pending);

    // ---- overview, connectors, drafts (provisional) ------------------------------------------------------
    let ov: OverviewVm = c.rt("GET", "/v1/overview", "/v1/overview", None, 200).await;
    assert_eq!(ov.range, OverviewRange::Days90);
    assert!(ov.value_locked_in.usd_per_year > 0.0 && !ov.staircase.points.is_empty());
    assert!(ov.needs_you.iter().any(|n| n.kind == NeedsYouKind::SignIn));
    assert_eq!(
        ov.rows.len(),
        5,
        "the four seeded runs and the one created above"
    );
    let _: OverviewVm = c
        .rt("GET", "/v1/overview", "/v1/overview?range=all", None, 200)
        .await;

    let conns: Vec<ConnectorVm> = c
        .rt("GET", "/v1/connectors", "/v1/connectors", None, 200)
        .await;
    assert_eq!(conns.len(), 8);
    let new: ConnectorVm = c
        .rt(
            "POST",
            "/v1/connectors",
            "/v1/connectors",
            Some(json!({"kind": "aws", "name": "sandbox-eu", "auth": "profile", "profile": "eu", "region": "eu-west-1", "capUsdPerMonth": 80})),
            201,
        )
        .await;
    assert_eq!(new.kind, ConnectorKind::Aws);
    let got: ConnectorVm = c
        .rt(
            "GET",
            "/v1/connectors/{id}",
            &format!("/v1/connectors/{}", new.id),
            None,
            200,
        )
        .await;
    assert_eq!(got, new);
    let upd: ConnectorVm = c
        .rt(
            "PATCH",
            "/v1/connectors/{id}",
            &format!("/v1/connectors/{}", new.id),
            Some(json!({"name": "eu-sandbox", "isDefault": true})),
            200,
        )
        .await;
    assert!(upd.is_default && upd.name == "eu-sandbox");
    let tested: ConnectorTestVm = c
        .rt(
            "POST",
            "/v1/connectors/{id}/test",
            "/v1/connectors/conn_aws_staging/test",
            None,
            200,
        )
        .await;
    assert_eq!(tested.connector.status, ConnectorStatus::Connected);
    let sel: ConnectorSelectionVm = c
        .rt(
            "GET",
            "/v1/connector-selections",
            "/v1/connector-selections?repo=acme-org/shop-api",
            None,
            200,
        )
        .await;
    assert!(!sel.remembered);
    assert!(sel.selection.aws_connector_id.is_some());
    let saved: ConnectorSelectionVm = c
        .rt(
            "PUT",
            "/v1/connector-selections",
            "/v1/connector-selections",
            Some(json!({"repo": "acme-org/shop-api", "selection": {
                "awsConnectorId": "conn_aws_dev", "githubConnectorId": "conn_gh_acme",
                "agents": [{"connectorId": "conn_agent_cc_key", "runsIn": "cloud"}]}})),
            200,
        )
        .await;
    assert!(saved.remembered);
    c.no_content(
        "DELETE",
        "/v1/connectors/{id}",
        &format!("/v1/connectors/{}", new.id),
        None,
    )
    .await;

    let draft: EnvironmentDraftVm = c
        .rt(
            "POST",
            "/v1/environment-drafts",
            "/v1/environment-drafts",
            Some(json!({"repo": "acme-org/shop-api", "goal": "Make checkout p95 faster"})),
            201,
        )
        .await;
    assert_eq!(draft.scope, EnvScope::Full);
    assert_eq!(draft.resources.len(), 4);
    assert!((draft.cost.hourly_usd - 0.59).abs() < 1e-9);
    assert!(draft.checks_passed);
    let did = draft.id.clone();
    let _: EnvironmentDraftVm = c
        .rt(
            "GET",
            "/v1/environment-drafts/{id}",
            &format!("/v1/environment-drafts/{did}"),
            None,
            200,
        )
        .await;
    let files: EnvFilesVm = c
        .rt(
            "GET",
            "/v1/environment-drafts/{id}/files",
            &format!("/v1/environment-drafts/{did}/files"),
            None,
            200,
        )
        .await;
    assert_eq!(
        files
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["main.tf", "Dockerfile", "seed.sh"]
    );
    let revised: EnvironmentDraftVm = c
        .rt(
            "POST",
            "/v1/environment-drafts/{id}/revisions",
            &format!("/v1/environment-drafts/{did}/revisions"),
            Some(json!({"request": "Use a smaller setup"})),
            200,
        )
        .await;
    assert_eq!((revised.scope, revised.revision), (EnvScope::Single, 2));
    let pr: EnvironmentDraftVm = c
        .rt(
            "POST",
            "/v1/environment-drafts/{id}/pull-request",
            &format!("/v1/environment-drafts/{did}/pull-request"),
            None,
            200,
        )
        .await;
    assert!(pr.pr_url.is_some());
    let approved: EnvironmentDraftVm = c
        .rt(
            "POST",
            "/v1/environment-drafts/{id}/approval",
            &format!("/v1/environment-drafts/{did}/approval"),
            None,
            200,
        )
        .await;
    assert_eq!(approved.status, EnvironmentDraftStatus::Approved);
    c.fails(
        "POST",
        &format!("/v1/environment-drafts/{did}/revisions"),
        Some(json!({"request": "more"})),
        409,
        ErrorCode::Conflict,
    )
    .await;

    // ---- the document and the cases agree --------------------------------------------------------------
    let documented: BTreeSet<(String, String)> = ratchetd::openapi::route_table()
        .iter()
        .map(|r| {
            let mut it = r.split(' ');
            (it.next().unwrap().to_owned(), it.next().unwrap().to_owned())
        })
        .collect();
    let missing: Vec<_> = documented.difference(&c.covered).collect();
    let extra: Vec<_> = c.covered.difference(&documented).collect();
    assert!(
        missing.is_empty(),
        "routes without a contract case: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "cases for routes that are not documented: {extra:?}"
    );
    c.d.stop().await;
}

trait PipeInto {
    fn pipe_into<T: DeserializeOwned>(self) -> T;
}

impl PipeInto for Value {
    fn pipe_into<T: DeserializeOwned>(self) -> T {
        serde_json::from_value(self).unwrap()
    }
}

#[tokio::test]
async fn failures_are_api_errors() {
    let mut c = Ctx {
        d: TestDaemon::start().await,
        covered: BTreeSet::new(),
    };
    c.fails(
        "GET",
        "/v1/optimizations/nope",
        None,
        404,
        ErrorCode::NotFound,
    )
    .await;
    c.fails(
        "GET",
        "/v1/agents/nope/events",
        None,
        404,
        ErrorCode::NotFound,
    )
    .await;
    c.fails("GET", "/v1/nope", None, 404, ErrorCode::NotFound)
        .await;
    c.fails(
        "GET",
        "/v1/templates/not-a-template",
        None,
        404,
        ErrorCode::NotFound,
    )
    .await;
    c.fails(
        "GET",
        "/v1/optimizations/opt_demo_web/receipt",
        None,
        404,
        ErrorCode::NotFound,
    )
    .await;
    c.fails(
        "GET",
        "/v1/optimizations/opt_demo_web/calibration",
        None,
        404,
        ErrorCode::NotFound,
    )
    .await;
    // malformed bodies and queries are 400 with the same body shape
    let e = c
        .fails(
            "POST",
            "/v1/optimizations",
            Some(json!({"agents": "claude"})),
            400,
            ErrorCode::InvalidRequest,
        )
        .await;
    assert!(e.message.contains("invalid JSON body"), "{e:?}");
    c.fails(
        "GET",
        "/v1/optimizations?range=1y",
        None,
        400,
        ErrorCode::InvalidRequest,
    )
    .await;
    // field validation names the fields
    let e = c
        .fails(
            "POST",
            "/v1/optimizations",
            Some(json!({"agents": [], "budgetUsd": -1, "timeLimitSeconds": 60, "plateauEvals": 5})),
            400,
            ErrorCode::InvalidRequest,
        )
        .await;
    let paths: Vec<&str> = e.fields.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["agents", "budgetUsd", "repoUrl"]);
    let e = c
        .fails(
            "PUT",
            "/v1/settings/advanced",
            Some(json!({"awsRegion": "mars", "benchImageRef": "ami-nope"})),
            400,
            ErrorCode::InvalidRequest,
        )
        .await;
    assert_eq!(e.fields.len(), 2);
    // nothing was saved
    let s: SettingsVm = c.d.http.get_json("/v1/settings").await.pipe_into();
    assert_eq!(s.aws_region, "us-east-1");
    c.fails(
        "PUT",
        "/v1/settings/secrets/githubToken",
        Some(json!({"value": "hunter2"})),
        400,
        ErrorCode::InvalidRequest,
    )
    .await;
    c.fails(
        "PUT",
        "/v1/settings/secrets/bogus",
        Some(json!({"value": "x"})),
        400,
        ErrorCode::InvalidRequest,
    )
    .await;
    // state conflicts
    c.fails(
        "POST",
        "/v1/optimizations/opt_demo_ingest/start",
        None,
        409,
        ErrorCode::Conflict,
    )
    .await;
    c.fails(
        "POST",
        "/v1/optimizations/opt_demo_web/pull-request",
        None,
        409,
        ErrorCode::Conflict,
    )
    .await;
    c.fails(
        "POST",
        "/v1/environments/sbx-nope/end",
        None,
        404,
        ErrorCode::NotFound,
    )
    .await;
    c.fails(
        "POST",
        "/v1/agents/opt_demo_search_agt_0/messages",
        Some(json!({"text": "   "})),
        400,
        ErrorCode::InvalidRequest,
    )
    .await;
    c.d.stop().await;
}

#[tokio::test]
async fn the_agent_log_pages_by_cursor() {
    let c = TestDaemon::start().await;
    let a = "opt_demo_search_agt_1";
    let first: AgentEventsPage = c
        .http
        .get_json(&format!("/v1/agents/{a}/events?limit=3"))
        .await
        .pipe_into();
    assert_eq!(first.events.len(), 3, "the newest three, oldest first");
    let ids: Vec<i64> = first.events.iter().map(|e| e.id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
    c.http
        .post(
            &format!("/v1/agents/{a}/messages"),
            json!({"text": "hello"}),
        )
        .await;
    let next: AgentEventsPage = c
        .http
        .get_json(&format!("/v1/agents/{a}/events?cursor={}", first.cursor))
        .await
        .pipe_into();
    assert_eq!(next.events.len(), 1);
    assert_eq!(next.events[0].text, "You: hello");
    c.stop().await;
}
