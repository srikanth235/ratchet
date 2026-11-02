//! Starting the daemon when a command finds it down.
//!
//! On a Mac the daemon is a LaunchAgent that the Ratchet app registers. When it is not running, a command first asks
//! launchd to start it (`launchctl kickstart`), then opens the app in the background (`open -g -a Ratchet`), which
//! registers and starts it; if the socket still does not answer, the command says plainly what to do. Only the standard
//! location is handled: a command that names its own socket, token or `RATCHET_HOME` talks to a daemon someone started
//! on purpose, and is never "helped".

use std::path::Path;
use std::time::Duration;

use super::report_error;
use crate::client::Connection;
use crate::io::Io;

/// The LaunchAgent's label (`ratchetd launchagent print`).
pub const AGENT_LABEL: &str = "com.ratchet.ratchetd";

/// How long to wait for the socket after each way of starting the daemon.
const AFTER_KICKSTART: Duration = Duration::from_secs(4);
const AFTER_OPEN: Duration = Duration::from_secs(25);

/// What the decision depends on, apart from the connection options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Environment {
    pub macos: bool,
    /// `RATCHET_HOME` names a directory: a daemon of the user's own.
    pub ratchet_home_set: bool,
    /// `RATCHET_NO_AUTOSTART` is set.
    pub disabled: bool,
}

impl Environment {
    pub fn current() -> Self {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        Self {
            macos: cfg!(target_os = "macos"),
            ratchet_home_set: set("RATCHET_HOME"),
            disabled: set("RATCHET_NO_AUTOSTART"),
        }
    }
}

/// True when a down daemon should be started for this command line.
pub fn wanted(conn: &Connection, env: &Environment) -> bool {
    env.macos
        && !env.disabled
        && !env.ratchet_home_set
        && conn.socket.is_none()
        && conn.token.is_none()
        && conn.token_file.is_none()
}

/// What to tell the user when the daemon could not be started.
pub fn not_running_message() -> String {
    "ratchetd is not running and could not be started. Open the Ratchet app once: it starts the background service, \
     and macOS may ask you to allow it in System Settings > General > Login Items. Then run this command again."
        .to_owned()
}

/// True when something accepts connections at `socket`.
pub async fn is_up(socket: &Path) -> bool {
    tokio::net::UnixStream::connect(socket).await.is_ok()
}

async fn wait_for(socket: &Path, up_to: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + up_to;
    loop {
        if is_up(socket).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The argument list of `launchctl kickstart` for the user with this uid.
pub fn kickstart_args(uid: u32) -> Vec<String> {
    vec!["kickstart".to_owned(), format!("gui/{uid}/{AGENT_LABEL}")]
}

async fn run_quiet(program: &str, args: &[String]) -> bool {
    tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

/// Starts the daemon if it is down. Returns the exit code to stop with, or `None` when the daemon answers.
pub async fn ensure_running(socket: &Path, json: bool, io: &dyn Io) -> Option<i32> {
    if is_up(socket).await {
        return None;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    // 1. The agent is registered but stopped (a crash, a logout): launchd starts it.
    run_quiet("/bin/launchctl", &kickstart_args(uid)).await;
    if wait_for(socket, AFTER_KICKSTART).await {
        return None;
    }
    // 2. Not registered yet: the app registers and starts it.
    io.err("ratchet: ratchetd is not running; opening the Ratchet app in the background to start it...\n");
    let opened = run_quiet(
        "/usr/bin/open",
        &["-g".to_owned(), "-a".to_owned(), "Ratchet".to_owned()],
    )
    .await;
    if opened && wait_for(socket, AFTER_OPEN).await {
        return None;
    }
    let err = crate::client::ClientError::Connect {
        socket: socket.display().to_string(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, not_running_message()),
    };
    Some(report_error(io, json, &err))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        Connection::default()
    }

    fn mac() -> Environment {
        Environment {
            macos: true,
            ratchet_home_set: false,
            disabled: false,
        }
    }

    #[test]
    fn the_standard_location_on_a_mac_is_started() {
        assert!(wanted(&conn(), &mac()));
    }

    #[test]
    fn a_daemon_someone_started_on_purpose_is_left_alone() {
        let mut c = conn();
        c.socket = Some("/tmp/x.sock".into());
        assert!(!wanted(&c, &mac()));
        let mut c = conn();
        c.token = Some("t".into());
        assert!(!wanted(&c, &mac()));
        let mut c = conn();
        c.token_file = Some("/tmp/t".into());
        assert!(!wanted(&c, &mac()));
        let env = Environment {
            ratchet_home_set: true,
            ..mac()
        };
        assert!(!wanted(&conn(), &env));
    }

    #[test]
    fn other_systems_and_the_off_switch_never_start_anything() {
        let linux = Environment {
            macos: false,
            ..mac()
        };
        assert!(!wanted(&conn(), &linux));
        let off = Environment {
            disabled: true,
            ..mac()
        };
        assert!(!wanted(&conn(), &off));
    }

    #[test]
    fn the_kickstart_targets_the_users_agent() {
        assert_eq!(
            kickstart_args(501),
            vec!["kickstart", "gui/501/com.ratchet.ratchetd"]
        );
    }

    #[test]
    fn the_message_says_what_to_do() {
        let m = not_running_message();
        assert!(m.contains("Open the Ratchet app"));
        assert!(m.contains("Login Items"));
    }
}
