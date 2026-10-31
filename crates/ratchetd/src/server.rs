//! The server: axum on a unix socket (mode 0600), the bearer-token layer, graceful shutdown.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::auth::{
    FileTokenStore, TokenAuth, TokenStore, create_private_dir, provision_token, require_token,
};
use crate::hub::{DEFAULT_WINDOW, EventHub};
use crate::routes::{AppState, api_router};
use crate::service::{AppService, ServiceError};

/// The complete router: every route, `/v1/openapi.json`, the 404 body, and the auth layer around all of it.
pub fn build_router(state: AppState, auth: TokenAuth) -> Router {
    let (router, _) = api_router().split_for_parts();
    let spec = Arc::new(crate::openapi::openapi_json());
    router
        .route(
            "/v1/openapi.json",
            axum::routing::get(|State(spec): State<Arc<String>>| async move {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    spec.to_string(),
                )
            })
            .with_state(spec),
        )
        .fallback(not_found)
        .with_state(state)
        .layer(middleware::from_fn_with_state(auth, require_token))
}

async fn not_found() -> Response {
    ServiceError::NotFound("no such route".into()).into_response()
}

/// Bind the unix socket: private parent directory, a stale socket file removed (but not a live daemon's), mode 0600.
pub async fn bind(path: &Path) -> io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "another ratchetd is already listening on {}",
                    path.display()
                ),
            ));
        }
        std::fs::remove_file(path)?;
    }
    // No window in which the socket is more open than 0600, even in a directory we did not create.
    // SAFETY: umask only changes this process's file creation mask.
    let old = unsafe { libc::umask(0o177) };
    let bound = UnixListener::bind(path);
    unsafe { libc::umask(old) };
    let listener = bound?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Where and how a daemon starts.
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    /// Where the per-boot token goes; default a file next to the socket.
    pub token_store: Option<Arc<dyn TokenStore>>,
    /// Events a reconnecting client can still catch up on.
    pub event_window: usize,
}

impl DaemonConfig {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            token_store: None,
            event_window: DEFAULT_WINDOW,
        }
    }
}

/// A running daemon.
pub struct Daemon {
    pub socket_path: PathBuf,
    /// The token clients must send (also in the token store).
    pub token: String,
    pub hub: EventHub,
    shutdown: watch::Sender<bool>,
    serve: JoinHandle<io::Result<()>>,
    bridge: JoinHandle<()>,
    token_store: Arc<dyn TokenStore>,
}

impl Daemon {
    /// Provision a token, bind the socket and start serving `service`.
    pub async fn start(service: Arc<dyn AppService>, cfg: DaemonConfig) -> io::Result<Self> {
        let token_store: Arc<dyn TokenStore> = cfg.token_store.unwrap_or_else(|| {
            Arc::new(FileTokenStore::new(crate::paths::token_path_for_socket(
                &cfg.socket_path,
            )))
        });
        let listener = bind(&cfg.socket_path).await?;
        let token = provision_token(token_store.as_ref())?;
        let hub = EventHub::new(cfg.event_window);
        let bridge = hub.bridge(service.subscribe());
        let (shutdown, down) = watch::channel(false);
        let state = AppState {
            service,
            hub: hub.clone(),
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64),
            socket_path: Some(cfg.socket_path.display().to_string()),
            shutdown: down.clone(),
        };
        let app = build_router(state, TokenAuth::new(token.clone()));
        let mut stop = down;
        let serve = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop.wait_for(|d| *d).await;
                })
                .await
        });
        tracing::info!(socket = %cfg.socket_path.display(), token = %token_store.describe(), "ratchetd listening");
        Ok(Self {
            socket_path: cfg.socket_path,
            token,
            hub,
            shutdown,
            serve,
            bridge,
            token_store,
        })
    }

    /// Ask the server to stop (SSE streams end, in-flight requests finish).
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Wait until the server has stopped, then remove the socket and the token.
    pub async fn stopped(self) -> io::Result<()> {
        let r = self.serve.await.map_err(io::Error::other)?;
        self.bridge.abort();
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = self.token_store.clear();
        r
    }

    /// [`shutdown`](Self::shutdown) and [`stopped`](Self::stopped).
    pub async fn stop(self) -> io::Result<()> {
        self.shutdown();
        self.stopped().await
    }
}

/// Resolve when the process is asked to stop (Ctrl-C or SIGTERM).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}
