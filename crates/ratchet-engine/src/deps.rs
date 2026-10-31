//! What the helper modules (cost meter, reaper) need from the engine. Port of `packages/engine/src/deps.ts`.

use std::collections::VecDeque;
use std::fmt::Display;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ratchet_core::ports::{AgentUsage, Clock, CloudCompute, PriceBook};
use ratchet_ledger::Ledger;

use crate::kv::KvStore;
use crate::settings::{ResolvedSettings, TokenPricing};

/// One swallowed non-fatal error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorEntry {
    pub at: i64,
    pub context: String,
    pub message: String,
}

const MAX_ERRORS: usize = 200;

/// Swallowed non-fatal errors (poll failures, price lookups, ...), newest last, capped; and the `on_error` hook.
pub struct ErrorLog {
    clock: Arc<dyn Clock>,
    on_error: crate::settings::OnError,
    entries: Mutex<VecDeque<ErrorEntry>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ErrorLog {
    pub fn new(clock: Arc<dyn Clock>, on_error: crate::settings::OnError) -> Self {
        Self {
            clock,
            on_error,
            entries: Mutex::new(VecDeque::new()),
        }
    }

    pub fn report(&self, err: impl Display, context: &str) {
        let message = err.to_string();
        {
            let mut e = lock(&self.entries);
            e.push_back(ErrorEntry {
                at: self.clock.now(),
                context: context.to_owned(),
                message: message.clone(),
            });
            if e.len() > MAX_ERRORS {
                e.pop_front();
            }
        }
        // never let a logging hook break the loop
        let hook = self.on_error.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(&message, context)));
    }

    pub fn entries(&self) -> Vec<ErrorEntry> {
        lock(&self.entries).iter().cloned().collect()
    }
}

#[derive(Clone)]
pub struct Deps {
    pub ledger: Ledger,
    pub kv: KvStore,
    pub clock: Arc<dyn Clock>,
    pub compute: Arc<dyn CloudCompute>,
    pub prices: Arc<dyn PriceBook>,
    pub settings: Arc<ResolvedSettings>,
    pub errors: Arc<ErrorLog>,
}

impl Deps {
    pub fn report_error(&self, err: impl Display, context: &str) {
        self.errors.report(err, context);
    }
}

pub fn token_cost_usd(usage: &AgentUsage, pricing: &TokenPricing) -> f64 {
    if let Some(c) = usage.cost_usd.filter(|c| c.is_finite()) {
        return c.max(0.0);
    }
    let finite = |n: u64| n as f64;
    (finite(usage.input_tokens).max(0.0) * pricing.input_usd_per_mtok
        + finite(usage.output_tokens).max(0.0) * pricing.output_usd_per_mtok)
        / 1_000_000.0
}
