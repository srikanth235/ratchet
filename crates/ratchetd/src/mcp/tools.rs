//! What each session tool does. Chat tools are thin: they call the same [`AppService`](crate::service::AppService) methods the
//! routes call, so a chat can do exactly what the app can, and no more.

use ratchet_core::enums::{AgentKind, Direction};
use ratchet_core::ports::CommitFile;
use ratchet_core::templates::RenderTemplateOptions;
use ratchet_mcp::ToolSet;
use ratchet_mcp::args::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::config_branch::{CONFIG_BRANCH, commit_config};
use super::{OptimizationDraft, SessionInfo};
use crate::api::{CreateOptimizationInput, KpiRange, OverviewRange as OverviewWindow};
use crate::routes::AppState;
use crate::service::{ServiceError, ServiceResult};

const DEFAULT_TEMPLATE: &str = "test-suite-time";

fn args<T: DeserializeOwned>(arguments: Value) -> ServiceResult<T> {
    serde_json::from_value(arguments)
        .map_err(|e| ServiceError::Invalid(format!("bad arguments: {e}")))
}

fn to_value<T: serde::Serialize>(v: &T) -> ServiceResult<Value> {
    serde_json::to_value(v).map_err(|e| ServiceError::Internal(e.to_string()))
}

fn window<T: DeserializeOwned + Default>(name: Option<&str>) -> ServiceResult<T> {
    match name {
        None => Ok(T::default()),
        Some(n) => {
            serde_json::from_value(json!(n)).map_err(|e| ServiceError::Invalid(e.to_string()))
        }
    }
}

/// A chat bound to one optimization may not touch another.
fn in_scope(session: &SessionInfo, optimization: &str) -> ServiceResult<()> {
    match &session.scope.optimization {
        Some(bound) if bound != optimization => Err(ServiceError::Invalid(format!(
            "this session is bound to optimization {bound}; it cannot use {optimization}"
        ))),
        _ => Ok(()),
    }
}

/// Run `tool` for `session`. The caller has checked that the tool is in the session's set and that the arguments parse.
pub async fn call(
    state: &AppState,
    session: &SessionInfo,
    tool: &str,
    arguments: Value,
) -> ServiceResult<Value> {
    match session.set {
        ToolSet::Chat => chat(state, session, tool, arguments).await,
        ToolSet::Optimization => optimization(state, session, tool, arguments).await,
    }
}

async fn chat(
    state: &AppState,
    session: &SessionInfo,
    tool: &str,
    arguments: Value,
) -> ServiceResult<Value> {
    let svc = &state.service;
    match tool {
        "list_optimizations" => {
            let a: ListOptimizationsArgs = args(arguments)?;
            let mut snapshot = to_value(
                &svc.list_optimizations(window::<KpiRange>(a.range.map(Range::as_str))?)
                    .await?,
            )?;
            // a bound chat sees its own optimization only
            if let (Some(bound), Some(list)) = (
                &session.scope.optimization,
                snapshot
                    .get_mut("optimizations")
                    .and_then(Value::as_array_mut),
            ) {
                list.retain(|o| o["id"].as_str() == Some(bound));
            }
            Ok(snapshot)
        }
        "get_optimization" => {
            let a: OptimizationArgs = args(arguments)?;
            in_scope(session, &a.optimization)?;
            to_value(&svc.get_optimization(&a.optimization).await?)
        }
        "get_run" => {
            let a: RunArgs = args(arguments)?;
            in_scope(session, &a.run)?;
            Ok(json!({
                "run": a.run,
                "candidates": to_value(&svc.candidates(&a.run).await?)?,
                "spend": to_value(&svc.spend(&a.run).await?)?,
            }))
        }
        "get_overview" => {
            let a: OverviewArgs = args(arguments)?;
            to_value(
                &svc.overview(window::<OverviewWindow>(a.range.map(|r| r.as_str()))?)
                    .await?,
            )
        }
        "list_templates" => {
            let a: ListTemplatesArgs = args(arguments)?;
            to_value(&svc.list_templates(a.kind).await?)
        }
        "list_connectors" => to_value(&svc.list_connectors().await?),
        "list_repos" => {
            let a: ListReposArgs = args(arguments)?;
            to_value(&svc.list_repos(a.query, None).await?)
        }
        "draft_optimization" => draft(state, session, args(arguments)?).await,
        "create_optimization" => {
            let a: CreateOptimizationArgs = args(arguments)?;
            create(state, session, &a.draft_id).await
        }
        "run_now" => {
            let a: OptimizationArgs = args(arguments)?;
            in_scope(session, &a.optimization)?;
            svc.start_optimization(&a.optimization).await?;
            Ok(json!({"optimization": a.optimization, "started": true}))
        }
        "pause" | "resume" => {
            let a: OptimizationArgs = args(arguments)?;
            in_scope(session, &a.optimization)?;
            let detail = svc.get_optimization(&a.optimization).await?;
            let mut changed = Vec::new();
            for agent in &detail.agents {
                let paused = to_value(&agent.status)? == json!("paused");
                if tool == "pause" && !paused {
                    svc.pause_agent(&agent.id).await?;
                    changed.push(agent.id.clone());
                } else if tool == "resume" && paused {
                    svc.resume_agent(&agent.id).await?;
                    changed.push(agent.id.clone());
                }
            }
            Ok(json!({"optimization": a.optimization, "agents": changed}))
        }
        "add_steering_note" => {
            let a: SteeringNoteArgs = args(arguments)?;
            in_scope(session, &a.optimization)?;
            let detail = svc.get_optimization(&a.optimization).await?;
            if detail.agents.is_empty() {
                return Err(ServiceError::Conflict(
                    "this optimization has no agents yet; add the note once it has started".into(),
                ));
            }
            for agent in &detail.agents {
                svc.message_agent(&agent.id, &a.text).await?;
            }
            Ok(json!({"optimization": a.optimization, "deliveredTo": detail.agents.len()}))
        }
        "open_pr" => {
            let a: OpenPrArgs = args(arguments)?;
            in_scope(session, &a.optimization)?;
            if let Some(sha) = a.candidate.as_deref().filter(|s| !s.is_empty()) {
                let candidates = svc.candidates(&a.optimization).await?;
                if !candidates
                    .iter()
                    .any(|c| c.sha.starts_with(sha) || sha.starts_with(&c.sha))
                {
                    return Err(ServiceError::Invalid(format!(
                        "candidate {sha} is not part of optimization {}; see get_run",
                        a.optimization
                    )));
                }
            }
            to_value(&svc.open_pull_request(&a.optimization).await?)
        }
        "explain_result" => {
            let a: RunArgs = args(arguments)?;
            in_scope(session, &a.run)?;
            Ok(json!({
                "report": to_value(&svc.report(&a.run).await?)?,
                "candidates": to_value(&svc.candidates(&a.run).await?)?,
            }))
        }
        other => Err(ServiceError::Invalid(format!("unknown tool \"{other}\""))),
    }
}

