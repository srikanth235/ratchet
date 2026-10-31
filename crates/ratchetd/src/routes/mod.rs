//! The HTTP routes: thin handlers that decode a request, call one [`AppService`](crate::service::AppService)
//! method and encode the answer. Every handler carries its `#[utoipa::path]`, so the OpenAPI document is generated
//! from the same source as the router (`crate::openapi`).

mod agents;
mod connectors;
mod drafts;
pub mod extract;
mod misc;
mod optimizations;
mod repos;
mod settings;

use std::sync::Arc;

use tokio::sync::watch;
use utoipa_axum::router::OpenApiRouter;

use crate::hub::EventHub;
use crate::service::AppService;

/// Shared by every handler.
#[derive(Clone)]
pub struct AppState {
    pub service: Arc<dyn AppService>,
    pub hub: EventHub,
    /// Epoch ms the daemon started.
    pub started_at: i64,
    pub socket_path: Option<String>,
    /// Flips to `true` on shutdown: SSE streams end so the server can drain.
    pub shutdown: watch::Receiver<bool>,
}

/// All routes (without the auth layer and without `/v1/openapi.json`).
pub fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(misc::router())
        .merge(optimizations::router())
        .merge(agents::router())
        .merge(settings::router())
        .merge(repos::router())
        .merge(connectors::router())
        .merge(drafts::router())
}
