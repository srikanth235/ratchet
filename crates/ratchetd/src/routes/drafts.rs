//! Dynamic environment drafts (PROVISIONAL): draft, read, ask for changes, approve, open as a pull request.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiResult};
use crate::api::*;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_draft))
        .routes(routes!(get_draft))
        .routes(routes!(draft_files))
        .routes(routes!(revise_draft))
        .routes(routes!(approve_draft))
        .routes(routes!(open_draft_pr))
}

/// Ask an agent to draft the environment this goal needs (scope none, single or full, resources, checks, cost).
#[utoipa::path(
    post,
    path = "/v1/environment-drafts",
    tag = "environment drafts (provisional)",
    operation_id = "createEnvironmentDraft",
    request_body = CreateEnvironmentDraftInput,
    responses((status = 201, body = EnvironmentDraftVm))
)]
async fn create_draft(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<CreateEnvironmentDraftInput>,
) -> ApiResult<(StatusCode, Json<EnvironmentDraftVm>)> {
    Ok((
        StatusCode::CREATED,
        Json(s.service.create_environment_draft(input).await?),
    ))
}

/// One environment draft.
#[utoipa::path(
    get,
    path = "/v1/environment-drafts/{id}",
    tag = "environment drafts (provisional)",
    operation_id = "getEnvironmentDraft",
    params(("id" = String, Path, description = "Draft id")),
    responses((status = 200, body = EnvironmentDraftVm))
)]
async fn get_draft(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EnvironmentDraftVm>> {
    Ok(Json(s.service.get_environment_draft(&id).await?))
}

/// "View files": the files the draft adds, with their content.
#[utoipa::path(
    get,
    path = "/v1/environment-drafts/{id}/files",
    tag = "environment drafts (provisional)",
    operation_id = "listEnvironmentDraftFiles",
    params(("id" = String, Path, description = "Draft id")),
    responses((status = 200, body = EnvFilesVm))
)]
async fn draft_files(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EnvFilesVm>> {
    Ok(Json(s.service.environment_draft_files(&id).await?))
}

/// "Ask for a change" or "Use a smaller setup": the agent revises the draft. 409 once approved.
#[utoipa::path(
    post,
    path = "/v1/environment-drafts/{id}/revisions",
    tag = "environment drafts (provisional)",
    operation_id = "reviseEnvironmentDraft",
    params(("id" = String, Path, description = "Draft id")),
    request_body = ReviseEnvironmentDraftInput,
    responses((status = 200, body = EnvironmentDraftVm))
)]
async fn revise_draft(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<ReviseEnvironmentDraftInput>,
) -> ApiResult<Json<EnvironmentDraftVm>> {
    Ok(Json(s.service.revise_environment_draft(&id, input).await?))
}

/// Approve and lock the environment: agents change code only, never the environment. 409 unless every check passed.
#[utoipa::path(
    post,
    path = "/v1/environment-drafts/{id}/approval",
    tag = "environment drafts (provisional)",
    operation_id = "approveEnvironmentDraft",
    params(("id" = String, Path, description = "Draft id")),
    responses((status = 200, body = EnvironmentDraftVm), (status = 409, body = ApiError, description = "Not ready"))
)]
async fn approve_draft(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EnvironmentDraftVm>> {
    Ok(Json(s.service.approve_environment_draft(&id).await?))
}

/// "Open as PR": add the environment files to the repo through a pull request.
#[utoipa::path(
    post,
    path = "/v1/environment-drafts/{id}/pull-request",
    tag = "environment drafts (provisional)",
    operation_id = "openEnvironmentDraftPullRequest",
    params(("id" = String, Path, description = "Draft id")),
    responses((status = 200, body = EnvironmentDraftVm))
)]
async fn open_draft_pr(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EnvironmentDraftVm>> {
    Ok(Json(s.service.open_environment_draft_pr(&id).await?))
}
