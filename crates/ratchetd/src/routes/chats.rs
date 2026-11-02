//! Chat: create, talk, follow, interrupt, delete.

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiJsonOrDefault, ApiQuery, ApiResult};
use crate::api::*;
use crate::chat::ChatStream;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_chat))
        .routes(routes!(get_chat, delete_chat))
        .routes(routes!(send_message))
        .routes(routes!(interrupt_chat))
        .routes(routes!(stream_chat))
}

/// Start a chat with one of the user's own agent CLIs (Claude Code, Codex or Grok on this Mac). An empty body starts a Home chat
/// with no repo. The harness is the request's, else the user's chat harness, else the first one that is signed in; the answer says
/// which, and which model. 409 when no harness is ready (sign in first). With `repo` the chat reads a read-only checkout of the
/// repo's default branch; with `optimization` it is bound to that optimization (steering). The first chat with a harness carries
/// `dataNotice`.
#[utoipa::path(
    post,
    path = "/v1/chats",
    tag = "chats",
    operation_id = "createChat",
    request_body(content = CreateChatInput, description = "Optional; empty means a Home chat on the default harness"),
    responses(
        (status = 201, body = ChatVm),
        (status = 404, body = ApiError, description = "No such optimization"),
        (status = 409, body = ApiError, description = "No agent is signed in, or the one asked for is not ready")
    )
)]
async fn create_chat(
    State(s): State<AppState>,
    ApiJsonOrDefault(input): ApiJsonOrDefault<CreateChatInput>,
) -> ApiResult<(StatusCode, Json<ChatVm>)> {
    Ok((StatusCode::CREATED, Json(s.chats.create_chat(input).await?)))
}

/// A chat and its messages (oldest first), the reply being written, and what the stream would continue from.
#[utoipa::path(
    get,
    path = "/v1/chats/{id}",
    tag = "chats",
    operation_id = "getChat",
    params(("id" = String, Path, description = "Chat id"), ChatQuery),
    responses((status = 200, body = ChatVm), (status = 404, body = ApiError))
)]
async fn get_chat(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ChatQuery>,
) -> ApiResult<Json<ChatVm>> {
    Ok(Json(s.chats.get_chat(&id, &q).await?))
}

/// End the chat: stop the reply, close its session (its Ratchet tool token stops working), remove its checkout and forget it.
#[utoipa::path(
    delete,
    path = "/v1/chats/{id}",
    tag = "chats",
    operation_id = "deleteChat",
    params(("id" = String, Path, description = "Chat id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ApiError))
)]
async fn delete_chat(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.chats.delete_chat(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Say something. Answers 202 at once with the message's number; the reply arrives on the chat's stream. 409 while the chat is
/// still replying (press Stop first).
#[utoipa::path(
    post,
    path = "/v1/chats/{id}/messages",
    tag = "chats",
    operation_id = "sendChatMessage",
    params(("id" = String, Path, description = "Chat id")),
    request_body = SendChatMessageInput,
    responses(
        (status = 202, body = ChatMessageAccepted),
        (status = 409, body = ApiError, description = "The chat is still replying")
    )
)]
async fn send_message(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<SendChatMessageInput>,
) -> ApiResult<(StatusCode, Json<ChatMessageAccepted>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(s.chats.send_message(&id, input).await?),
    ))
}

/// Stop the reply being written (the Stop button). What was written stays in the chat; the stream ends the turn with `done`
/// (`outcome: interrupted`). Nothing to stop is fine.
#[utoipa::path(
    post,
    path = "/v1/chats/{id}/interrupt",
    tag = "chats",
    operation_id = "interruptChat",
    params(("id" = String, Path, description = "Chat id")),
    responses((status = 204, description = "Asked to stop"), (status = 404, body = ApiError))
)]
async fn interrupt_chat(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.chats.interrupt(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The chat's live stream (server-sent events); each `data:` is a `ChatStreamEvent`. The first event is always `ready`. Events
/// carry an `id:` that counts up per chat; reconnect with `Last-Event-ID` to receive what was missed (`ready.resync` says when it is
/// gone and the chat must be fetched again). A connection that cannot keep up is closed: reconnect. A comment line keeps it alive.
#[utoipa::path(
    get,
    path = "/v1/chats/{id}/stream",
    tag = "chats",
    operation_id = "streamChat",
    params(
        ("id" = String, Path, description = "Chat id"),
        ("Last-Event-ID" = Option<u64>, Header, description = "Id of the last event the client saw.")
    ),
    responses(
        (status = 200, description = "A stream of server-sent events; each `data:` is a ChatStreamEvent.", content_type = "text/event-stream", body = ChatStreamEvent),
        (status = 404, body = ApiError)
    )
)]
async fn stream_chat(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let last = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let ChatStream { head, live } = s.chats.stream(&id, last).await?;
    let head = stream::iter(head.into_iter().map(|(id, e)| frame(id, &e)));
    // a receiver that fell behind ends its stream: the client reconnects with Last-Event-ID instead of seeing a gap
    let live = BroadcastStream::new(live)
        .take_while(|m| std::future::ready(m.is_ok()))
        .filter_map(|m| std::future::ready(m.ok()))
        .map(|m| frame(Some(m.0), &m.1));
    let mut shutdown = s.shutdown.clone();
    let stream = head.chain(live).take_until(async move {
        let _ = shutdown.wait_for(|down| *down).await;
    });
    Ok(Sse::new(stream.map(Ok)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

fn frame(id: Option<u64>, e: &ChatStreamEvent) -> Event {
    let ev = Event::default().event(e.name());
    let ev = match id {
        Some(id) => ev.id(id.to_string()),
        None => ev,
    };
    ev.json_data(e)
        .unwrap_or_else(|_| Event::default().event("error").data("unserializable event"))
}
