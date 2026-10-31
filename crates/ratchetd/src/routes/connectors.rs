//! Connectors: saved GitHub accounts, AWS profiles and agent credentials, and the choice remembered per repo.

use axum::Json;
use axum::extract::{Path, State};
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiJsonOrDefault, ApiQuery, ApiResult};
use crate::api::*;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_connectors))
        .routes(routes!(refresh_connectors))
        .routes(routes!(add_github_connector))
        .routes(routes!(start_github_connector_sign_in, cancel_github_connector_sign_in))
        .routes(routes!(add_aws_connector))
        .routes(routes!(add_agent_connector))
        .routes(routes!(update_connector, remove_connector))
        .routes(routes!(set_default_connector))
        .routes(routes!(test_connector))
        .routes(routes!(get_selection, save_selection))
}

#[derive(Debug, serde::Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SelectionQuery {
    /// `owner/name`
    repo: String,
}

/// Every connector (detected CLI logins included), the device flow in progress and the keychain state.
#[utoipa::path(get, path = "/v1/connectors", tag = "connectors", operation_id = "listConnectors", responses((status = 200, body = ConnectorsVm)))]
async fn list_connectors(State(s): State<AppState>) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.list_connectors().await?))
}

/// Check every connector with its provider (and probe the machine for agent CLIs again), then list.
#[utoipa::path(post, path = "/v1/connectors/refresh", tag = "connectors", operation_id = "refreshConnectors", responses((status = 200, body = ConnectorsVm)))]
async fn refresh_connectors(State(s): State<AppState>) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.refresh_connectors().await?))
}

/// A pasted GitHub token (personal access token or app token), checked against GitHub before it is kept. The token
/// goes to the keychain and is never returned.
#[utoipa::path(
    post,
    path = "/v1/connectors/github",
    tag = "connectors",
    operation_id = "addGithubConnector",
    request_body = AddGithubConnectorInput,
    responses((status = 200, body = ConnectorsVm))
)]
async fn add_github_connector(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<AddGithubConnectorInput>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.add_github_connector(input).await?))
}

/// Start the device flow for a new account, or `connectorId` to sign it in again. `githubFlow` in the answer carries
/// the code to show; the daemon polls and stores the token.
#[utoipa::path(
    post,
    path = "/v1/connectors/github/sign-in",
    tag = "connectors",
    operation_id = "startGithubConnectorSignIn",
    request_body(content = StartGithubSignInInput, description = "Optional; an empty body starts a new account."),
    responses((status = 200, body = ConnectorsVm))
)]
async fn start_github_connector_sign_in(
    State(s): State<AppState>,
    ApiJsonOrDefault(input): ApiJsonOrDefault<StartGithubSignInInput>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.start_github_connector_sign_in(input).await?))
}

/// Stop the device flow in progress.
#[utoipa::path(delete, path = "/v1/connectors/github/sign-in", tag = "connectors", operation_id = "cancelGithubConnectorSignIn", responses((status = 200, body = ConnectorsVm)))]
async fn cancel_github_connector_sign_in(
    State(s): State<AppState>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.cancel_github_connector_sign_in().await?))
}

/// Connect an AWS profile from `~/.aws` with a region and an optional monthly cap.
#[utoipa::path(
    post,
    path = "/v1/connectors/aws",
    tag = "connectors",
    operation_id = "addAwsConnector",
    request_body = AddAwsConnectorInput,
    responses((status = 200, body = ConnectorsVm))
)]
async fn add_aws_connector(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<AddAwsConnectorInput>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.add_aws_connector(input).await?))
}

/// A detected CLI login (one that was removed) or a pasted API key. The key goes to the keychain and is never
/// returned.
#[utoipa::path(
    post,
    path = "/v1/connectors/agents",
    tag = "connectors",
    operation_id = "addAgentConnector",
    request_body = AddAgentConnectorInput,
    responses((status = 200, body = ConnectorsVm))
)]
async fn add_agent_connector(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<AddAgentConnectorInput>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.add_agent_connector(input).await?))
}

/// Rename; AWS: region and monthly cap (`null` removes the cap).
#[utoipa::path(
    patch,
    path = "/v1/connectors/{id}",
    tag = "connectors",
    operation_id = "updateConnector",
    params(("id" = String, Path, description = "Connector id")),
    request_body = UpdateConnectorInput,
    responses((status = 200, body = ConnectorsVm))
)]
async fn update_connector(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<UpdateConnectorInput>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.update_connector(&id, input).await?))
}

/// Remove a connector and forget its secret. Optimizations already running keep what they have.
#[utoipa::path(
    delete,
    path = "/v1/connectors/{id}",
    tag = "connectors",
    operation_id = "removeConnector",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 200, body = ConnectorsVm))
)]
async fn remove_connector(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.remove_connector(&id).await?))
}

/// Make this connector the default of its kind.
#[utoipa::path(
    post,
    path = "/v1/connectors/{id}/default",
    tag = "connectors",
    operation_id = "setDefaultConnector",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 200, body = ConnectorsVm))
)]
async fn set_default_connector(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ConnectorsVm>> {
    Ok(Json(s.service.set_default_connector(&id).await?))
}

/// "Test connection": probe the account with its provider and remember the status.
#[utoipa::path(
    post,
    path = "/v1/connectors/{id}/test",
    tag = "connectors",
    operation_id = "testConnector",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 200, body = ConnectorTestVm))
)]
async fn test_connector(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ConnectorTestVm>> {
    Ok(Json(s.service.test_connector(&id).await?))
}

/// The choice remembered for a repo, and what a run on it would use now (saved choice, else the defaults).
#[utoipa::path(
    get,
    path = "/v1/connector-selections",
    tag = "connectors",
    operation_id = "getConnectorSelection",
    params(SelectionQuery),
    responses((status = 200, body = RepoConnectionsVm))
)]
async fn get_selection(
    State(s): State<AppState>,
    ApiQuery(q): ApiQuery<SelectionQuery>,
) -> ApiResult<Json<RepoConnectionsVm>> {
    Ok(Json(s.service.repo_connections(&q.repo).await?))
}

/// Check and remember the choice for a repo. 400 when it cannot run: an unknown id, a login in the cloud, two
/// connectors for one agent, no agent.
#[utoipa::path(
    put,
    path = "/v1/connector-selections",
    tag = "connectors",
    operation_id = "saveConnectorSelection",
    request_body = SaveRepoConnectionsInput,
    responses((status = 200, body = RepoConnectionsVm))
)]
async fn save_selection(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<SaveRepoConnectionsInput>,
) -> ApiResult<Json<RepoConnectionsVm>> {
    Ok(Json(
        s.service
            .save_repo_connections(&input.repo, input.selection)
            .await?,
    ))
}
