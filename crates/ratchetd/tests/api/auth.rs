//! Authentication and the socket: no token or the wrong one is 401 on every route, the socket and the token file
//! are private, a second daemon cannot take a live socket, a stale socket is replaced, shutdown cleans up.

use crate::common;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use common::{Http, TestDaemon};
use hyper::StatusCode;
use ratchetd::api::{ApiError, ErrorCode};
use ratchetd::auth::{FileTokenStore, TokenStore};
use ratchetd::server::{Daemon, DaemonConfig};
use ratchetd::sim::SimService;

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// `/v1/agents/{id}/events` -> `/v1/agents/x/events`
fn concrete(path: &str) -> String {
    let mut out = String::new();
    let mut in_brace = false;
    for c in path.chars() {
        match c {
            '{' => {
                in_brace = true;
                out.push('x');
            }
            '}' => in_brace = false,
            _ if !in_brace => out.push(c),
            _ => {}
        }
    }
    out
}

#[tokio::test]
async fn every_route_needs_the_token() {
    let d = TestDaemon::start().await;
    let routes = ratchetd::openapi::route_table();
    assert!(routes.len() > 50);
    for r in &routes {
        let mut it = r.split(' ');
        let (method, path) = (it.next().unwrap(), concrete(it.next().unwrap()));
        for (label, http) in [
            ("no token", d.http.without_token()),
            ("wrong token", d.http.with_token("not-the-token")),
            ("almost the token", d.http.with_token(&d.daemon.token[..63])),
        ] {
            let reply = http.send(method, &path, None).await;
            assert_eq!(
                reply.status,
                StatusCode::UNAUTHORIZED,
                "{method} {path} with {label}"
            );
            assert_eq!(reply.headers.get("www-authenticate").unwrap(), "Bearer");
            let e: ApiError = serde_json::from_value(reply.json()).unwrap();
            assert_eq!(e.code, ErrorCode::Unauthorized);
            assert!(
                !e.message.contains(&d.daemon.token),
                "the message must not echo the token"
            );
        }
    }
    // the event stream and the document are behind it too, and so is an unknown path
    for p in ["/v1/events", "/v1/openapi.json", "/v1/nope"] {
        assert_eq!(
            d.http.without_token().get(p).await.status,
            StatusCode::UNAUTHORIZED,
            "{p}"
        );
    }
    d.stop().await;
}

#[tokio::test]
async fn the_right_token_gets_in_and_the_scheme_is_checked() {
    let d = TestDaemon::start().await;
    assert_eq!(d.http.get("/v1/status").await.status, StatusCode::OK);
    // a Basic header with the right secret is not a bearer token
    {
        use http_body_util::Full;
        use hyper::body::Bytes;
        use hyper_util::rt::TokioIo;
        let stream = tokio::net::UnixStream::connect(d.socket()).await.unwrap();
        let (mut s, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(conn);
        let req = hyper::Request::builder()
            .uri("/v1/status")
            .header("host", "localhost")
            .header("authorization", format!("Basic {}", d.daemon.token))
            .body(Full::new(Bytes::new()))
            .unwrap();
        let r = s.send_request(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    d.stop().await;
}

#[tokio::test]
async fn the_openapi_document_is_served_behind_the_token() {
    let d = TestDaemon::start().await;
    let r = d.http.get("/v1/openapi.json").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers.get("content-type").unwrap(), "application/json");
    let doc = r.json();
    assert_eq!(doc["openapi"], "3.1.0");
    assert!(doc["paths"]["/v1/optimizations"].is_object());
    assert_eq!(
        doc["components"]["securitySchemes"]["bearer"]["scheme"],
        "bearer"
    );
    d.stop().await;
}

#[tokio::test]
async fn the_socket_and_the_token_file_are_private() {
    let d = TestDaemon::start().await;
    assert_eq!(mode(&d.socket()), 0o600, "socket mode");
    let token_file = d.dir.path().join("ratchetd.token");
    assert_eq!(mode(&token_file), 0o600, "token file mode");
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap(),
        d.daemon.token
    );
    assert_eq!(d.daemon.token.len(), 64);
    d.stop().await;
}

#[tokio::test]
async fn a_new_directory_for_the_socket_is_created_private() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("fresh/Ratchet/ratchetd.sock");
    let sim = SimService::manual().await.unwrap();
    let daemon = Daemon::start(Arc::new(sim.clone()), DaemonConfig::new(&socket))
        .await
        .unwrap();
    assert_eq!(mode(socket.parent().unwrap()), 0o700);
    assert_eq!(mode(&socket), 0o600);
    sim.shutdown();
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn each_boot_has_its_own_token() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("ratchetd.sock");
    let sim = SimService::manual().await.unwrap();
    let first = Daemon::start(Arc::new(sim.clone()), DaemonConfig::new(&socket))
        .await
        .unwrap();
    let t1 = first.token.clone();
    first.stop().await.unwrap();
    let second = Daemon::start(Arc::new(sim.clone()), DaemonConfig::new(&socket))
        .await
        .unwrap();
    assert_ne!(t1, second.token);
    // the old token no longer works
    let old = Http {
        socket: socket.clone(),
        token: Some(t1),
    };
    assert_eq!(old.get("/v1/status").await.status, StatusCode::UNAUTHORIZED);
    let new = Http {
        socket,
        token: Some(second.token.clone()),
    };
    assert_eq!(new.get("/v1/status").await.status, StatusCode::OK);
    sim.shutdown();
    second.stop().await.unwrap();
}

