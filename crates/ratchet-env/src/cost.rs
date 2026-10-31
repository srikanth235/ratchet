//! Cost estimate for an environment (`cost.ts`).

use std::collections::HashMap;

use ratchet_core::enums::EnvironmentScope;
use ratchet_core::ports::PriceBook;

use crate::types::{EnvironmentCost, EnvironmentResource, EnvironmentRole};

/// Approximate us-east-1 Linux on-demand USD per hour, in the order of the TypeScript table. A static table is enough
/// for an ESTIMATE shown before anything is created; the real bill is reconciled from Cost Explorer after the run.
/// [`estimate_environment_cost_live`] takes a `PriceBook` and falls back to this table.
pub const INSTANCE_USD_PER_HOUR: &[(&str, f64)] = &[
    ("t3.medium", 0.0416),
    ("t3.large", 0.0832),
    ("t3.xlarge", 0.1664),
    ("t3.2xlarge", 0.3328),
    ("t4g.small", 0.0168),
    ("t4g.medium", 0.0336),
    ("t4g.large", 0.0672),
    ("t4g.xlarge", 0.1344),
    ("c7i.large", 0.08925),
    ("c7i.xlarge", 0.1785),
    ("c7i.2xlarge", 0.357),
    ("c7i.4xlarge", 0.714),
    ("c7i.8xlarge", 1.428),
    ("c7i.12xlarge", 2.142),
    ("c7i.16xlarge", 2.856),
    ("c7g.large", 0.0725),
    ("c7g.xlarge", 0.145),
    ("c7g.2xlarge", 0.29),
    ("c7g.4xlarge", 0.58),
    ("m7i.large", 0.1008),
    ("m7i.xlarge", 0.2016),
    ("m7i.2xlarge", 0.4032),
    ("m7i.4xlarge", 0.8064),
    ("m7i.8xlarge", 1.6128),
    ("m7g.large", 0.0816),
    ("m7g.xlarge", 0.1632),
    ("m7g.2xlarge", 0.3264),
    ("m7g.4xlarge", 0.6528),
    ("r7i.large", 0.1323),
    ("r7i.xlarge", 0.2646),
    ("r7i.2xlarge", 0.5292),
    ("r7i.4xlarge", 1.0584),
    ("r7g.large", 0.1071),
    ("r7g.xlarge", 0.2142),
    ("r7g.2xlarge", 0.4284),
    ("r7g.4xlarge", 0.8568),
];
/// EBS gp3, USD per GB-month (us-east-1).
pub const GP3_USD_PER_GB_MONTH: f64 = 0.08;
const HOURS_PER_MONTH: f64 = 730.0;
/// Minutes of sandbox start-up (VPC, endpoints, roles) before the first machine boots.
pub const BASE_STARTUP_MINUTES: u32 = 2;
/// Added start-up per machine (boot + user data + seed load).
pub const STARTUP_MINUTES_PER_MACHINE: u32 = 1;

/// Hourly USD of an instance type from the static table, or `None` when the table does not know it.
pub fn static_hourly_usd(instance_type: &str) -> Option<f64> {
    INSTANCE_USD_PER_HOUR
        .iter()
        .find(|(t, _)| *t == instance_type)
        .map(|(_, p)| *p)
}

/// Hourly cost of `gb` gigabytes of gp3.
pub fn volume_usd_per_hour(gb: f64) -> f64 {
    (gb.max(0.0) * GP3_USD_PER_GB_MONTH) / HOURS_PER_MONTH
}

/// JavaScript `Math.round` (halves round up).
pub(crate) fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

