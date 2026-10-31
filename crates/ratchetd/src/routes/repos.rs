//! GitHub, repos, AWS profiles, agent CLIs, templates (benchmarks), environments.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use ratchet_core::ports::{ProposeTemplateInput, ProposeTemplateResult};
use ratchet_core::templates::TemplateInfo;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiQuery, ApiResult};
use crate::api::*;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(github_status))
        .routes(routes!(github_sign_in))
        .routes(routes!(github_sign_out))
        .routes(routes!(list_repos))
        .routes(routes!(inspect_repo))
        .routes(routes!(aws_profiles))
        .routes(routes!(list_harnesses))
        .routes(routes!(detect_harnesses))
        .routes(routes!(list_templates))
        .routes(routes!(get_template))
        .routes(routes!(propose_template))
        .routes(routes!(list_environments))
        .routes(routes!(end_environment))
}

/// The GitHub row of Settings.
#[utoipa::path(get, path = "/v1/github/status", tag = "github", operation_id = "getGithubStatus", responses((status = 200, body = GithubStatusVm)))]
async fn github_status(State(s): State<AppState>) -> ApiResult<Json<GithubStatusVm>> {
    Ok(Json(s.service.github_status().await?))
}

/// Start "Sign in with GitHub" (device flow): returns the code to show; the daemon polls and stores the token.
#[utoipa::path(post, path = "/v1/github/sign-in", tag = "github", operation_id = "startGithubSignIn", responses((status = 200, body = GithubStatusVm)))]
async fn github_sign_in(State(s): State<AppState>) -> ApiResult<Json<GithubStatusVm>> {
    Ok(Json(s.service.github_sign_in().await?))
}

/// Sign out of GitHub and forget the token.
#[utoipa::path(delete, path = "/v1/github/session", tag = "github", operation_id = "signOutOfGithub", responses((status = 200, body = GithubStatusVm)))]
async fn github_sign_out(State(s): State<AppState>) -> ApiResult<Json<GithubStatusVm>> {
    Ok(Json(s.service.github_sign_out().await?))
}

/// The signed-in account's repos, optionally filtered by a substring of `owner/name`. 409 when not signed in.
#[utoipa::path(
    get,
    path = "/v1/github/repos",
    tag = "github",
    operation_id = "listRepos",
    params(ReposQuery),
    responses((status = 200, body = Vec<RepoVm>))
)]
async fn list_repos(
    State(s): State<AppState>,
    ApiQuery(q): ApiQuery<ReposQuery>,
) -> ApiResult<Json<Vec<RepoVm>>> {
    Ok(Json(s.service.list_repos(q.query, q.connector_id).await?))
}

/// What step 1 of New optimization learns about a repo (a GitHub repo or a local folder): its ratchet.yaml, a
/// summary, or the templates to add one.
#[utoipa::path(
    post,
    path = "/v1/repos/inspections",
    tag = "github",
    operation_id = "inspectRepo",
    request_body = InspectRepoInput,
    responses((status = 200, body = RepoInspectVm))
)]
async fn inspect_repo(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<InspectRepoInput>,
) -> ApiResult<Json<RepoInspectVm>> {
    Ok(Json(s.service.inspect_repo(input.source, input.connector_id).await?))
}

/// The AWS profiles found on this Mac (names only).
#[utoipa::path(get, path = "/v1/aws/profiles", tag = "aws", operation_id = "listAwsProfiles", responses((status = 200, body = AwsProfilesVm)))]
async fn aws_profiles(State(s): State<AppState>) -> ApiResult<Json<AwsProfilesVm>> {
    Ok(Json(s.service.aws_profiles().await?))
}

/// The detected agent CLIs (Claude Code, Codex): installed, version, signed in.
#[utoipa::path(get, path = "/v1/harnesses", tag = "harnesses", operation_id = "listHarnesses", responses((status = 200, body = Vec<HarnessVm>)))]
async fn list_harnesses(State(s): State<AppState>) -> ApiResult<Json<Vec<HarnessVm>>> {
    Ok(Json(s.service.harnesses(false).await?))
}

/// "Re-detect": probe the machine again.
#[utoipa::path(post, path = "/v1/harnesses/detections", tag = "harnesses", operation_id = "detectHarnesses", responses((status = 200, body = Vec<HarnessVm>)))]
async fn detect_harnesses(State(s): State<AppState>) -> ApiResult<Json<Vec<HarnessVm>>> {
    Ok(Json(s.service.harnesses(true).await?))
}

/// The evaluator template gallery (Benchmarks): runnable templates, then the "Try a demo" entries.
#[utoipa::path(
    get,
    path = "/v1/templates",
    tag = "templates",
    operation_id = "listTemplates",
    params(TemplatesQuery),
    responses((status = 200, body = Vec<TemplateInfo>))
)]
async fn list_templates(
    State(s): State<AppState>,
    ApiQuery(q): ApiQuery<TemplatesQuery>,
) -> ApiResult<Json<Vec<TemplateInfo>>> {
    Ok(Json(s.service.list_templates(q.kind).await?))
}

/// One template by id (a legacy `--pack` name is accepted).
#[utoipa::path(
    get,
    path = "/v1/templates/{id}",
    tag = "templates",
    operation_id = "getTemplate",
    params(("id" = String, Path, description = "Template id, e.g. http-latency")),
    responses((status = 200, body = TemplateInfo))
)]
async fn get_template(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TemplateInfo>> {
    Ok(Json(s.service.get_template(&id).await?))
}

/// "Use template" / "Add via pull request": commit the template's files on a branch of the repo and open the PR.
#[utoipa::path(
    post,
    path = "/v1/template-proposals",
    tag = "templates",
    operation_id = "proposeTemplate",
    request_body = ProposeTemplateInput,
    responses((status = 201, body = ProposeTemplateResult))
)]
async fn propose_template(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<ProposeTemplateInput>,
) -> ApiResult<(StatusCode, Json<ProposeTemplateResult>)> {
    Ok((
        StatusCode::CREATED,
        Json(s.service.propose_template(input).await?),
    ))
}

/// What is running and what it costs: the automatic environments and the last teardown receipt.
#[utoipa::path(get, path = "/v1/environments", tag = "environments", operation_id = "listEnvironments", responses((status = 200, body = EnvironmentsVm)))]
async fn list_environments(State(s): State<AppState>) -> ApiResult<Json<EnvironmentsVm>> {
    Ok(Json(s.service.environments().await?))
}

/// "End sandbox now": tear the environment down without waiting for the idle window. 409 while a run is active.
#[utoipa::path(
    post,
    path = "/v1/environments/{sandboxId}/end",
    tag = "environments",
    operation_id = "endEnvironment",
    params(("sandboxId" = String, Path, description = "Sandbox id")),
    responses((status = 200, body = EnvironmentsVm), (status = 409, body = ApiError, description = "A run is still active"))
)]
async fn end_environment(
    State(s): State<AppState>,
    Path(sandbox_id): Path<String>,
) -> ApiResult<Json<EnvironmentsVm>> {
    Ok(Json(s.service.end_environment(&sandbox_id).await?))
}