/// `draft_optimization`: render the template with the goal as the brief and commit the files to `ratchet/config`.
async fn draft(
    state: &AppState,
    session: &SessionInfo,
    a: DraftOptimizationArgs,
) -> ServiceResult<Value> {
    let repo = ratchet_github::slug_of(&a.repo).ok_or_else(|| {
        ServiceError::Invalid(format!(
            "{} is not a GitHub repository (owner/name)",
            a.repo
        ))
    })?;
    if let Some(bound) = &session.scope.repo
        && bound != &repo
    {
        return Err(ServiceError::Invalid(format!(
            "this chat is bound to {bound}; it cannot draft for {repo}"
        )));
    }
    if a.goal.trim().is_empty() {
        return Err(ServiceError::Invalid("the goal must not be empty".into()));
    }
    let budget = a.budget.unwrap_or(50.0);
    if !budget.is_finite() || budget <= 0.0 {
        return Err(ServiceError::Invalid(
            "the budget must be a positive number of USD".into(),
        ));
    }
    let template = a
        .template
        .clone()
        .unwrap_or_else(|| DEFAULT_TEMPLATE.to_owned());
    let rendered = ratchet_core::render_template(
        &template,
        &RenderTemplateOptions {
            metric: a.metric.clone(),
            ..RenderTemplateOptions::default()
        },
    )
    .map_err(|e| ServiceError::Invalid(format!("{e}; see list_templates")))?;

    let mut files: Vec<CommitFile> = Vec::new();
    for f in &rendered.files {
        let content = if f.path == "RATCHET.md" {
            format!("# Goal\n\n{}\n\n{}", a.goal.trim(), f.content)
        } else {
            f.content.clone()
        };
        files.push(CommitFile::new(f.path.clone(), content));
    }
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();

    // the repo's default branch is where the config branch starts
    let base = state
        .service
        .list_repos(Some(repo.clone()), None)
        .await
        .ok()
        .and_then(|r| r.into_iter().find(|r| r.full_name == repo))
        .map_or_else(|| "main".to_owned(), |r| r.default_branch);

    let host = state.mcp.host_for(&repo).await?;
    let commit = commit_config(
        host.as_ref(),
        &base,
        &format!("ratchet: draft config for {}", one_line(&a.goal)),
        files,
    )
    .await
    .map_err(|e| ServiceError::Upstream(format!("cannot write {CONFIG_BRANCH}: {e}")))?;

    let agents: Vec<AgentKind> = a
        .agents
        .clone()
        .unwrap_or_else(|| vec![AgentKind::ClaudeCode]);
    let draft = OptimizationDraft {
        draft_id: format!("draft_{}", &uuid_hex()[..10]),
        repo,
        base_ref: CONFIG_BRANCH.to_owned(),
        config_branch: CONFIG_BRANCH.to_owned(),
        commit: commit.sha,
        goal: a.goal.trim().to_owned(),
        template: rendered.info.id.clone(),
        metric: rendered.options.metric.clone(),
        direction: match rendered.options.direction {
            Direction::Minimize => "minimize",
            Direction::Maximize => "maximize",
        }
        .to_owned(),
        schedule: a.schedule.filter(|s| !s.trim().is_empty()),
        budget_usd: budget,
        agents: agents.iter().map(|k| k.as_str().to_owned()).collect(),
        files: paths,
        created_optimization: None,
    };
    state.mcp.put_draft(draft.clone());
    to_value(&draft)
}

