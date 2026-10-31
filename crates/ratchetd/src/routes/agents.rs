//! Agents: their log, pause, resume, and messages to them.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiQuery, ApiResult};
use crate::api::*;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(agent_events))
        .routes(routes!(pause_agent))
        .routes(routes!(resume_agent))
        .routes(routes!(message_agent))
}

/// An agent's log. Without `cursor`: the newest events (oldest first). With it: only events newer than the cursor.
#[utoipa::path(
    get,
    path = "/v1/agents/{id}/events",
    tag = "agents",
    operation_id = "listAgentEvents",
    params(("id" = String, Path, description = "Agent id"), AgentEventsQuery),
    responses((status = 200, body = AgentEventsPage))
)]
async fn agent_events(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<AgentEventsQuery>,
) -> ApiResult<Json<AgentEventsPage>> {
    Ok(Json(
        s.service
            .agent_events(&id, q.cursor, q.limit.unwrap_or(200).clamp(1, 1000))
            .await?,
    ))
}

/// Pause the agent after its current step.
#[utoipa::path(
    post,
    path = "/v1/agents/{id}/pause",
    tag = "agents",
    operation_id = "pauseAgent",
    params(("id" = String, Path, description = "Agent id")),
    responses((status = 204, description = "Paused"))
)]
async fn pause_agent(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.service.pause_agent(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Resume a paused agent.
#[utoipa::path(
    post,
    path = "/v1/agents/{id}/resume",
    tag = "agents",
    operation_id = "resumeAgent",
    params(("id" = String, Path, description = "Agent id")),
    responses((status = 204, description = "Resumed"))
)]
async fn resume_agent(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.service.resume_agent(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Send the agent a note; it reads it at the start of its next turn.
#[utoipa::path(
    post,
    path = "/v1/agents/{id}/messages",
    tag = "agents",
    operation_id = "messageAgent",
    params(("id" = String, Path, description = "Agent id")),
    request_body = MessageAgentInput,
    responses((status = 204, description = "Delivered"))
)]
async fn message_agent(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<MessageAgentInput>,
) -> ApiResult<StatusCode> {
    s.service.message_agent(&id, &input.text).await?;
    Ok(StatusCode::NO_CONTENT)
}
