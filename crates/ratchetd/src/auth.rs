//! Authentication: a per-boot bearer token, on top of the 0600 socket.
//!
//! The daemon generates a fresh token at every start and hands it to a [`TokenStore`]; clients read it from the
//! same store and send `Authorization: Bearer <token>`. Every route needs it.
//!
//! The store is a trait so the Mac app and the daemon can share the macOS Keychain.
//! TODO(keychain): add `KeychainTokenStore` (Security.framework generic password `ratchet` / `ratchetd-token`) in
//! the signed daemon; until then [`FileTokenStore`] keeps the token in a 0600 file next to the socket, which has
//! the same trust boundary (only the user can read it).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::api::{ApiError, ErrorCode};

/// Where the per-boot token lives.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> io::Result<Option<String>>;
    fn save(&self, token: &str) -> io::Result<()>;
    fn clear(&self) -> io::Result<()>;
    /// For logs and error messages ("file /…/ratchetd.token").
    fn describe(&self) -> String;
}

/// The token in a file with mode 0600, written atomically.
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> io::Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => Ok(Some(s.trim().to_owned()).filter(|t| !t.is_empty())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn save(&self, token: &str) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = self.path.parent() {
            create_private_dir(dir)?;
        }
        let tmp = self.path.with_extension("token.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)
    }

    fn clear(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn describe(&self) -> String {
        format!("file {}", self.path.display())
    }
}

/// Create `dir` (and parents) readable by the user only.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // `mode` is filtered by the umask; make it exact for the leaf.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// 256 random bits as 64 hex characters.
pub fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// A new token for this boot, saved to the store.
pub fn provision_token(store: &dyn TokenStore) -> io::Result<String> {
    let t = generate_token();
    store.save(&t)?;
    Ok(t)
}

/// Compare without leaking where the first difference is.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token every request must carry.
#[derive(Clone)]
pub struct TokenAuth {
    token: Arc<String>,
}

impl TokenAuth {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: Arc::new(token.into()),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// True when `header` is `Bearer <the token>`.
    pub fn accepts(&self, header: Option<&HeaderValue>) -> bool {
        let Some(v) = header.and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let Some((scheme, value)) = v.split_once(' ') else {
            return false;
        };
        scheme.eq_ignore_ascii_case("bearer")
            && constant_time_eq(value.trim().as_bytes(), self.token.as_bytes())
    }
}

fn unauthorized(message: &str) -> Response {
    let body = ApiError {
        code: ErrorCode::Unauthorized,
        message: message.to_owned(),
        fields: vec![],
    };
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        axum::Json(body),
    )
        .into_response()
}

/// Middleware: 401 unless the request carries the bearer token.
pub async fn require_token(State(auth): State<TokenAuth>, req: Request, next: Next) -> Response {
    match req.headers().get(header::AUTHORIZATION) {
        None => unauthorized("missing bearer token (read it from the ratchetd token file)"),
        Some(h) if auth.accepts(Some(h)) => next.run(req).await,
        Some(_) => unauthorized("wrong bearer token"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn tokens_are_long_and_different() {
        let a = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, generate_token());
    }

    #[test]
    fn the_file_store_is_private_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(dir.path().join("sub/ratchetd.token"));
        assert_eq!(store.load().unwrap(), None);
        let t = provision_token(&store).unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some(t.as_str()));
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let dmode = std::fs::metadata(store.path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dmode, 0o700);
        // a second boot replaces it
        let t2 = provision_token(&store).unwrap();
        assert_ne!(t, t2);
        assert_eq!(store.load().unwrap().as_deref(), Some(t2.as_str()));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
    }

    #[test]
    fn bearer_parsing() {
        let a = TokenAuth::new("s3cret");
        let h = |s: &str| HeaderValue::from_str(s).unwrap();
        assert!(a.accepts(Some(&h("Bearer s3cret"))));
        assert!(a.accepts(Some(&h("bearer s3cret"))));
        assert!(!a.accepts(Some(&h("Bearer s3cre"))));
        assert!(!a.accepts(Some(&h("Bearer s3cretx"))));
        assert!(!a.accepts(Some(&h("Basic s3cret"))));
        assert!(!a.accepts(Some(&h("s3cret"))));
        assert!(!a.accepts(None));
    }
}
