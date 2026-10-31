//! Cost estimate for an environment (`cost.ts`). STUB: see PARITY.md.

use std::collections::HashMap;

use ratchet_core::enums::EnvironmentScope;
use ratchet_core::ports::PriceBook;

use crate::types::{EnvironmentCost, EnvironmentResource, EnvironmentRole};

/// Approximate us-east-1 Linux on-demand USD per hour, in the order of the TypeScript table.
pub const INSTANCE_USD_PER_HOUR: &[(&str, f64)] = &[];
/// EBS gp3, USD per GB-month (us-east-1).
pub const GP3_USD_PER_GB_MONTH: f64 = 0.08;
/// Minutes of sandbox start-up (VPC, endpoints, roles) before the first machine boots.
pub const BASE_STARTUP_MINUTES: u32 = 2;
/// Added start-up per machine (boot + user data + seed load).
pub const STARTUP_MINUTES_PER_MACHINE: u32 = 1;

pub fn static_hourly_usd(_instance_type: &str) -> Option<f64> {
    todo!()
}

pub fn volume_usd_per_hour(_gb: f64) -> f64 {
    todo!()
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineSpec {
    /// The key in the base module's `machines` map (absent for the default machines of `none` / `single`).
    pub key: Option<String>,
    pub name: String,
    pub role: EnvironmentRole,
    pub machine: String,
    pub detail: String,
    /// Data volume attached to the machine, GB (gp3).
    pub data_volume_gb: Option<f64>,
    /// Root volume, GB. Default 30.
    pub root_volume_gb: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct CostEstimateOptions {
    /// The run's time limit, seconds.
    pub time_limit_seconds: f64,
    /// Hourly price per instance type (default: the static table). Missing types are reported in `unpriced`.
    pub prices: Option<HashMap<String, f64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostEstimate {
    pub resources: Vec<EnvironmentResource>,
    pub cost: EnvironmentCost,
    /// Instance types the price table does not know: their rows are priced 0 and the draft must not pass the cost
    /// check.
    pub unpriced: Vec<String>,
}

pub fn estimate_machines_cost(
    _machines: &[MachineSpec],
    _opts: &CostEstimateOptions,
) -> CostEstimate {
    todo!()
}

pub async fn estimate_environment_cost_live(
    _machines: &[MachineSpec],
    _opts: &CostEstimateOptions,
    _book: &dyn PriceBook,
) -> CostEstimate {
    todo!()
}

pub fn default_machines(_scope: EnvironmentScope, _machine: &str) -> Vec<MachineSpec> {
    todo!()
}
