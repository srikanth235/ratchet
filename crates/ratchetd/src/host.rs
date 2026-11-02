//! What the daemon needs from the machine it runs on: keeping it awake while a run is active, and finding the files a
//! packaged Mac app ships next to it. (The Electron app had both; see `docs/CUTOVER.md`.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use ratchet_engine::desktop::PowerSaveBlocker;

pub use crate::power::KEEP_AWAKE_SUPPORTED;
use crate::power::{Held, IoKitAssertions};

/// Keeps the machine awake by running a helper process for as long as the blocker is held (`caffeinate -i -w <pid>` on
/// macOS: no idle sleep, and the helper exits by itself when the daemon does, so a crashed daemon cannot leave the Mac
/// awake). Where the helper does not exist (Linux CI, other hosts) starting is a no-op.
pub struct CommandPowerBlocker {
    program: PathBuf,
    args: Vec<String>,
    children: Mutex<HashMap<u64, Child>>,
    next: Mutex<u64>,
    /// An OS power assertion tried before the helper process (macOS: `IOPMAssertion`).
    assertions: Option<Held>,
}

impl CommandPowerBlocker {
    pub fn new(program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            children: Mutex::new(HashMap::new()),
            next: Mutex::new(1),
            assertions: None,
        }
    }

    /// Hold `assertions` first; the helper process is only the fallback when the OS refuses.
    pub fn with_assertions(mut self, assertions: Held) -> Self {
        self.assertions = Some(assertions);
        self
    }

    /// A blocker that does nothing (hosts without a sleep helper).
    pub fn disabled() -> Self {
        Self::new("", vec![])
    }

    /// What the daemon uses on a Mac: an `IOPMAssertion` (`PreventUserIdleSystemSleep`) held while local agents run, with
    /// `/usr/bin/caffeinate -i -w <this process>` as the fallback when the assertion cannot be created.
    pub fn caffeinate() -> Self {
        Self::new(
            "/usr/bin/caffeinate",
            vec!["-i".into(), "-w".into(), std::process::id().to_string()],
        )
        .with_assertions(Held::new(
            Box::new(IoKitAssertions),
            "Ratchet is running optimizations with agents on this Mac",
        ))
    }

    /// How many holds are in force (OS assertions plus helper processes).
    pub fn held(&self) -> usize {
        self.children.lock().expect("lock").len() + self.assertions.as_ref().map_or(0, Held::count)
    }
}

impl PowerSaveBlocker for CommandPowerBlocker {
    fn start(&self) -> u64 {
        let mut next = self.next.lock().expect("lock");
        let id = *next;
        *next += 1;
        if let Some(a) = &self.assertions
            && a.take(id)
        {
            return id;
        }
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
        if let Some(a) = &self.assertions {
            a.drop_handle(id);
        }
        if let Some(mut child) = self.children.lock().expect("lock").remove(&id) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for CommandPowerBlocker {
    fn drop(&mut self) {
        if let Some(a) = &self.assertions {
            a.release_all();
        }
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
    fn the_os_assertion_is_held_instead_of_a_helper_and_the_helper_is_the_fallback() {
        use crate::power::fake::FakeAssertions;
        let api = FakeAssertions::default();
        let b = CommandPowerBlocker::new("sleep", vec!["60".into()])
            .with_assertions(Held::new(Box::new(api.clone()), "agents"));
        let id = b.start();
        assert_eq!(b.held(), 1);
        assert_eq!(api.log.lock().unwrap().len(), 1, "the assertion, no helper");
        assert!(b.children.lock().unwrap().is_empty());
        b.stop(id);
        assert_eq!(b.held(), 0);
        assert_eq!(api.log.lock().unwrap().len(), 2, "released");

        // the OS refuses: the helper process holds the Mac awake instead
        api.refuse.store(true, std::sync::atomic::Ordering::SeqCst);
        let id = b.start();
        assert_eq!(b.children.lock().unwrap().len(), 1);
        b.stop(id);
        assert_eq!(b.held(), 0);
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
