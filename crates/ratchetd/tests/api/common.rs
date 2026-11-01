//! Shared helpers of the integration tests: an in-process daemon on a temp socket and a tiny HTTP client over it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use ratchetd::server::{Daemon, DaemonConfig};
use ratchetd::sim::{SimOptions, SimService};
use serde_json::Value;
use tokio::net::UnixStream;

/// A daemon serving a manual (timer-free) simulation on a socket in a temp dir.
pub struct TestDaemon {
    pub dir: tempfile::TempDir,
    pub daemon: Daemon,
    pub sim: SimService,
    pub http: Http,
}

impl TestDaemon {
    pub async fn start() -> Self {
        Self::with(SimOptions {
            auto_run: false,
            github_flow_ms: 0,
            ..SimOptions::default()
        })
        .await
    }

    pub async fn with(opts: SimOptions) -> Self {
        Self::with_window(opts, ratchetd::hub::DEFAULT_WINDOW).await
    }

    pub async fn with_window(opts: SimOptions, window: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sim = SimService::new(opts).await.unwrap();
        let socket = dir.path().join("ratchetd.sock");
        let mut cfg = DaemonConfig::new(&socket);
        cfg.event_window = window;
        let daemon = Daemon::start(Arc::new(sim.clone()), cfg).await.unwrap();
        let http = Http {
            socket,
            token: Some(daemon.token.clone()),
        };
        Self {
            dir,
            daemon,
            sim,
            http,
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.http.socket.clone()
    }

    pub async fn stop(self) {
        self.sim.shutdown();
        self.daemon.stop().await.unwrap();
    }
}

#[derive(Clone)]
pub struct Http {
    pub socket: PathBuf,
    pub token: Option<String>,
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: hyper::HeaderMap,
    pub body: Bytes,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl Http {
    pub fn without_token(&self) -> Self {
        Self {
            token: None,
            ..self.clone()
        }
    }

    pub fn with_token(&self, token: &str) -> Self {
        Self {
            token: Some(token.to_owned()),
            ..self.clone()
        }
    }

    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Request<Full<Bytes>> {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost");
        if let Some(t) = &self.token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let payload = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Bytes::from(serde_json::to_vec(v).unwrap())
            }
            None => Bytes::new(),
        };
        b.body(Full::new(payload)).unwrap()
    }

    async fn connect(&self) -> hyper::client::conn::http1::SendRequest<Full<Bytes>> {
        let stream = UnixStream::connect(&self.socket).await.unwrap();
        let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(conn);
        sender
    }

    pub async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let mut sender = self.connect().await;
        let resp = sender
            .send_request(self.request(method, path, body.as_ref()))
            .await
            .unwrap();
        let (parts, body) = resp.into_parts();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body: body.collect().await.unwrap().to_bytes(),
        }
    }

    pub async fn get(&self, path: &str) -> Reply {
        self.send("GET", path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Reply {
        self.send("POST", path, Some(body)).await
    }

    /// GET `path` and expect 200 + JSON.
    pub async fn get_json(&self, path: &str) -> Value {
        let r = self.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "GET {path}: {}", r.text());
        r.json()
    }

    /// Open the event stream; `last_event_id` is sent as the `Last-Event-ID` header.
    pub async fn events(&self, last_event_id: Option<u64>) -> SseReader {
        let mut sender = self.connect().await;
        let mut req = self.request("GET", "/v1/events", None);
        if let Some(id) = last_event_id {
            req.headers_mut()
                .insert("last-event-id", id.to_string().parse().unwrap());
        }
        let resp: Response<Incoming> = sender.send_request(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/event-stream"
        );
        SseReader {
            body: resp.into_body(),
            buf: String::new(),
            _sender: sender,
        }
    }
}

/// One server-sent message.
#[derive(Debug, Clone, PartialEq)]
pub struct Sse {
    pub event: Option<String>,
    pub id: Option<u64>,
    pub data: Value,
}

pub struct SseReader {
    body: Incoming,
    buf: String,
    _sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
}

impl SseReader {
    /// The next message, skipping keep-alive comments; `None` when the stream ended.
    pub async fn next(&mut self) -> Option<Sse> {
        loop {
            if let Some(i) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..i + 2).collect();
                if let Some(m) = parse_block(&block) {
                    return Some(m);
                }
                continue;
            }
            let frame = tokio::time::timeout(Duration::from_secs(10), self.body.frame())
                .await
                .expect("timed out waiting for an event")?;
            match frame {
                Ok(f) => {
                    if let Ok(d) = f.into_data() {
                        self.buf.push_str(&String::from_utf8_lossy(&d));
                    }
                }
                Err(_) => return None,
            }
        }
    }

    /// Read until a message of type `ty` arrives.
    pub async fn until(&mut self, ty: &str) -> Sse {
        loop {
            let m = self
                .next()
                .await
                .unwrap_or_else(|| panic!("stream ended before {ty}"));
            if m.event.as_deref() == Some(ty) {
                return m;
            }
        }
    }
}

fn parse_block(block: &str) -> Option<Sse> {
    let mut event = None;
    let mut id = None;
    let mut data = String::new();
    for line in block.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let v = v.strip_prefix(' ').unwrap_or(v);
        match k {
            "event" => event = Some(v.to_owned()),
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
    if event.is_none() && data.is_empty() {
        return None; // a comment-only block
    }
    Some(Sse {
        event,
        id,
        data: serde_json::from_str(&data).unwrap_or(Value::String(data)),
    })
}
