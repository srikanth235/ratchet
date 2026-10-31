//! The cost meter (live spend estimate from instance lifetimes) and the budget guard. (TS `cost.ts`)

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::domain::{
    AgentId, Clock, InstanceInfo, InstanceRole, InstanceState, OptimizationId, SpendCategory,
};
use crate::error::{Error, Result};
use crate::ports::{Compute, Pricing};
use crate::util::SystemClock;

/// The ledger categories the meter produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeteredCategory {
    AgentRunner,
    Experiment,
    BenchRunner,
}

impl MeteredCategory {
    pub fn spend_category(self) -> SpendCategory {
        match self {
            Self::AgentRunner => SpendCategory::AgentRunner,
            Self::Experiment => SpendCategory::Experiment,
            Self::BenchRunner => SpendCategory::BenchRunner,
        }
    }
}

/// Instance role -> ledger spend category (packages/core/src/ledger/schema.ts).
pub fn role_category(role: InstanceRole) -> MeteredCategory {
    match role {
        InstanceRole::AgentRunner => MeteredCategory::AgentRunner,
        InstanceRole::Experiment => MeteredCategory::Experiment,
        InstanceRole::Bench => MeteredCategory::BenchRunner,
    }
}

/// Shape accepted by the ledger's `recordSpend` (`RecordSpendInput`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendDelta {
    pub optimization_id: OptimizationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    pub category: MeteredCategory,
    pub usd: f64,
    /// Always true: these are estimates until reconciled against billing.
    pub estimated: bool,
    pub source: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceCost {
    pub instance_id: String,
    pub optimization_id: OptimizationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    pub category: MeteredCategory,
    pub instance_type: String,
    pub billed_seconds: f64,
    pub hourly_usd: f64,
    pub usd: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationCost {
    pub total_usd: f64,
    pub by_category: BTreeMap<MeteredCategory, f64>,
    pub by_agent: BTreeMap<AgentId, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSnapshot {
    pub at: i64,
    pub total_usd: f64,
    pub by_optimization: BTreeMap<OptimizationId, OptimizationCost>,
    pub instances: Vec<InstanceCost>,
    /// Instance types the price book could not price (cost counted as 0 - treat as a metering gap).
    pub unpriced: Vec<String>,
}

pub struct CostMeterOptions {
    pub compute: Arc<dyn Compute>,
    pub prices: Arc<dyn Pricing>,
    pub clock: Arc<dyn Clock>,
    /// EC2 Linux bills per second with a 60 s minimum.
    pub min_billed_seconds: f64,
    pub source: String,
}

impl CostMeterOptions {
    pub fn new(compute: Arc<dyn Compute>, prices: Arc<dyn Pricing>) -> Self {
        Self {
            compute,
            prices,
            clock: Arc::new(SystemClock),
            min_billed_seconds: 60.0,
            source: "cost-meter".to_string(),
        }
    }

    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
}

#[derive(Default)]
struct MeterState {
    /// instance id -> usd already emitted
    emitted: HashMap<String, f64>,
    /// instance id -> first time we saw it ended without a timestamp
    ended_at: HashMap<String, i64>,
}

/// Live spend estimate: instance lifetimes x on-demand price. `collect()` is meant to run about once a minute and
/// returns only the new spend since the previous call, ready for the ledger's `recordSpend`.
pub struct CostMeter {
    opts: CostMeterOptions,
    state: Mutex<MeterState>,
}

impl CostMeter {
    pub fn new(opts: CostMeterOptions) -> Self {
        Self {
            opts,
            state: Mutex::default(),
        }
    }

    /// Cumulative cost of every tagged instance (optionally one optimization).
    pub async fn snapshot(&self, optimization_id: Option<&str>) -> Result<CostSnapshot> {
        let now = self.opts.clock.now_ms();
        let list = self.opts.compute.list_tagged(optimization_id).await?;
        let mut snap = CostSnapshot {
            at: now,
            total_usd: 0.0,
            by_optimization: BTreeMap::new(),
            instances: vec![],
            unpriced: vec![],
        };
        let mut price_cache: HashMap<String, Option<f64>> = HashMap::new();
        for i in &list {
            let price = match price_cache.get(&i.instance_type) {
                Some(p) => *p,
                None => {
                    let p = self.opts.prices.hourly_usd(&i.instance_type).await.ok();
                    price_cache.insert(i.instance_type.clone(), p);
                    if p.is_none() {
                        snap.unpriced.push(i.instance_type.clone());
                    }
                    p
                }
            };
            let seconds = self.billed_seconds(i, now);
            let usd = price.map_or(0.0, |p| (seconds / 3600.0) * p);
            let category = role_category(i.role);
            snap.instances.push(InstanceCost {
                instance_id: i.id.clone(),
                optimization_id: i.optimization_id.clone(),
                agent_id: i.agent_id.clone(),
                category,
                instance_type: i.instance_type.clone(),
                billed_seconds: seconds,
                hourly_usd: price.unwrap_or(0.0),
                usd,
            });
            snap.total_usd += usd;
            let o = snap
                .by_optimization
                .entry(i.optimization_id.clone())
                .or_default();
            o.total_usd += usd;
            *o.by_category.entry(category).or_insert(0.0) += usd;
            if let Some(a) = &i.agent_id {
                *o.by_agent.entry(a.clone()).or_insert(0.0) += usd;
            }
        }
        Ok(snap)
    }

    /// New spend since the previous `collect()`, aggregated per (optimization, agent, category).
    pub async fn collect(&self, optimization_id: Option<&str>) -> Result<Vec<SpendDelta>> {
        let snap = self.snapshot(optimization_id).await?;
        let mut order: Vec<SpendDelta> = vec![];
        let mut index: HashMap<(String, String, MeteredCategory), usize> = HashMap::new();
        let mut state = self.state.lock().expect("meter state poisoned");
        for c in &snap.instances {
            let delta = c.usd - state.emitted.get(&c.instance_id).copied().unwrap_or(0.0);
            if delta <= 1e-9 {
                continue;
            }
            state.emitted.insert(c.instance_id.clone(), c.usd);
            let key = (
                c.optimization_id.clone(),
                c.agent_id.clone().unwrap_or_default(),
                c.category,
            );
            match index.get(&key) {
                Some(&i) => order[i].usd += delta,
                None => {
                    index.insert(key, order.len());
                    order.push(SpendDelta {
                        optimization_id: c.optimization_id.clone(),
                        agent_id: c.agent_id.clone(),
                        category: c.category,
                        usd: delta,
                        estimated: true,
                        source: self.opts.source.clone(),
                        at: snap.at,
                    });
                }
            }
        }
        Ok(order)
    }

    fn billed_seconds(&self, i: &InstanceInfo, now: i64) -> f64 {
        let end = if matches!(i.state, InstanceState::Terminated | InstanceState::Stopped) {
            let mut state = self.state.lock().expect("meter state poisoned");
            match i
                .terminated_at
                .or_else(|| state.ended_at.get(&i.id).copied())
            {
                Some(e) => e,
                None => {
                    state.ended_at.insert(i.id.clone(), now);
                    now
                }
            }
        } else {
            now
        };
        let secs = ((end - i.launched_at) as f64 / 1000.0).max(0.0);
        secs.max(self.opts.min_billed_seconds)
    }
}

// ---------------------------------------------------------------- budget

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetState {
    Ok,
    Warn,
    Tripped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Crossed {
    Warn,
    Trip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetStatus {
    pub state: BudgetState,
    pub spent_usd: f64,
    pub budget_usd: f64,
    pub fraction: f64,
    /// Set only on the call that crossed a threshold, so callers act exactly once.
    pub crossed: Option<Crossed>,
}

type StatusCallback = Box<dyn Fn(&BudgetStatus) + Send + Sync>;

/// Warn at 80% of budget, trip (kill switch) at 100%. State only moves forward.
pub struct BudgetGuard {
    budget_usd: f64,
    warn_fraction: f64,
    state: BudgetState,
    on_warn: Option<StatusCallback>,
    on_trip: Option<StatusCallback>,
}

impl BudgetGuard {
    pub fn new(budget_usd: f64) -> Result<Self> {
        if budget_usd.is_nan() || budget_usd <= 0.0 {
            return Err(Error::invalid("budgetUsd must be > 0"));
        }
        Ok(Self {
            budget_usd,
            warn_fraction: 0.8,
            state: BudgetState::Ok,
            on_warn: None,
            on_trip: None,
        })
    }

    pub fn warn_fraction(mut self, fraction: f64) -> Self {
        self.warn_fraction = fraction;
        self
    }

    pub fn on_warn(mut self, f: impl Fn(&BudgetStatus) + Send + Sync + 'static) -> Self {
        self.on_warn = Some(Box::new(f));
        self
    }

    /// Wire the kill switch + reaper here.
    pub fn on_trip(mut self, f: impl Fn(&BudgetStatus) + Send + Sync + 'static) -> Self {
        self.on_trip = Some(Box::new(f));
        self
    }

    /// `spent_usd` is the total for the optimization (infra + tokens).
    pub fn update(&mut self, spent_usd: f64) -> BudgetStatus {
        let fraction = spent_usd / self.budget_usd;
        let mut crossed = None;
        if fraction >= 1.0 && self.state != BudgetState::Tripped {
            self.state = BudgetState::Tripped;
            crossed = Some(Crossed::Trip);
        } else if fraction >= self.warn_fraction && self.state == BudgetState::Ok {
            self.state = BudgetState::Warn;
            crossed = Some(Crossed::Warn);
        }
        let status = BudgetStatus {
            state: self.state,
            spent_usd,
            budget_usd: self.budget_usd,
            fraction,
            crossed,
        };
        match crossed {
            Some(Crossed::Warn) => {
                if let Some(f) = &self.on_warn {
                    f(&status);
                }
            }
            Some(Crossed::Trip) => {
                if let Some(f) = &self.on_trip {
                    f(&status);
                }
            }
            None => {}
        }
        status
    }

    pub fn current(&self) -> BudgetState {
        self.state
    }
}
