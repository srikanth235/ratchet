//! Keep the machine awake while a run is active. Port of `packages/desktop/src/main/power.ts`; the app (or the daemon's
//! host) supplies the actual blocker (IOKit assertion, `caffeinate`, ...).

use std::sync::Mutex;

/// The OS-level "do not sleep" assertion.
pub trait PowerSaveBlocker: Send + Sync {
    /// Start preventing app suspension; returns a handle for [`stop`](Self::stop).
    fn start(&self) -> u64;
    fn stop(&self, id: u64);
}

/// `run-active: true` starts a blocker, `false` stops it. Idempotent, so a repeated `true` never leaks a second one.
pub struct PowerSaveHook<B: PowerSaveBlocker> {
    blocker: B,
    held: Mutex<Option<u64>>,
}

impl<B: PowerSaveBlocker> PowerSaveHook<B> {
    pub fn new(blocker: B) -> Self {
        Self {
            blocker,
            held: Mutex::new(None),
        }
    }

    /// Feed every `run-active` event (`active`) here.
    pub fn on_run_active(&self, active: bool) {
        if active {
            let mut h = self.held.lock().expect("lock");
            if h.is_none() {
                *h = Some(self.blocker.start());
            }
        } else {
            self.release();
        }
    }

    pub fn held(&self) -> bool {
        self.held.lock().expect("lock").is_some()
    }

    pub fn release(&self) {
        if let Some(id) = self.held.lock().expect("lock").take() {
            self.blocker.stop(id);
        }
    }
}

/// A blocker that does nothing (the daemon without a host app).
#[derive(Default)]
pub struct NoPowerBlocker;

impl PowerSaveBlocker for NoPowerBlocker {
    fn start(&self) -> u64 {
        0
    }
    fn stop(&self, _id: u64) {}
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    struct Recording {
        calls: Arc<Mutex<Vec<String>>>,
        next: AtomicU64,
    }

    impl PowerSaveBlocker for Recording {
        fn start(&self) -> u64 {
            self.calls.lock().unwrap().push("start".into());
            self.next.fetch_add(1, Ordering::SeqCst)
        }
        fn stop(&self, id: u64) {
            self.calls.lock().unwrap().push(format!("stop:{id}"));
        }
    }

    /// machine.test.ts › powerSaveBlocker hook › starts prevent-app-suspension on run-active and stops it when the last
    /// run ends
    #[test]
    fn starts_on_run_active_and_stops_when_the_last_run_ends() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let hook = PowerSaveHook::new(Recording {
            calls: calls.clone(),
            next: AtomicU64::new(7),
        });
        let seen = || calls.lock().unwrap().clone();
        assert!(seen().is_empty());
        hook.on_run_active(true);
        hook.on_run_active(true); // idempotent: no second blocker
        assert_eq!(seen(), ["start"]);
        assert!(hook.held());
        hook.on_run_active(false);
        hook.on_run_active(false);
        assert_eq!(seen(), ["start", "stop:7"]);
        assert!(!hook.held());
        hook.on_run_active(true);
        hook.release(); // on quit
        assert_eq!(seen().last().map(String::as_str), Some("stop:8"));
    }
}
