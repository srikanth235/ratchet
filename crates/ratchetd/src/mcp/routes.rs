//! `/v1/mcp/*`: the only routes a session token opens. They sit outside the owner-token layer and are not part of the app's
//! OpenAPI contract (the Mac app never calls them; the `ratchetd mcp` child process does).

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::SessionInfo;
use crate::routes::AppState;
use crate::routes::extract::{ApiJson, ApiResult};
use crate::service::ServiceError;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/mcp/session", get(describe))
        .route("/v1/mcp/call", post(call))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ))
        .with_state(state)
}

fn bearer(value: Option<&HeaderValue>) -> Option<&str> {
    let (scheme, token) = value?.to_str().ok()?.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

async fn require_session(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(info) =
        bearer(req.headers().get(header::AUTHORIZATION)).and_then(|t| s.mcp.sessions.resolve(t))
    else {
        return crate::auth::unauthorized("not a valid MCP session token");
    };
    req.extensions_mut().insert(info);
    next.run(req).await
}

async fn describe(Extension(session): Extension<SessionInfo>) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "id": session.id,
            "set": session.set,
            "repo": session.scope.repo,
            "optimization": session.scope.optimization,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct CallBody {
    tool: String,
    #[serde(default)]
    arguments: Value,
}

async fn call(
    State(s): State<AppState>,
    Extension(session): Extension<SessionInfo>,
    ApiJson(body): ApiJson<CallBody>,
) -> ApiResult<Json<Value>> {
    let arguments = if body.arguments.is_null() {
        json!({})
    } else {
        body.arguments
    };
    // The daemon is the authority: it checks the set again, whatever the child process believes.
    let Some(tool) = session.set.find(&body.tool) else {
        return Err(ServiceError::Invalid(format!(
            "tool \"{}\" is not available in this session",
            body.tool
        )));
    };
    if let Some(validate) = tool.validate {
        validate(&arguments).map_err(ServiceError::Invalid)?;
    }
    let result = super::tools::call(&s, &session, &body.tool, arguments).await?;
    Ok(Json(json!({ "result": result })))
}