#[tokio::test]
async fn a_live_socket_is_not_taken_over_but_a_stale_one_is_replaced() {
    let d = TestDaemon::start().await;
    let sim = SimService::manual().await.unwrap();
    let err = Daemon::start(Arc::new(sim.clone()), DaemonConfig::new(d.socket()))
        .await
        .err()
        .expect("the socket is in use");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    // the first daemon still answers, with its own token
    assert_eq!(d.http.get("/v1/status").await.status, StatusCode::OK);

    // a socket file nobody listens on (a crashed daemon) is replaced
    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("ratchetd.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    assert!(stale.exists());
    let again = Daemon::start(Arc::new(sim.clone()), DaemonConfig::new(&stale))
        .await
        .unwrap();
    let h = Http {
        socket: stale,
        token: Some(again.token.clone()),
    };
    assert_eq!(h.get("/v1/status").await.status, StatusCode::OK);
    sim.shutdown();
    again.stop().await.unwrap();
    d.stop().await;
}

#[tokio::test]
async fn shutdown_removes_the_socket_and_the_token() {
    let d = TestDaemon::start().await;
    let socket = d.socket();
    let token_file = d.dir.path().join("ratchetd.token");
    assert!(socket.exists() && token_file.exists());
    // an open event stream must not hold the shutdown up
    let mut sse = d.http.events(None).await;
    assert!(sse.next().await.is_some());
    let dir = d.dir;
    d.sim.shutdown();
    tokio::time::timeout(std::time::Duration::from_secs(5), d.daemon.stop())
        .await
        .expect("graceful shutdown with a stream open")
        .unwrap();
    assert!(!socket.exists());
    assert!(!token_file.exists());
    assert!(sse.next().await.is_none(), "the stream ended");
    drop(dir);
}

#[tokio::test]
async fn a_custom_token_store_is_used() {
    struct Mem(std::sync::Mutex<Option<String>>);
    impl TokenStore for Mem {
        fn load(&self) -> std::io::Result<Option<String>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, t: &str) -> std::io::Result<()> {
            *self.0.lock().unwrap() = Some(t.to_owned());
            Ok(())
        }
        fn clear(&self) -> std::io::Result<()> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
        fn describe(&self) -> String {
            "memory".into()
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mem(Default::default()));
    let sim = SimService::manual().await.unwrap();
    let mut cfg = DaemonConfig::new(dir.path().join("s.sock"));
    cfg.token_store = Some(store.clone());
    let daemon = Daemon::start(Arc::new(sim.clone()), cfg).await.unwrap();
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(daemon.token.as_str())
    );
    assert!(
        !dir.path().join("ratchetd.token").exists(),
        "nothing was written to disk"
    );
    sim.shutdown();
    daemon.stop().await.unwrap();
    assert_eq!(store.load().unwrap(), None);
    let _ = FileTokenStore::new(dir.path().join("x"));
}
