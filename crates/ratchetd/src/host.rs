//! What the daemon needs from the machine it runs on: keeping it awake while a run is active, and finding the files a
//! packaged Mac app ships next to it. (The Electron app had both; see `docs/CUTOVER.md`.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use ratchet_engine::desktop::PowerSaveBlocker;

/// Keeps the machine awake by running a helper process for as long as the blocker is held (`caffeinate -i -w <pid>` on
/// macOS: no idle sleep, and the helper exits by itself when the daemon does, so a crashed daemon cannot leave the Mac
/// awake). Where the helper does not exist (Linux CI, other hosts) starting is a no-op.
pub struct CommandPowerBlocker {
    program: PathBuf,
    args: Vec<String>,
    children: Mutex<HashMap<u64, Child>>,
    next: Mutex<u64>,
}

impl CommandPowerBlocker {
    pub fn new(program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            children: Mutex::new(HashMap::new()),
            next: Mutex::new(1),
        }
    }

    /// A blocker that does nothing (hosts without a sleep helper).
    pub fn disabled() -> Self {
        Self::new("", vec![])
    }

    /// `/usr/bin/caffeinate -i -w <this process>`.
    pub fn caffeinate() -> Self {
        Self::new(
            "/usr/bin/caffeinate",
            vec!["-i".into(), "-w".into(), std::process::id().to_string()],
        )
    }

    /// Is a helper process currently held?
    pub fn held(&self) -> usize {
        self.children.lock().expect("lock").len()
    }
}

impl PowerSaveBlocker for CommandPowerBlocker {
    fn start(&self) -> u64 {
        let mut next = self.next.lock().expect("lock");
        let id = *next;
        *next += 1;
        if self.program.as_os_str().is_empty() {
            return id; // disabled: this host has no such helper
        }
        match Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.children.lock().expect("lock").insert(id, child);
            }
            Err(err) => {
                tracing::warn!(program = %self.program.display(), %err, "cannot keep the machine awake")
            }
        }
        id
    }

    fn stop(&self, id: u64) {
        if let Some(mut child) = self.children.lock().expect("lock").remove(&id) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for CommandPowerBlocker {
    fn drop(&mut self) {
        for (_, mut child) in self.children.lock().expect("lock").drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The `Resources` directory of the app bundle `exe` lives in (`<App>/Contents/Helpers/ratchetd`), when it ships the
/// OpenTofu module (`Resources/infra/aws`). The LaunchAgent plist cannot carry the bundle's absolute path, so the
/// daemon finds it from where it is.
pub fn bundled_resources(exe: &Path) -> Option<PathBuf> {
    let helpers = exe.parent()?;
    if helpers.file_name()? != "Helpers" {
        return None;
    }
    let contents = helpers.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let resources = contents.join("Resources");
    resources
        .join("infra")
        .join("aws")
        .is_dir()
        .then_some(resources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_a_process_while_active_and_ends_it_on_stop() {
        let b = CommandPowerBlocker::new("sleep", vec!["60".into()]);
        let id = b.start();
        assert_eq!(b.held(), 1);
        b.stop(id);
        assert_eq!(b.held(), 0);
        b.stop(id); // idempotent
    }

    #[test]
    fn a_missing_helper_is_not_an_error() {
        let b = CommandPowerBlocker::new("/nonexistent/caffeinate", vec![]);
        let id = b.start();
        assert_eq!(b.held(), 0);
        b.stop(id);
    }

    #[test]
    fn the_hook_drives_the_blocker_from_run_active_events() {
        use ratchet_engine::desktop::PowerSaveHook;
        let hook = PowerSaveHook::new(CommandPowerBlocker::new("sleep", vec!["60".into()]));
        hook.on_run_active(true);
        hook.on_run_active(true);
        assert!(hook.held());
        hook.on_run_active(false);
        assert!(!hook.held());
    }

    #[test]
    fn finds_the_resources_of_an_app_bundle_that_ships_the_module() {
        let app = tempfile::tempdir().unwrap();
        let exe = app.path().join("Contents/Helpers/ratchetd");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(bundled_resources(&exe), None, "no module shipped");
        std::fs::create_dir_all(app.path().join("Contents/Resources/infra/aws")).unwrap();
        assert_eq!(
            bundled_resources(&exe),
            Some(app.path().join("Contents/Resources"))
        );
        assert_eq!(
            bundled_resources(Path::new("/usr/local/bin/ratchetd")),
            None
        );
    }
}
