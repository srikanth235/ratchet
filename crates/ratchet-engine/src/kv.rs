//! The `engine_kv` table of the ledger: engine bookkeeping the ledger schema has no column for (poll cursors, agent
//! identity refs, launch progress, cost watermarks, stop mode). It never holds secret values. Port of `kv.ts`.

use ratchet_ledger::Ledger;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::errors::{EngineError, EngineResult};

#[derive(Clone)]
pub struct KvStore {
    ledger: Ledger,
}

impl KvStore {
    pub fn new(ledger: Ledger) -> Self {
        Self { ledger }
    }

    pub async fn get(&self, key: &str) -> EngineResult<Option<String>> {
        Ok(self.ledger.kv_get(key).await?)
    }

    pub async fn set(&self, key: &str, value: &str) -> EngineResult<()> {
        Ok(self.ledger.kv_set(key, value).await?)
    }

    pub async fn delete(&self, key: &str) -> EngineResult<()> {
        Ok(self.ledger.kv_delete(key).await?)
    }

    pub async fn get_number(&self, key: &str) -> EngineResult<Option<f64>> {
        Ok(self
            .get(key)
            .await?
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|n| n.is_finite()))
    }

    pub async fn get_i64(&self, key: &str) -> EngineResult<Option<i64>> {
        Ok(self.get_number(key).await?.map(|n| n as i64))
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> EngineResult<Option<T>> {
        match self.get(key).await? {
            None => Ok(None),
            Some(v) => serde_json::from_str(&v)
                .map(Some)
                .map_err(|e| EngineError::internal(format!("kv {key}: {e}"))),
        }
    }

    pub async fn set_json<T: Serialize>(&self, key: &str, value: &T) -> EngineResult<()> {
        self.set(key, &serde_json::to_string(value)?).await
    }

    pub async fn has(&self, key: &str) -> EngineResult<bool> {
        Ok(self.get(key).await?.is_some())
    }

    pub async fn entries(&self, prefix: &str) -> EngineResult<Vec<(String, String)>> {
        Ok(self.ledger.kv_prefix(prefix).await?)
    }
}

/// Every kv key the engine writes, in one place.
pub mod k {
    pub fn plateau(opt: &str) -> String {
        format!("opt:{opt}:plateau_evals")
    }
    pub fn deadline(opt: &str) -> String {
        format!("opt:{opt}:deadline_at")
    }
    pub fn warned(opt: &str) -> String {
        format!("opt:{opt}:budget_warned")
    }
    /// `kill` | `graceful`
    pub fn stop_mode(opt: &str) -> String {
        format!("opt:{opt}:stop_mode")
    }
    pub fn stop_reason(opt: &str) -> String {
        format!("opt:{opt}:stop_reason")
    }
    pub fn stop_at(opt: &str) -> String {
        format!("opt:{opt}:stop_at")
    }
    pub fn confirm_best(opt: &str) -> String {
        format!("opt:{opt}:confirm_best")
    }
    pub fn base_sha(opt: &str) -> String {
        format!("opt:{opt}:base_sha")
    }
    /// JSON `Calibration` derived from the baseline (repeats, win threshold, plain-words explanation).
    pub fn calibration(opt: &str) -> String {
        format!("opt:{opt}:calibration")
    }
    pub fn pr(opt: &str) -> String {
        format!("opt:{opt}:pr_url")
    }
    pub fn teardown(opt: &str) -> String {
        format!("opt:{opt}:teardown_report")
    }
    pub fn reconcile(opt: &str) -> String {
        format!("opt:{opt}:cost_reconcile")
    }
    pub fn cursor(agent: &str) -> String {
        format!("agent:{agent}:cursor")
    }
    pub fn identity(agent: &str) -> String {
        format!("agent:{agent}:identity")
    }
    pub fn secrets(agent: &str) -> String {
        format!("agent:{agent}:secret_refs")
    }
    pub fn launch(agent: &str) -> String {
        format!("agent:{agent}:launch_step")
    }
    pub fn revoked(agent: &str) -> String {
        format!("agent:{agent}:revoked")
    }
    pub fn acct(instance: &str) -> String {
        format!("acct:{instance}")
    }
    pub fn sandbox_input(sandbox: &str) -> String {
        format!("sandbox:{sandbox}:input")
    }
    /// Set when the engine created the sandbox itself (and so destroys it after the idle window).
    pub fn sandbox_auto(sandbox: &str) -> String {
        format!("sandbox:{sandbox}:auto")
    }
    pub fn sandbox_idle(sandbox: &str) -> String {
        format!("sandbox:{sandbox}:idle_since")
    }
    pub fn sandbox_receipt(sandbox: &str) -> String {
        format!("sandbox:{sandbox}:receipt")
    }
    pub fn opt_receipt(opt: &str) -> String {
        format!("opt:{opt}:sandbox_receipt")
    }
    pub fn agents_where(opt: &str) -> String {
        format!("opt:{opt}:agents_where")
    }
}
