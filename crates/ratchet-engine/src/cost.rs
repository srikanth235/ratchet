//! Live cost estimate. Port of `packages/engine/src/cost.ts`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use ratchet_core::enums::{InstanceRole, SpendCategory};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::{InstanceInfo, InstanceState};
use ratchet_ledger::RecordSpendInput;

use crate::deps::Deps;
use crate::errors::EngineResult;
use crate::kv::k;

const HOUR_MS: f64 = 3_600_000.0;

fn category(role: InstanceRole) -> SpendCategory {
    match role {
        InstanceRole::AgentRunner => SpendCategory::AgentRunner,
        InstanceRole::Experiment => SpendCategory::Experiment,
        InstanceRole::Bench => SpendCategory::BenchRunner,
    }
}

/// Tagged instances x on-demand price, accrued since a per-instance watermark (kept in the kv table, so it survives
/// restarts and never double counts). Called once per tick; the tick interval is therefore the metering interval.
pub struct CostMeter {
    d: Deps,
    price_cache: Mutex<BTreeMap<String, f64>>,
}

impl CostMeter {
    pub fn new(d: Deps) -> Self {
        Self {
            d,
            price_cache: Mutex::new(BTreeMap::new()),
        }
    }

    async fn hourly(&self, instance_type: &str) -> f64 {
        match self.d.prices.hourly_usd(instance_type).await {
            Ok(p) if p.is_finite() && p >= 0.0 => {
                self.price_cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(instance_type.to_owned(), p);
                p
            }
            other => {
                let msg = match other {
                    Ok(p) => format!("bad price {p} for {instance_type}"),
                    Err(e) => e.to_string(),
                };
                self.d
                    .report_error(msg, &format!("price lookup {instance_type}"));
                self.price_cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(instance_type)
                    .copied()
                    .unwrap_or(self.d.settings.fallback_hourly_usd)
            }
        }
    }

    /// Records spend for everything accrued up to now. Returns the USD added.
    pub async fn meter(
        &self,
        optimization_id: &str,
        instances: Option<Vec<InstanceInfo>>,
    ) -> EngineResult<f64> {
        let d = &self.d;
        let now = d.clock.now();
        let opt_id = OptimizationId::new(optimization_id);
        let list = match instances {
            Some(l) => l,
            None => d.compute.list_tagged(Some(&opt_id)).await?,
        };
        let agent_ids: HashSet<String> = d
            .ledger
            .list_agents(&opt_id)
            .await?
            .into_iter()
            .map(|a| a.id.to_string())
            .collect();

        struct Row {
            inst: InstanceInfo,
            usd: f64,
            to: i64,
        }
        let mut rows: Vec<Row> = Vec::new();
        for inst in list {
            if inst.optimization_id != opt_id {
                continue;
            }
            let from = d
                .kv
                .get_i64(&k::acct(&inst.id))
                .await?
                .unwrap_or(inst.launched_at);
            // A terminated instance with no recorded end time cannot be charged past what we already have.
            let ended = inst.terminated_at.unwrap_or(if inst.state == InstanceState::Terminated {
                from
            } else {
                now
            });
            let to = now.min(ended);
            if to <= from {
                continue;
            }
            let usd = self.hourly(&inst.instance_type).await * (to - from) as f64 / HOUR_MS;
            rows.push(Row { inst, usd, to });
        }
        if rows.is_empty() {
            return Ok(0.0);
        }

        let added: f64 = rows.iter().filter(|r| r.usd > 0.0).map(|r| r.usd).sum();
        let work: Vec<(RecordSpendInput, String, String)> = rows
            .iter()
            .map(|r| {
                let mut input = RecordSpendInput::new(
                    opt_id.clone(),
                    category(r.inst.role),
                    r.usd,
                    format!("ec2:{}", r.inst.id),
                );
                input.agent_id = r
                    .inst
                    .agent_id
                    .clone()
                    .filter(|a| agent_ids.contains(a.as_str()));
                input.estimated = Some(true);
                input.at = Some(now);
                (input, k::acct(&r.inst.id), r.to.to_string())
            })
            .collect();
        d.ledger
            .atomically(move |mut tx| {
                Box::pin(async move {
                    for (input, key, to) in work {
                        if input.usd > 0.0 {
                            tx.record_spend(input).await?;
                        }
                        tx.kv_set(&key, &to).await?;
                    }
                    Ok(())
                })
            })
            .await?;
        Ok(added)
    }
}
