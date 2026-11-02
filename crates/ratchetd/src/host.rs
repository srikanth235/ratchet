//! What the daemon needs from the machine it runs on: keeping it awake while a run is active, and finding the files a
//! packaged Mac app ships next to it. (The Electron app had both; see `docs/CUTOVER.md`.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use ratchet_engine::desktop::{PowerSaveBlocker, PowerSaveHook};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::api::ApiEvent;

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

/// Drive `hook` from the daemon's events until the stream closes.
///
/// `RunActive` events switch the blocker. A slow loop can fall behind the broadcast channel and get
/// `RecvError::Lagged`: that only means events were dropped (possibly a `RunActive`), so it asks `run_active` what is
/// true now and carries on. Only `Closed` (the service is gone) ends the loop and releases the blocker. The state is
/// also read once up front, since a run may have started before the subscription.
pub async fn keep_awake<B: PowerSaveBlocker>(
    hook: &PowerSaveHook<B>,
    mut events: broadcast::Receiver<ApiEvent>,
    run_active: impl Fn() -> bool,
) {
    hook.on_run_active(run_active());
    loop {
        match events.recv().await {
            Ok(ApiEvent::RunActive { active, .. }) => hook.on_run_active(active),
            Ok(_) => {}
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "keep-awake fell behind the event stream; resyncing");
                hook.on_run_active(run_active());
            }
            Err(RecvError::Closed) => break,
        }
    }
    hook.release();
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

    fn run_active_event(active: bool) -> ApiEvent {
        ApiEvent::RunActive {
            active,
            optimization_ids: vec![],
        }
    }

    /// A blocker that records start/stop, so a test can tell "released" from "never held".
    #[derive(Default, Clone)]
    struct Recording(std::sync::Arc<Mutex<Vec<&'static str>>>);

    impl PowerSaveBlocker for Recording {
        fn start(&self) -> u64 {
            self.0.lock().unwrap().push("start");
            1
        }
        fn stop(&self, _id: u64) {
            self.0.lock().unwrap().push("stop");
        }
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn keep_awake_survives_lagged_and_resyncs_from_the_engine() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (tx, rx) = broadcast::channel::<ApiEvent>(2);
        let rec = Recording::default();
        let hook = Arc::new(PowerSaveHook::new(rec.clone()));
        let active = Arc::new(AtomicBool::new(false));
        let task = {
            let (hook, active) = (hook.clone(), active.clone());
            tokio::spawn(async move {
                keep_awake(&*hook, rx, move || active.load(Ordering::SeqCst)).await;
            })
        };
        settle().await;
        assert!(!hook.held());

        // The receiver is overrun while the loop has not run: 5 events into a channel of 2. The run-active (true)
        // is among the dropped ones, but the engine says a run is active.
        active.store(true, Ordering::SeqCst);
        for e in [
            run_active_event(true),
            ApiEvent::Notice {
                optimization_id: "o".into(),
                level: ratchet_core::ports::NoticeLevel::Info,
                text: "a".into(),
            },
            ApiEvent::Notice {
                optimization_id: "o".into(),
                level: ratchet_core::ports::NoticeLevel::Info,
                text: "b".into(),
            },
            ApiEvent::Notice {
                optimization_id: "o".into(),
                level: ratchet_core::ports::NoticeLevel::Info,
                text: "c".into(),
            },
            ApiEvent::Notice {
                optimization_id: "o".into(),
                level: ratchet_core::ports::NoticeLevel::Info,
                text: "d".into(),
            },
        ] {
            tx.send(e).unwrap();
        }
        settle().await;
        assert!(!task.is_finished(), "Lagged must not end the loop");
        assert!(hook.held(), "resynced to the engine's active state");

        // and the loop still follows later events
        active.store(false, Ordering::SeqCst);
        tx.send(run_active_event(false)).unwrap();
        settle().await;
        assert!(!hook.held());
        tx.send(run_active_event(true)).unwrap();
        settle().await;
        assert!(hook.held(), "events after the lag are still handled");

        // only the channel closing ends it, and it releases the blocker
        drop(tx);
        task.await.unwrap();
        assert!(!hook.held());
        assert_eq!(rec.0.lock().unwrap().last(), Some(&"stop"));
    }

    #[tokio::test]
    async fn keep_awake_resyncs_to_released_when_the_lagged_run_ended() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (tx, rx) = broadcast::channel::<ApiEvent>(1);
        let hook = Arc::new(PowerSaveHook::new(Recording::default()));
        let active = Arc::new(AtomicBool::new(true)); // a run was already active at startup
        let task = {
            let (hook, active) = (hook.clone(), active.clone());
            tokio::spawn(async move {
                keep_awake(&*hook, rx, move || active.load(Ordering::SeqCst)).await;
            })
        };
        settle().await;
        assert!(hook.held(), "read once at startup");

        // the run ends, but the `false` event is lost to the lag
        active.store(false, Ordering::SeqCst);
        for _ in 0..3 {
            tx.send(run_active_event(true)).unwrap();
        }
        // (the one event the channel still holds is not a run-active, so only the resync can release)
        tx.send(ApiEvent::Notice {
            optimization_id: "o".into(),
            level: ratchet_core::ports::NoticeLevel::Info,
            text: "x".into(),
        })
        .unwrap();
        settle().await;
        assert!(!hook.held(), "the lag resynced to the engine: not active");
        drop(tx);
        task.await.unwrap();
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