/// `create_optimization`: from the draft, create and start. There is no approval step; the user's message was the go-ahead.
async fn create(state: &AppState, session: &SessionInfo, draft_id: &str) -> ServiceResult<Value> {
    let draft = state
        .mcp
        .draft(draft_id)
        .ok_or_else(|| ServiceError::not_found("draft", draft_id))?;
    if let Some(bound) = &session.scope.repo
        && bound != &draft.repo
    {
        return Err(ServiceError::Invalid(format!(
            "this chat is bound to {bound}; draft {draft_id} is for {}",
            draft.repo
        )));
    }
    if let Some(done) = &draft.created_optimization {
        return Err(ServiceError::Conflict(format!(
            "draft {draft_id} already became optimization {done}"
        )));
    }
    let agents: Vec<AgentKind> = draft.agents.iter().filter_map(|a| a.parse().ok()).collect();
    let created = state
        .service
        .create_optimization(CreateOptimizationInput {
            name: Some(one_line(&draft.goal)),
            repo_url: draft.repo.clone(),
            base_ref: draft.base_ref.clone(),
            demo: None,
            agents,
            agents_where: None,
            budget_usd: draft.budget_usd,
            time_limit_seconds: 7200,
            plateau_evals: 25,
            connections: None,
            environment_draft_id: None,
        })
        .await?;
    state.mcp.mark_created(draft_id, &created.id);
    state.service.start_optimization(&created.id).await?;
    Ok(json!({"optimization": created.id, "draftId": draft_id, "started": true}))
}

/// The optimization set on the daemon (local agents). Cloud agents get theirs from `ratchet-runner mcp`.
async fn optimization(
    state: &AppState,
    session: &SessionInfo,
    tool: &str,
    arguments: Value,
) -> ServiceResult<Value> {
    let Some(id) = session.scope.optimization.as_deref() else {
        return Err(ServiceError::Invalid(
            "this session is not bound to an optimization".into(),
        ));
    };
    match tool {
        "history" => {
            let a: HistoryArgs = args(arguments)?;
            let limit = a.limit.unwrap_or(20).clamp(1, 200) as usize;
            let mut candidates = state.service.candidates(id).await?;
            let skip = candidates.len().saturating_sub(limit);
            candidates.drain(..skip);
            Ok(json!({
                "candidates": to_value(&candidates)?,
                "series": to_value(&state.service.ratchet_history(id, None).await?)?,
            }))
        }
        "steering_notes" => {
            let Some(agent) = session.scope.agent.as_deref() else {
                return Err(ServiceError::Invalid("this session has no agent".into()));
            };
            let page = state.service.agent_events(agent, None, 200).await?;
            let notes: Vec<Value> = page
                .events
                .iter()
                .filter(|e| e.event_type == "message")
                .filter_map(|e| {
                    e.text
                        .strip_prefix("You: ")
                        .map(|t| json!({"at": e.at, "text": t}))
                })
                .collect();
            Ok(json!({ "notes": notes }))
        }
        "connector_query" => {
            let a: ConnectorQueryArgs = args(arguments)?;
            let connector = a.connector.trim();
            let query = a.query.trim();
            if connector.is_empty() || query.is_empty() || query.chars().count() > 2000 {
                return Err(ServiceError::Invalid(
                    "name a connector and a query of up to 2000 characters".into(),
                ));
            }
            let Some(querier) = state.mcp.connector_querier() else {
                return Err(ServiceError::Unsupported(
                    "no connector can be asked from this daemon".into(),
                ));
            };
            let answer = querier.query(connector, query).await?;
            Ok(cap_answer(answer))
        }
        "submit_candidate" => {
            let Some(agent) = session.scope.agent.as_deref() else {
                return Err(ServiceError::Invalid("this session has no agent".into()));
            };
            let a: SubmitCandidateArgs = args(arguments)?;
            state
                .service
                .submit_candidate(agent, &a.sha, a.note)
                .await?;
            Ok(json!({
                "submitted": a.sha.trim(),
                "note": "queued: the runner submits your branch head when your turn ends",
            }))
        }
        other => Err(ServiceError::Invalid(format!("unknown tool \"{other}\""))),
    }
}

/// A connector's answer stays small enough to read in one tool result.
fn cap_answer(answer: Value) -> Value {
    const MAX: usize = 20_000;
    let text = answer.to_string();
    if text.len() <= MAX {
        return answer;
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({"truncated": true, "text": &text[..end]})
}

fn one_line(s: &str) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        out.push('…');
    }
    out
}

fn uuid_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
