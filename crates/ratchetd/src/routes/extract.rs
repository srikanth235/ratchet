//! Extractors and the error mapping: every failure is an [`ApiError`] body, never axum's plain text.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use crate::api::{ApiError, ErrorCode};
use crate::service::ServiceError;

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let status = match self.code() {
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
            ErrorCode::Conflict | ErrorCode::NotReady => StatusCode::CONFLICT,
            ErrorCode::Unsupported => StatusCode::NOT_IMPLEMENTED,
            ErrorCode::Upstream => StatusCode::BAD_GATEWAY,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if status.is_server_error() && status != StatusCode::NOT_IMPLEMENTED {
            tracing::error!(code = ?self.code(), "{self}");
        }
        (status, axum::Json::<ApiError>(self.into_api_error())).into_response()
    }
}

/// Handlers return this.
pub type ApiResult<T> = Result<T, ServiceError>;

/// A JSON body; a malformed one is a 400 with an `ApiError`.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ServiceError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(Self(v)),
            Err(e) => Err(json_rejection(&e)),
        }
    }
}

fn json_rejection(e: &JsonRejection) -> ServiceError {
    ServiceError::Invalid(format!("invalid JSON body: {}", e.body_text()))
}

/// An optional JSON body: absent or empty gives `T::default()`.
pub struct ApiJsonOrDefault<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Default> FromRequest<S> for ApiJsonOrDefault<T> {
    type Rejection = ServiceError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(|e| ServiceError::Invalid(format!("cannot read the body: {e}")))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(T::default()));
        }
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|e| ServiceError::Invalid(format!("invalid JSON body: {e}")))
    }
}

/// Query parameters; a bad one is a 400 with an `ApiError`.
pub struct ApiQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = ServiceError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(v)) => Ok(Self(v)),
            Err(e) => Err(query_rejection(&e)),
        }
    }
}

fn query_rejection(e: &QueryRejection) -> ServiceError {
    ServiceError::Invalid(format!("invalid query: {}", e.body_text()))
}