fn round(n: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    js_round(n * f) / f
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

/// Pure cost math for a set of machines: `usd_per_hour` is the sum of instance and volume prices (rounded to cents of
/// a cent), `startup_minutes` is the sandbox boot plus one minute per machine (0 with no machine), and the run
/// estimate bills the machines for the run's time limit plus the start-up (they are up while the stack loads).
pub fn estimate_machines_cost(
    machines: &[MachineSpec],
    opts: &CostEstimateOptions,
) -> CostEstimate {
    let price_of = |t: &str| match &opts.prices {
        Some(p) => p.get(t).copied(),
        None => static_hourly_usd(t),
    };
    let mut unpriced: Vec<String> = vec![];
    let resources: Vec<EnvironmentResource> = machines
        .iter()
        .map(|m| {
            let base = price_of(&m.machine);
            if base.is_none() && !unpriced.contains(&m.machine) {
                unpriced.push(m.machine.clone());
            }
            let usd = base.unwrap_or(0.0)
                + volume_usd_per_hour(
                    m.root_volume_gb.unwrap_or(30.0) + m.data_volume_gb.unwrap_or(0.0),
                );
            EnvironmentResource {
                name: m.name.clone(),
                role: m.role,
                machine: m.machine.clone(),
                detail: m.detail.clone(),
                usd_per_hour: round(usd, 4),
            }
        })
        .collect();
    let usd_per_hour = round(resources.iter().fold(0.0, |s, r| s + r.usd_per_hour), 4);
    let startup_minutes = if machines.is_empty() {
        0
    } else {
        BASE_STARTUP_MINUTES + STARTUP_MINUTES_PER_MACHINE * machines.len() as u32
    };
    let run_hours = opts.time_limit_seconds.max(0.0) / 3600.0;
    let estimate_for_run_usd = round(
        usd_per_hour * (run_hours + f64::from(startup_minutes) / 60.0),
        2,
    );
    CostEstimate {
        resources,
        cost: EnvironmentCost {
            usd_per_hour,
            startup_minutes,
            estimate_for_run_usd,
        },
        unpriced,
    }
}

/// As [`estimate_machines_cost`], with prices from a live `PriceBook`; a type the book cannot price falls back to the
/// static table.
pub async fn estimate_environment_cost_live(
    machines: &[MachineSpec],
    opts: &CostEstimateOptions,
    book: &dyn PriceBook,
) -> CostEstimate {
    let mut prices: HashMap<String, f64> = INSTANCE_USD_PER_HOUR
        .iter()
        .map(|(t, p)| ((*t).to_owned(), *p))
        .collect();
    let mut seen: Vec<&str> = vec![];
    for m in machines {
        if seen.contains(&m.machine.as_str()) {
            continue;
        }
        seen.push(&m.machine);
        if let Ok(p) = book.hourly_usd(&m.machine).await {
            prices.insert(m.machine.clone(), p);
        }
    }
    estimate_machines_cost(
        machines,
        &CostEstimateOptions {
            time_limit_seconds: opts.time_limit_seconds,
            prices: Some(prices),
        },
    )
}

/// The one machine of a scope that needs no drafting: the evaluator's host (`none`) or the pinned machine (`single`).
pub fn default_machines(scope: EnvironmentScope, machine: &str) -> Vec<MachineSpec> {
    match scope {
        EnvironmentScope::Full => vec![],
        EnvironmentScope::None => vec![MachineSpec {
            key: None,
            name: "Evaluator".into(),
            role: EnvironmentRole::Evaluator,
            machine: machine.to_owned(),
            detail: "runs the evaluation in its container, on pinned hardware".into(),
            data_volume_gb: None,
            root_volume_gb: None,
        }],
        EnvironmentScope::Single => vec![MachineSpec {
            key: None,
            name: "Machine".into(),
            role: EnvironmentRole::App,
            machine: machine.to_owned(),
            detail: "builds and measures the repo on pinned hardware".into(),
            data_volume_gb: None,
            root_volume_gb: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ratchet_core::ports::{PortError, PortResult};

    fn m(machine: &str) -> MachineSpec {
        MachineSpec {
            key: None,
            name: machine.into(),
            role: EnvironmentRole::App,
            machine: machine.into(),
            detail: String::new(),
            data_volume_gb: None,
            root_volume_gb: Some(0.0),
        }
    }
    fn opts(secs: f64) -> CostEstimateOptions {
        CostEstimateOptions {
            time_limit_seconds: secs,
            prices: None,
        }
    }
    fn close(a: f64, b: f64, digits: i32) -> bool {
        (a - b).abs() < 10f64.powi(-digits) / 2.0
    }

    // "sums the instance prices and prices the run for its time limit plus start-up"
    #[test]
    fn sums_instance_prices_and_prices_the_run() {
        let e = estimate_machines_cost(
            &[
                m("c7i.2xlarge"),
                m("r7g.large"),
                m("t4g.medium"),
                m("c7i.large"),
            ],
            &opts(7200.0),
        );
        assert!(close(e.cost.usd_per_hour, 0.587, 3));
        assert_eq!(
            e.cost.startup_minutes,
            BASE_STARTUP_MINUTES + 4 * STARTUP_MINUTES_PER_MACHINE
        );
        assert!(close(
            e.cost.estimate_for_run_usd,
            0.58695 * (2.0 + 6.0 / 60.0),
            2
        ));
        assert!(e.unpriced.is_empty());
        let per: Vec<f64> = e.resources.iter().map(|r| r.usd_per_hour).collect();
        assert_eq!(per, vec![0.357, 0.1071, 0.0336, 0.0893]);
    }

    // "adds gp3 for the root and data volumes"
    #[test]
    fn adds_gp3_for_root_and_data_volumes() {
        let mut spec = m("c7i.large");
        spec.root_volume_gb = Some(30.0);
        spec.data_volume_gb = Some(970.0);
        let e = estimate_machines_cost(&[spec], &opts(3600.0));
        assert!(close(
            e.resources[0].usd_per_hour,
            0.08925 + (1000.0 * GP3_USD_PER_GB_MONTH) / 730.0,
            4
        ));
        assert_eq!(volume_usd_per_hour(0.0), 0.0);
        assert_eq!(volume_usd_per_hour(-5.0), 0.0);
        assert!(close(volume_usd_per_hour(730.0), GP3_USD_PER_GB_MONTH, 10));
    }

    // "defaults the root volume to 30 GB"
    #[test]
    fn defaults_the_root_volume_to_30_gb() {
        let mut spec = m("c7i.large");
        spec.root_volume_gb = None;
        let e = estimate_machines_cost(&[spec], &opts(3600.0));
        assert!(close(
            e.resources[0].usd_per_hour,
            0.08925 + (30.0 * GP3_USD_PER_GB_MONTH) / 730.0,
            4
        ));
    }

    // "has no start-up and no cost without machines"
    #[test]
    fn has_no_startup_and_no_cost_without_machines() {
        let c = estimate_machines_cost(&[], &opts(7200.0)).cost;
        assert_eq!(
            c,
            EnvironmentCost {
                usd_per_hour: 0.0,
                startup_minutes: 0,
                estimate_for_run_usd: 0.0
            }
        );
    }

    // "scales with the time limit"
    #[test]
    fn scales_with_the_time_limit() {
        let one = estimate_machines_cost(&[m("c7i.large")], &opts(3600.0))
            .cost
            .estimate_for_run_usd;
        let four = estimate_machines_cost(&[m("c7i.large")], &opts(4.0 * 3600.0))
            .cost
            .estimate_for_run_usd;
        assert!(four > one * 3.0);
        assert!(
            estimate_machines_cost(&[m("c7i.large")], &opts(-5.0))
                .cost
                .estimate_for_run_usd
                >= 0.0
        );
    }

    // "reports instance types it cannot price, once, and prices them 0 (the validator then fails the cost check)"
    #[test]
    fn reports_unpriced_types_once() {
        let e = estimate_machines_cost(
            &[m("x9z.huge"), m("x9z.huge"), m("c7i.large")],
            &opts(3600.0),
        );
        assert_eq!(e.unpriced, vec!["x9z.huge".to_string()]);
        assert_eq!(e.resources[0].usd_per_hour, 0.0);
    }

    // "takes an explicit price table"
    #[test]
    fn takes_an_explicit_price_table() {
        let e = estimate_machines_cost(
            &[m("foo.bar")],
            &CostEstimateOptions {
                time_limit_seconds: 3600.0,
                prices: Some(HashMap::from([("foo.bar".to_string(), 2.0)])),
            },
        );
        assert_eq!(e.cost.usd_per_hour, 2.0);
        assert!(e.unpriced.is_empty());
    }

    // "covers the families a draft uses, with sane relative prices"
    #[test]
    fn price_table_covers_the_families_a_draft_uses() {
        for t in [
            "c7i.large",
            "c7i.2xlarge",
            "m7i.xlarge",
            "r7g.large",
            "r7i.large",
            "t4g.medium",
            "m7g.large",
            "c7g.large",
        ] {
            assert!(static_hourly_usd(t).unwrap() > 0.0, "{t}");
        }
        assert!(close(
            static_hourly_usd("c7i.xlarge").unwrap(),
            2.0 * static_hourly_usd("c7i.large").unwrap(),
            3
        ));
        assert!(close(
            static_hourly_usd("c7i.2xlarge").unwrap(),
            2.0 * static_hourly_usd("c7i.xlarge").unwrap(),
            3
        ));
        assert_eq!(static_hourly_usd("nope.nope"), None);
        assert!(INSTANCE_USD_PER_HOUR.iter().all(|(_, p)| *p > 0.0));
    }

    struct Book;
    #[async_trait]
    impl PriceBook for Book {
        async fn hourly_usd(&self, t: &str) -> PortResult<f64> {
            if t == "c7i.large" {
                Ok(1.0)
            } else {
                Err(PortError::failed("no price"))
            }
        }
    }

    // "prefers the live price book and falls back to the table per instance type"
    #[tokio::test]
    async fn prefers_the_live_price_book_and_falls_back() {
        let e =
            estimate_environment_cost_live(&[m("c7i.large"), m("r7g.large")], &opts(3600.0), &Book)
                .await;
        let per: Vec<f64> = e.resources.iter().map(|r| r.usd_per_hour).collect();
        assert_eq!(per, vec![1.0, 0.1071]);
        assert!(e.unpriced.is_empty());
    }

    // "none is the evaluator host, single is one pinned machine, full has none of its own"
    #[test]
    fn default_machines_per_scope() {
        let n = default_machines(EnvironmentScope::None, "c7i.large");
        assert_eq!(
            (n[0].role, n[0].machine.as_str()),
            (EnvironmentRole::Evaluator, "c7i.large")
        );
        let s = default_machines(EnvironmentScope::Single, "c7i.xlarge");
        assert_eq!(
            (s[0].role, s[0].machine.as_str()),
            (EnvironmentRole::App, "c7i.xlarge")
        );
        assert!(default_machines(EnvironmentScope::Full, "c7i.xlarge").is_empty());
    }
}
