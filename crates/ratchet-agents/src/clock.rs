//! Clocks: the wall clock, an accelerated one for simulation, and a manual one for tests.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub use crate::domain::Clock;

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Sleeps for `duration`; returns early (never errors) when `cancel` fires. A zero duration or an already-cancelled token returns at once.
async fn sleep_for(duration: Duration, cancel: &CancellationToken) {
    if cancel.is_cancelled() || duration.is_zero() {
        return;
    }
    tokio::select! {
        _ = tokio::time::sleep(duration) => {}
        _ = cancel.cancelled() => {}
    }
}

/// The wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        epoch_ms()
    }
    async fn sleep(&self, duration: Duration, cancel: &CancellationToken) {
        sleep_for(duration, cancel).await
    }
}

/// Wall clock running `speed` times faster: `sleep(d)` takes `d / speed` real time and `now()` advances `speed` virtual ms per real
/// ms. Lets simulation compress hours into seconds while keeping deadlines and real waits (verdicts arriving from the engine)
/// consistent with each other.
#[derive(Debug)]
pub struct AcceleratedClock {
    speed: f64,
    real_start: Instant,
    virtual_start: i64,
}

impl AcceleratedClock {
    /// # Panics
    /// When `speed` is not a positive finite number (the TS version throws a `RangeError`).
    pub fn new(speed: f64) -> Self {
        Self::starting_at(speed, epoch_ms())
    }

    pub fn starting_at(speed: f64, virtual_start: i64) -> Self {
        assert!(speed > 0.0 && speed.is_finite(), "speed must be > 0");
        Self {
            speed,
            real_start: Instant::now(),
            virtual_start,
        }
    }

    pub fn try_new(speed: f64) -> Result<Self, String> {
        if speed > 0.0 && speed.is_finite() {
            Ok(Self::new(speed))
        } else {
            Err("speed must be > 0".to_string())
        }
    }
}

#[async_trait]
impl Clock for AcceleratedClock {
    fn now(&self) -> i64 {
        let real_ms = self.real_start.elapsed().as_secs_f64() * 1000.0;
        self.virtual_start + (real_ms * self.speed) as i64
    }
    async fn sleep(&self, duration: Duration, cancel: &CancellationToken) {
        sleep_for(duration.div_f64(self.speed), cancel).await
    }
}

struct Sleeper {
    at: i64,
    wake: oneshot::Sender<()>,
}

/// Deterministic clock for tests: time only moves via `advance`.
#[derive(Default)]
pub struct ManualClock {
    state: Mutex<ManualState>,
}

#[derive(Default)]
struct ManualState {
    time: i64,
    sleepers: Vec<Sleeper>,
}

impl ManualClock {
    pub fn new(time: i64) -> Self {
        Self {
            state: Mutex::new(ManualState {
                time,
                sleepers: Vec::new(),
            }),
        }
    }

    /// Move time forward, waking every sleeper that comes due (in time order).
    pub async fn advance(&self, ms: i64) {
        let target = self.state.lock().unwrap().time + ms;
        loop {
            let due = {
                let mut st = self.state.lock().unwrap();
                let next = st
                    .sleepers
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.at <= target)
                    .min_by_key(|(_, s)| s.at)
                    .map(|(i, _)| i);
                next.map(|i| {
                    let s = st.sleepers.remove(i);
                    st.time = st.time.max(s.at);
                    s
                })
            };
            match due {
                Some(s) => {
                    let _ = s.wake.send(());
                    tokio::task::yield_now().await;
                }
                None => break,
            }
        }
        self.state.lock().unwrap().time = target;
    }

    pub fn pending_sleepers(&self) -> usize {
        self.state.lock().unwrap().sleepers.len()
    }
}

#[async_trait]
impl Clock for ManualClock {
    fn now(&self) -> i64 {
        self.state.lock().unwrap().time
    }
    async fn sleep(&self, duration: Duration, cancel: &CancellationToken) {
        if cancel.is_cancelled() || duration.is_zero() {
            return;
        }
        let rx = {
            let mut st = self.state.lock().unwrap();
            let (tx, rx) = oneshot::channel();
            let at = st.time + duration.as_millis() as i64;
            st.sleepers.push(Sleeper { at, wake: tx });
            rx
        };
        tokio::select! {
            _ = rx => {}
            _ = cancel.cancelled() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn accelerated_clock_compresses_sleeps_and_scales_now() {
        let clock = AcceleratedClock::new(1000.0);
        let t0 = clock.now();
        let started = Instant::now();
        clock
            .sleep(Duration::from_secs(5), &CancellationToken::new())
            .await; // 5 ms real
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(clock.now() - t0 >= 4_000);
    }

    #[test]
    fn accelerated_clock_rejects_non_positive_speed() {
        assert!(AcceleratedClock::try_new(0.0).is_err());
        assert!(AcceleratedClock::try_new(-1.0).is_err());
        assert!(AcceleratedClock::try_new(f64::NAN).is_err());
    }

    #[tokio::test]
    async fn sleep_resolves_on_cancel() {
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            c2.cancel();
        });
        let started = Instant::now();
        SystemClock.sleep(Duration::from_secs(60), &cancel).await;
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn manual_clock_wakes_sleepers_in_time_order() {
        let clock = Arc::new(ManualClock::new(1000));
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for (name, ms) in [("late", 300u64), ("early", 100)] {
            let (c, l) = (clock.clone(), log.clone());
            tasks.push(tokio::spawn(async move {
                c.sleep(Duration::from_millis(ms), &CancellationToken::new())
                    .await;
                l.lock().unwrap().push((name, c.now()));
            }));
        }
        while clock.pending_sleepers() < 2 {
            tokio::task::yield_now().await;
        }
        clock.advance(99).await;
        assert!(log.lock().unwrap().is_empty());
        clock.advance(1000).await;
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(*log.lock().unwrap(), vec![("early", 1100), ("late", 1300)]);
        assert_eq!(clock.now(), 2099);
    }
}
