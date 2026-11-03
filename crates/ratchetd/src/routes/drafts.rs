//! Dynamic environment drafts: draft, list, read, ask for changes, approve (commit or pull request), discard.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiJsonOrDefault, ApiResult};
use crate::api::*;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_draft, list_drafts))
        .routes(routes!(get_draft, discard_draft))
        .routes(routes!(revise_draft))
        .routes(routes!(approve_draft))
}

/// Ask the setup agent to draft the environment this goal needs (scope none, single or full, resources, checks, cost).
/// Answers at once with the draft in status `drafting`; the agent works in the background and `environment-draft`
/// events say when it is `ready` (or `failed`).
#[utoipa::path(
    post,
    path = "/v1/environment-drafts",
    tag = "environment drafts",
    operation_id = "createEnvironmentDraft",
    request_body = CreateEnvironmentDraftInput,
    responses(
        (status = 201, body = EnvironmentDraftVm),
        (status = 409, body = ApiError, description = "Something to set up first (sign in, an agent connector)"),
        (status = 501, body = ApiError, description = "This engine has no setup agent")
    )
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

/// Every environment draft, newest first.
#[utoipa::path(
    get,
    path = "/v1/environment-drafts",
    tag = "environment drafts",
    operation_id = "listEnvironmentDrafts",
    responses((status = 200, body = EnvironmentDraftsVm))
)]
async fn list_drafts(State(s): State<AppState>) -> ApiResult<Json<EnvironmentDraftsVm>> {
    Ok(Json(s.service.list_environment_drafts().await?))
}

/// One environment draft, with the content of its files ("View files").
#[utoipa::path(
    get,
    path = "/v1/environment-drafts/{id}",
    tag = "environment drafts",
    operation_id = "getEnvironmentDraft",
    params(("id" = String, Path, description = "Draft id")),
    responses((status = 200, body = EnvironmentDraftVm), (status = 404, body = ApiError))
)]
async fn get_draft(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EnvironmentDraftVm>> {
    Ok(Json(s.service.get_environment_draft(&id).await?))
}

/// "Back": forget a draft that was not approved. 409 once approved.
#[utoipa::path(
    delete,
    path = "/v1/environment-drafts/{id}",
    tag = "environment drafts",
    operation_id = "discardEnvironmentDraft",
    params(("id" = String, Path, description = "Draft id")),
    responses((status = 204, description = "Discarded"), (status = 409, body = ApiError, description = "Approved"))
)]
async fn discard_draft(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.service.discard_environment_draft(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// "Ask for a change" or "Use a smaller setup": the same agent session continues and the draft is re-validated and
/// re-costed. Answers with the draft in status `drafting`; 409 once approved or while it is being drafted.
#[utoipa::path(
    post,
    path = "/v1/environment-drafts/{id}/revisions",
    tag = "environment drafts",
    operation_id = "reviseEnvironmentDraft",
    params(("id" = String, Path, description = "Draft id")),
    request_body = ReviseEnvironmentDraftInput,
    responses((status = 200, body = EnvironmentDraftVm), (status = 409, body = ApiError, description = "Approved or being drafted"))
)]
async fn revise_draft(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<ReviseEnvironmentDraftInput>,
) -> ApiResult<Json<EnvironmentDraftVm>> {
    Ok(Json(s.service.revise_environment_draft(&id, input).await?))
}

/// Approve and lock the environment: the files are committed to the protected ref (`mode` `commit`) or proposed as a
/// pull request (`pr`, the default), and from then on agents change code only, never the environment. 409 unless
/// every check passed (and unless the base `ratchet.yaml` is still the one the draft was made from).
#[utoipa::path(
    post,
    path = "/v1/environment-drafts/{id}/approval",
    tag = "environment drafts",
    operation_id = "approveEnvironmentDraft",
    params(("id" = String, Path, description = "Draft id")),
    request_body(content = ApproveEnvironmentDraftInput, description = "Optional; empty means a pull request"),
    responses((status = 200, body = EnvironmentDraftVm), (status = 409, body = ApiError, description = "Not ready"))
)]
async fn approve_draft(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJsonOrDefault(input): ApiJsonOrDefault<ApproveEnvironmentDraftInput>,
) -> ApiResult<Json<EnvironmentDraftVm>> {
    Ok(Json(s.service.approve_environment_draft(&id, input).await?))
}
