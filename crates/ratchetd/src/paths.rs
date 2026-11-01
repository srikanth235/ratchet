//! Where the daemon keeps its files.
//!
//! macOS: `~/Library/Application Support/Ratchet/` (`ratchetd.sock`, `ratchetd.token`, `ratchet.db`).
//! Linux (CI, bench hosts): `$XDG_RUNTIME_DIR/ratchet/`, else `<tmp>/ratchet-<user>/`.
//! `RATCHET_HOME` overrides the directory on both.

use std::path::{Path, PathBuf};

pub const SOCKET_FILE: &str = "ratchetd.sock";
pub const TOKEN_FILE: &str = "ratchetd.token";
pub const LEDGER_FILE: &str = "ratchet.db";
pub const LOG_FILE: &str = "ratchetd.log";

/// The directory holding the socket, the token and (by default) the ledger.
pub fn support_dir() -> PathBuf {
    support_dir_from(
        std::env::var_os("RATCHET_HOME").map(PathBuf::from),
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var("USER").ok(),
        cfg!(target_os = "macos"),
    )
}

/// [`support_dir`] with the environment passed in (tests).
pub fn support_dir_from(
    ratchet_home: Option<PathBuf>,
    xdg_runtime_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    user: Option<String>,
    macos: bool,
) -> PathBuf {
    if let Some(h) = ratchet_home.filter(|p| !p.as_os_str().is_empty()) {
        return h;
    }
    if macos && let Some(home) = home {
        return home.join("Library/Application Support/Ratchet");
    }
    if let Some(x) = xdg_runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
        return x.join("ratchet");
    }
    std::env::temp_dir().join(format!("ratchet-{}", user.as_deref().unwrap_or("user")))
}

pub fn socket_path() -> PathBuf {
    support_dir().join(SOCKET_FILE)
}

pub fn token_path() -> PathBuf {
    support_dir().join(TOKEN_FILE)
}

pub fn ledger_path() -> PathBuf {
    support_dir().join(LEDGER_FILE)
}

/// The token file that sits next to a socket (`<dir>/ratchetd.token`).
pub fn token_path_for_socket(socket: &Path) -> PathBuf {
    socket
        .parent()
        .map_or_else(|| PathBuf::from(TOKEN_FILE), |d| d.join(TOKEN_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_uses_application_support() {
        let d = support_dir_from(
            None,
            Some("/run/user/1".into()),
            Some("/Users/me".into()),
            None,
            true,
        );
        assert_eq!(
            d,
            Path::new("/Users/me/Library/Application Support/Ratchet")
        );
    }

    #[test]
    fn linux_falls_back_to_the_runtime_dir_then_tmp() {
        let d = support_dir_from(
            None,
            Some("/run/user/1000".into()),
            Some("/home/me".into()),
            None,
            false,
        );
        assert_eq!(d, Path::new("/run/user/1000/ratchet"));
        let t = support_dir_from(
            None,
            None,
            Some("/home/me".into()),
            Some("ci".into()),
            false,
        );
        assert_eq!(t, std::env::temp_dir().join("ratchet-ci"));
    }

    #[test]
    fn ratchet_home_wins() {
        let d = support_dir_from(
            Some("/x/y".into()),
            Some("/run".into()),
            Some("/h".into()),
            None,
            true,
        );
        assert_eq!(d, Path::new("/x/y"));
        assert_eq!(
            token_path_for_socket(Path::new("/x/y/ratchetd.sock")),
            Path::new("/x/y/ratchetd.token")
        );
    }
}
