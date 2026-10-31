//! Status, the event stream, overview, config validation, preflight, activity.

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiQuery, ApiResult};
use crate::api::*;
use crate::hub::Subscription;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(events))
        .routes(routes!(overview))
        .routes(routes!(validate_config))
        .routes(routes!(preflight))
        .routes(routes!(activity))
}

/// Who is answering: version, engine, uptime, where the socket and the ledger are.
#[utoipa::path(
    get,
    path = "/v1/status",
    tag = "status",
    operation_id = "getStatus",
    responses((status = 200, body = DaemonStatus))
)]
async fn status(State(s): State<AppState>) -> Json<DaemonStatus> {
    let now = now_ms();
    Json(DaemonStatus {
        version: crate::VERSION.to_owned(),
        api_version: API_VERSION.to_owned(),
        engine: s.service.engine(),
        pid: std::process::id(),
        started_at: s.started_at,
        uptime_s: ((now - s.started_at) / 1000).max(0),
        socket_path: s.socket_path.clone(),
        ledger_path: s.service.ledger_path(),
        last_event_id: s.hub.last_id(),
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The live event stream (server-sent events).
///
/// Each message has `event:` = the `type` of the payload, `id:` = a sequence number and `data:` = the JSON of an
/// `ApiEvent`. Reconnect with `Last-Event-ID` to receive what was missed; when the daemon cannot replay it (it
/// restarted, or the gap is older than its window) the stream starts with a `resync` event and the client refetches
/// its snapshots. The first message is always `hello` (it has no `id`). A comment line keeps the connection alive.
#[utoipa::path(
    get,
    path = "/v1/events",
    tag = "events",
    operation_id = "streamEvents",
    params(("Last-Event-ID" = Option<u64>, Header, description = "Id of the last event the client saw.")),
    responses((status = 200, description = "A stream of server-sent events; each `data:` is an ApiEvent.", content_type = "text/event-stream", body = ApiEvent))
)]
async fn events(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let last = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let Subscription {
        last_id,
        resync,
        replay,
        live,
    } = s.hub.subscribe(last);

    let hello = ApiEvent::Hello {
        last_event_id: last_id,
        server_time: now_ms(),
        version: crate::VERSION.to_owned(),
    };
    let mut head = vec![frame(None, &hello)];
    if let Some(reason) = resync {
        head.push(frame(None, &ApiEvent::Resync { reason }));
    }
    head.extend(replay.iter().map(|e| frame(Some(e.id), &e.event)));

    let mut shutdown = s.shutdown.clone();
    let live = BroadcastStream::new(live).map(move |m| match m {
        Ok(e) => frame(Some(e.id), &e.event),
        Err(_) => frame(
            None,
            &ApiEvent::Resync {
                reason: "this connection fell behind and events were dropped".into(),
            },
        ),
    });
    let stream = stream::iter(head).chain(live).take_until(async move {
        let _ = shutdown.wait_for(|down| *down).await;
    });
    Sse::new(stream.map(Ok)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn frame(id: Option<u64>, e: &ApiEvent) -> Event {
    let ev = Event::default().event(e.name());
    let ev = match id {
        Some(id) => ev.id(id.to_string()),
        None => ev,
    };
    ev.json_data(e)
        .unwrap_or_else(|_| Event::default().event("error").data("unserializable event"))
}

/// Overview: spent, value locked in, return, the value staircase, what needs you, the table. PROVISIONAL.
#[utoipa::path(
    get,
    path = "/v1/overview",
    tag = "overview (provisional)",
    operation_id = "getOverview",
    params(OverviewQuery),
    responses((status = 200, body = OverviewVm))
)]
async fn overview(
    State(s): State<AppState>,
    ApiQuery(q): ApiQuery<OverviewQuery>,
) -> ApiResult<Json<OverviewVm>> {
    Ok(Json(s.service.overview(q.range.unwrap_or_default()).await?))
}

/// Parse ratchet.yaml text (read-only; the repo owns the file). Always 200: problems are in the result.
#[utoipa::path(
    post,
    path = "/v1/config/validations",
    tag = "config",
    operation_id = "validateConfig",
    request_body = ValidateConfigInput,
    responses((status = 200, body = ValidateConfigResult))
)]
async fn validate_config(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<ValidateConfigInput>,
) -> ApiResult<Json<ValidateConfigResult>> {
    if input.yaml.len() > 200_000 {
        return Err(crate::service::ServiceError::Invalid(
            "ratchet.yaml is larger than 200 kB".into(),
        ));
    }
    Ok(Json(s.service.validate_config(&input.yaml).await?))
}

/// "What's missing" for a run: the short list on the New optimization page.
#[utoipa::path(
    post,
    path = "/v1/preflight",
    tag = "config",
    operation_id = "runPreflight",
    request_body = PreflightInput,
    responses((status = 200, body = Vec<PreflightItem>))
)]
async fn preflight(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<PreflightInput>,
) -> ApiResult<Json<Vec<PreflightItem>>> {
    if input.agents.len() > 8 {
        return Err(crate::service::ServiceError::Invalid(
            "at most 8 agents".into(),
        ));
    }
    Ok(Json(s.service.preflight(input).await?))
}

/// The merged, newest-first feed of verdicts and verified wins.
#[utoipa::path(
    get,
    path = "/v1/activity",
    tag = "optimizations",
    operation_id = "listActivity",
    params(ActivityQuery),
    responses((status = 200, body = Vec<ActivityVm>))
)]
async fn activity(
    State(s): State<AppState>,
    ApiQuery(q): ApiQuery<ActivityQuery>,
) -> ApiResult<Json<Vec<ActivityVm>>> {
    Ok(Json(
        s.service
            .recent_activity(q.optimization_id, q.limit.unwrap_or(20).clamp(1, 200))
            .await?,
    ))
}
