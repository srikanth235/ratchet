//! Port of `packages/runtime/src/clock.ts`.

use std::sync::Arc;

use ratchet_core::ports::Clock;
use ratchet_core::ports::mock::SystemClock;

/// The wall clock (`systemClock`): `now` is epoch ms, `sleep` waits for real and returns early when cancelled.
pub fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}
