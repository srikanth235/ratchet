//! The client of the `ratchetd` socket API: HTTP/1.1 over a unix socket, bearer token, typed bodies from
//! `ratchetd::api`, and the server-sent event stream.

use std::path::{Path, PathBuf};

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use ratchetd::api::{ApiError, ApiEvent, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(
        "cannot reach ratchetd at {socket}: {source} (is the daemon running? start it with `ratchetd`)"
    )]
    Connect {
        socket: String,
        source: std::io::Error,
    },
    #[error("{0}")]
    NoToken(String),
    /// The daemon answered with an error body.
    #[error("{}", .error.message)]
    Api { status: u16, error: ApiError },
    #[error("unexpected answer from ratchetd: {0}")]
    Protocol(String),
}

impl ClientError {
    /// The error class when the daemon sent one.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Api { error, .. } => Some(error.code),
            _ => None,
        }
    }
}

/// Where to find the daemon and how to authenticate.
#[derive(Debug, Clone, Default)]
pub struct Connection {
    pub socket: Option<PathBuf>,
    pub token_file: Option<PathBuf>,
    pub token: Option<String>,
}

pub struct Client {
    socket: PathBuf,
    token: String,
}

impl Client {
    /// Resolve the socket (`--socket`, else the default location) and the token (`--token`, else the token file
    /// given or the one next to the socket).
    pub fn resolve(c: &Connection) -> Result<Self, ClientError> {
        let socket = c
            .socket
            .clone()
            .unwrap_or_else(ratchetd::paths::socket_path);
        let token = match &c.token {
            Some(t) if !t.trim().is_empty() => t.trim().to_owned(),
            _ => {
                let file = c
                    .token_file
                    .clone()
                    .unwrap_or_else(|| ratchetd::paths::token_path_for_socket(&socket));
                std::fs::read_to_string(&file)
                    .map(|s| s.trim().to_owned())
                    .ok()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ClientError::NoToken(format!(
                            "cannot read the daemon's token from {} (is ratchetd running? pass --token or --token-file)",
                            file.display()
                        ))
                    })?
            }
        };
        Ok(Self { socket, token })
    }

    pub fn new(socket: impl Into<PathBuf>, token: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
            token: token.into(),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    async fn connect(
        &self,
    ) -> Result<hyper::client::conn::http1::SendRequest<Full<Bytes>>, ClientError> {
        let stream =
            UnixStream::connect(&self.socket)
                .await
                .map_err(|source| ClientError::Connect {
                    socket: self.socket.display().to_string(),
                    source,
                })?;
        let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        tokio::spawn(conn);
        Ok(sender)
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Request<Full<Bytes>>, ClientError> {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("authorization", format!("Bearer {}", self.token))
            .header("accept", "application/json");
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        b.body(Full::new(Bytes::from(body.unwrap_or_default())))
            .map_err(|e| ClientError::Protocol(e.to_string()))
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<(StatusCode, Bytes), ClientError> {
        let mut sender = self.connect().await?;
        let resp = sender
            .send_request(self.request(method, path, body)?)
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        let (parts, body) = resp.into_parts();
        let bytes = body
            .collect()
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?
            .to_bytes();
        if parts.status.is_success() {
            Ok((parts.status, bytes))
        } else {
            Err(match serde_json::from_slice::<ApiError>(&bytes) {
                Ok(error) => ClientError::Api {
                    status: parts.status.as_u16(),
                    error,
                },
                Err(_) => ClientError::Protocol(format!(
                    "HTTP {}: {}",
                    parts.status,
                    String::from_utf8_lossy(&bytes)
                        .chars()
                        .take(200)
                        .collect::<String>()
                )),
            })
        }
    }

    fn decode<T: DeserializeOwned>(bytes: &Bytes, path: &str) -> Result<T, ClientError> {
        serde_json::from_slice(bytes)
            .map_err(|e| ClientError::Protocol(format!("{path}: the answer does not parse: {e}")))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let (_, b) = self.send(Method::GET, path, None).await?;
        Self::decode(&b, path)
    }

    /// `POST path` with a JSON body; the answer is JSON.
    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let (_, b) = self.send(Method::POST, path, Some(to_vec(body)?)).await?;
        Self::decode(&b, path)
    }

    /// `POST path` without a body; the answer is JSON.
    pub async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let (_, b) = self.send(Method::POST, path, None).await?;
        Self::decode(&b, path)
    }

    pub async fn put<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let (_, b) = self.send(Method::PUT, path, Some(to_vec(body)?)).await?;
        Self::decode(&b, path)
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let (_, b) = self.send(Method::DELETE, path, None).await?;
        Self::decode(&b, path)
    }

    /// `POST path` that answers `204 No Content`.
    pub async fn post_unit<B: Serialize>(
        &self,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), ClientError> {
        let body = body.map(to_vec).transpose()?;
        self.send(Method::POST, path, body).await?;
        Ok(())
    }

    /// Open the event stream, resuming after `last_event_id`.
    pub async fn events(&self, last_event_id: Option<u64>) -> Result<EventStream, ClientError> {
        let mut sender = self.connect().await?;
        let mut req = self.request(Method::GET, "/v1/events", None)?;
        req.headers_mut().insert(
            "accept",
            "text/event-stream".parse().expect("static header"),
        );
        if let Some(id) = last_event_id {
            req.headers_mut().insert(
                "last-event-id",
                id.to_string().parse().expect("digits are a valid header"),
            );
        }
        let resp = sender
            .send_request(req)
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        if !resp.status().is_success() {
            let (parts, body) = resp.into_parts();
            let bytes = body
                .collect()
                .await
                .map_err(|e| ClientError::Protocol(e.to_string()))?
                .to_bytes();
            return Err(match serde_json::from_slice::<ApiError>(&bytes) {
                Ok(error) => ClientError::Api {
                    status: parts.status.as_u16(),
                    error,
                },
                Err(_) => ClientError::Protocol(format!("HTTP {}", parts.status)),
            });
        }
        Ok(EventStream {
            body: resp.into_body(),
            buf: String::new(),
            _sender: sender,
        })
    }
}

