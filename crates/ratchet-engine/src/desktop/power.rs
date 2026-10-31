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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[derive(Default)]
    struct Counting {
        started: AtomicU64,
        stopped: AtomicU64,
    }

    impl PowerSaveBlocker for Arc<Counting> {
        fn start(&self) -> u64 {
            self.started.fetch_add(1, Ordering::SeqCst) + 1
        }
        fn stop(&self, _id: u64) {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn starts_once_stops_once() {
        let c = Arc::new(Counting::default());
        let hook = PowerSaveHook::new(c.clone());
        hook.on_run_active(true);
        hook.on_run_active(true);
        assert!(hook.held());
        assert_eq!(c.started.load(Ordering::SeqCst), 1);
        hook.on_run_active(false);
        hook.on_run_active(false);
        assert!(!hook.held());
        assert_eq!(c.stopped.load(Ordering::SeqCst), 1);
        hook.on_run_active(true);
        hook.release();
        assert_eq!(c.stopped.load(Ordering::SeqCst), 2);
    }
}
