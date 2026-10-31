//! Clocks beyond core's: an accelerated one for simulation, and a manual one for tests whose sleepers wake only when time is
//! advanced. The wall clock (`SystemClock`) and the clock that jumps on `sleep` (`ManualClock`) are
//! `ratchet_core::ports::mock`'s.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ratchet_core::ports::{CancellationToken, Clock, PortError, PortResult};
use tokio::sync::oneshot;

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Sleeps for `duration`; `Err(Cancelled)` when `cancel` fires first. A zero duration returns at once.
async fn sleep_for(duration: Duration, cancel: Option<&CancellationToken>) -> PortResult<()> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err(PortError::Cancelled);
    }
    if duration.is_zero() {
        return Ok(());
    }
    let sleep = tokio::time::sleep(duration);
    match cancel {
        None => {
            sleep.await;
            Ok(())
        }
        Some(c) => tokio::select! {
            () = sleep => Ok(()),
            () = c.cancelled() => Err(PortError::Cancelled),
        },
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
    async fn sleep(&self, ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()> {
        sleep_for(Duration::from_millis(ms).div_f64(self.speed), cancel).await
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
    async fn sleep(&self, ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()> {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(PortError::Cancelled);
        }
        if ms == 0 {
            return Ok(());
        }
        let rx = {
            let mut st = self.state.lock().unwrap();
            let (tx, rx) = oneshot::channel();
            let at = st.time + ms as i64;
            st.sleepers.push(Sleeper { at, wake: tx });
            rx
        };
        match cancel {
            None => {
                let _ = rx.await;
                Ok(())
            }
            Some(c) => tokio::select! {
                _ = rx => Ok(()),
                () = c.cancelled() => Err(PortError::Cancelled),
            },
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
            .sleep(5_000, Some(&CancellationToken::new()))
            .await
            .unwrap(); // 5 ms real
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
        let res = AcceleratedClock::new(1.0)
            .sleep(60_000, Some(&cancel))
            .await;
        assert_eq!(res, Err(PortError::Cancelled));
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
                c.sleep(ms, Some(&CancellationToken::new())).await.unwrap();
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