fn to_vec<B: Serialize>(b: &B) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(b).map_err(|e| ClientError::Protocol(e.to_string()))
}

/// A message of the event stream.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerEvent {
    /// `None` for `hello` and `resync`.
    pub id: Option<u64>,
    pub event: ApiEvent,
}

pub struct EventStream {
    body: Incoming,
    buf: String,
    _sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
}

impl EventStream {
    /// The next event; `None` when the daemon closed the stream.
    pub async fn next(&mut self) -> Option<Result<ServerEvent, ClientError>> {
        loop {
            if let Some(i) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..i + 2).collect();
                if let Some(m) = parse_block(&block) {
                    return Some(m);
                }
                continue;
            }
            match self.body.frame().await? {
                Ok(f) => {
                    if let Ok(d) = f.into_data() {
                        self.buf.push_str(&String::from_utf8_lossy(&d));
                    }
                }
                Err(e) => return Some(Err(ClientError::Protocol(e.to_string()))),
            }
        }
    }
}

fn parse_block(block: &str) -> Option<Result<ServerEvent, ClientError>> {
    let mut id = None;
    let mut data = String::new();
    for line in block.lines() {
        if line.starts_with(':') {
            continue; // keep-alive comment
        }
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let v = v.strip_prefix(' ').unwrap_or(v);
        match k {
            "id" => id = v.parse().ok(),
            "data" => {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(v);
            }
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(
        serde_json::from_str::<ApiEvent>(&data)
            .map(|event| ServerEvent { id, event })
            .map_err(|e| ClientError::Protocol(format!("unknown event: {e}"))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_blocks_parse_and_comments_are_skipped() {
        assert!(parse_block(": keep-alive\n\n").is_none());
        let m = parse_block("event: spend\nid: 7\ndata: {\"type\":\"spend\",\"optimizationId\":\"o\",\"totalUsd\":1.5}\n\n")
            .unwrap()
            .unwrap();
        assert_eq!(m.id, Some(7));
        assert_eq!(m.event.name(), "spend");
        assert!(parse_block("data: not json\n\n").unwrap().is_err());
    }

    #[test]
    fn a_missing_token_file_is_explained() {
        let dir = std::env::temp_dir().join("ratchet-cli-no-such-dir");
        let err = Client::resolve(&Connection {
            socket: Some(dir.join("ratchetd.sock")),
            ..Connection::default()
        })
        .err()
        .unwrap();
        assert!(err.to_string().contains("ratchetd.token"), "{err}");
        assert!(err.to_string().contains("is ratchetd running"));
    }

    #[test]
    fn an_explicit_token_wins() {
        let c = Client::resolve(&Connection {
            socket: Some("/nope/ratchetd.sock".into()),
            token: Some(" abc \n".into()),
            ..Connection::default()
        })
        .unwrap();
        assert_eq!(c.token, "abc");
    }
}
