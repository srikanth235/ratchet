//! Settings: AWS profile, secrets, the advanced (development) overrides, connection tests.

use axum::Json;
use axum::extract::{Path, State};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiResult};
use crate::api::*;
use crate::service::ServiceError;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_settings))
        .routes(routes!(save_advanced))
        .routes(routes!(set_aws_profile))
        .routes(routes!(set_secret, clear_secret))
        .routes(routes!(test_connection))
}

/// The Settings screen: AWS profile and region, which secrets are stored, the advanced overrides, the engine.
#[utoipa::path(
    get,
    path = "/v1/settings",
    tag = "settings",
    operation_id = "getSettings",
    responses((status = 200, body = SettingsVm))
)]
async fn get_settings(State(s): State<AppState>) -> ApiResult<Json<SettingsVm>> {
    Ok(Json(s.service.settings().await?))
}

/// Region and the development overrides (AMI ids, OpenTofu paths). Validated as a whole; nothing is saved if any
/// field is invalid (400 with a `fields` entry per problem).
#[utoipa::path(
    put,
    path = "/v1/settings/advanced",
    tag = "settings",
    operation_id = "saveAdvancedSettings",
    request_body = LiveSettingsInput,
    responses((status = 200, body = SettingsVm))
)]
async fn save_advanced(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<LiveSettingsInput>,
) -> ApiResult<Json<SettingsVm>> {
    Ok(Json(s.service.save_advanced_settings(input).await?))
}

/// Choose the AWS profile Ratchet uses (an empty name clears it).
#[utoipa::path(
    put,
    path = "/v1/settings/aws-profile",
    tag = "settings",
    operation_id = "setAwsProfile",
    request_body = SetAwsProfileInput,
    responses((status = 200, body = SettingsVm))
)]
async fn set_aws_profile(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<SetAwsProfileInput>,
) -> ApiResult<Json<SettingsVm>> {
    Ok(Json(s.service.set_aws_profile(&input.profile).await?))
}

fn secret(name: &str) -> ApiResult<SecretName> {
    name.parse::<SecretName>().map_err(ServiceError::Invalid)
}

/// Store a secret in the keychain. The value is checked for format before anything is stored and never returned.
#[utoipa::path(
    put,
    path = "/v1/settings/secrets/{name}",
    tag = "settings",
    operation_id = "setSecret",
    params(("name" = SecretName, Path, description = "anthropicApiKey, openaiApiKey or githubToken")),
    request_body = SetSecretInput,
    responses((status = 200, body = SettingsVm))
)]
async fn set_secret(
    State(s): State<AppState>,
    Path(name): Path<String>,
    ApiJson(input): ApiJson<SetSecretInput>,
) -> ApiResult<Json<SettingsVm>> {
    Ok(Json(
        s.service.set_secret(secret(&name)?, &input.value).await?,
    ))
}

/// Remove a stored secret.
#[utoipa::path(
    delete,
    path = "/v1/settings/secrets/{name}",
    tag = "settings",
    operation_id = "clearSecret",
    params(("name" = SecretName, Path, description = "anthropicApiKey, openaiApiKey or githubToken")),
    responses((status = 200, body = SettingsVm))
)]
async fn clear_secret(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<SettingsVm>> {
    Ok(Json(s.service.clear_secret(secret(&name)?).await?))
}

/// A read-only probe of AWS (STS), GitHub or a model API. Secrets are read inside the daemon.
#[utoipa::path(
    post,
    path = "/v1/connection-tests",
    tag = "settings",
    operation_id = "testConnection",
    request_body = TestConnectionInput,
    responses((status = 200, body = ConnectionResultVm))
)]
async fn test_connection(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<TestConnectionInput>,
) -> ApiResult<Json<ConnectionResultVm>> {
    Ok(Json(s.service.test_connection(input.target).await?))
}
