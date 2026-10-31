//! Connectors (PROVISIONAL): saved GitHub / AWS / agent accounts and the per-repo selection.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiQuery, ApiResult};
use crate::api::*;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_connectors, create_connector))
        .routes(routes!(get_connector, update_connector, delete_connector))
        .routes(routes!(test_connector))
        .routes(routes!(get_selection, save_selection))
}

#[derive(Debug, serde::Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SelectionQuery {
    /// `owner/name`
    repo: String,
}

/// Every saved connector: GitHub accounts, AWS accounts, agents.
#[utoipa::path(get, path = "/v1/connectors", tag = "connectors (provisional)", operation_id = "listConnectors", responses((status = 200, body = Vec<ConnectorVm>)))]
async fn list_connectors(State(s): State<AppState>) -> ApiResult<Json<Vec<ConnectorVm>>> {
    Ok(Json(s.service.list_connectors().await?))
}

/// Add a connector. Secrets in the body go to the keychain and are never returned.
#[utoipa::path(
    post,
    path = "/v1/connectors",
    tag = "connectors (provisional)",
    operation_id = "createConnector",
    request_body = CreateConnectorInput,
    responses((status = 201, body = ConnectorVm))
)]
async fn create_connector(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<CreateConnectorInput>,
) -> ApiResult<(StatusCode, Json<ConnectorVm>)> {
    Ok((
        StatusCode::CREATED,
        Json(s.service.create_connector(input).await?),
    ))
}

/// One connector.
#[utoipa::path(
    get,
    path = "/v1/connectors/{id}",
    tag = "connectors (provisional)",
    operation_id = "getConnector",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 200, body = ConnectorVm))
)]
async fn get_connector(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ConnectorVm>> {
    Ok(Json(s.service.get_connector(&id).await?))
}

/// Rename, change the cap or region, replace the key, make default.
#[utoipa::path(
    patch,
    path = "/v1/connectors/{id}",
    tag = "connectors (provisional)",
    operation_id = "updateConnector",
    params(("id" = String, Path, description = "Connector id")),
    request_body = UpdateConnectorInput,
    responses((status = 200, body = ConnectorVm))
)]
async fn update_connector(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<UpdateConnectorInput>,
) -> ApiResult<Json<ConnectorVm>> {
    Ok(Json(s.service.update_connector(&id, input).await?))
}

/// Remove a connector and forget its secret. Optimizations already running keep what they have.
#[utoipa::path(
    delete,
    path = "/v1/connectors/{id}",
    tag = "connectors (provisional)",
    operation_id = "deleteConnector",
    params(("id" = String, Path, description = "Connector id")),
    responses((status = 204, description = "Removed"))
)]
async fn delete_connector(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    s.service.delete_connector(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// "Test" / "Sign in": probe the account and refresh its status.
#[utoipa::path(
    post,
    path = "/v1/connectors/{id}/test",
    tag = "connectors (provisional)",
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

/// The remembered connector choice for a repo, or the defaults.
#[utoipa::path(
    get,
    path = "/v1/connector-selections",
    tag = "connectors (provisional)",
    operation_id = "getConnectorSelection",
    params(SelectionQuery),
    responses((status = 200, body = ConnectorSelectionVm))
)]
async fn get_selection(
    State(s): State<AppState>,
    ApiQuery(q): ApiQuery<SelectionQuery>,
) -> ApiResult<Json<ConnectorSelectionVm>> {
    Ok(Json(s.service.connector_selection(&q.repo).await?))
}

/// Remember the connector choice for a repo ("Remembered for this repo").
#[utoipa::path(
    put,
    path = "/v1/connector-selections",
    tag = "connectors (provisional)",
    operation_id = "saveConnectorSelection",
    request_body = SaveConnectorSelectionInput,
    responses((status = 200, body = ConnectorSelectionVm))
)]
async fn save_selection(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<SaveConnectorSelectionInput>,
) -> ApiResult<Json<ConnectorSelectionVm>> {
    Ok(Json(s.service.save_connector_selection(input).await?))
}
