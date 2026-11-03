//! Billing truth: AWS Cost Explorer by cost-allocation tag, and an in-memory mock. (TS `cost-explorer.ts`,
//! `cost-explorer.mock.ts`)

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use aws_sdk_costexplorer::types::{
    DateInterval, Expression, Granularity, GroupDefinition, GroupDefinitionType, MatchOption,
    ResultByTime, TagValues,
};

use crate::civil::{DAY_MS, ymd};
use crate::error::{Error, Result, sdk};
use crate::util::{TAG_AGENT, TAG_OPT, TAG_ROLE, TAG_SANDBOX, assert_valid_id, json_str};
use ratchet_core::enums::SpendCategory;
use ratchet_core::ports::{CostActuals, CostActualsQuery, CostReconciler, PortError, PortResult};

/// Map the value of the `ratchet:role` / `ratchet:agent` tags of a billed resource to a ledger category.
/// Runners and bench instances carry `ratchet:role`; agent-launched experiments carry only `ratchet:opt` +
/// `ratchet:agent` (the permission boundary forces exactly those); anything tagged only with `ratchet:opt` (volumes,
/// snapshots, data transfer attributed by tag) is `storage_egress`.
pub fn category_for_tags(role: &str, agent: &str) -> SpendCategory {
    match role {
        "agent_runner" => SpendCategory::AgentRunner,
        "bench" => SpendCategory::BenchRunner,
        "experiment" => SpendCategory::Experiment,
        "" if !agent.is_empty() => SpendCategory::Experiment,
        _ => SpendCategory::StorageEgress,
    }
}

/// Cost Explorer returns group keys as `<tag key>$<tag value>` (empty value when the tag is absent).
fn tag_value<'a>(key: Option<&'a String>, tag: &str) -> &'a str {
    key.and_then(|k| k.strip_prefix(&format!("{tag}$")))
        .unwrap_or("")
}

/// `CostReconciler` backed by AWS Cost Explorer `GetCostAndUsage`: unblended cost of everything tagged `ratchet:opt=<id>`,
/// grouped by the `ratchet:role` and `ratchet:agent` tags and mapped to ledger categories. Requires
/// `ce:GetCostAndUsage` and the `ratchet:*` tags activated as cost-allocation tags (docs/real-env/aws.md). Each request
/// costs $0.01.
#[derive(Clone)]
pub struct AwsCostReconciler {
    client: aws_sdk_costexplorer::Client,
    sandbox_id: Option<String>,
    accept_estimated: bool,
}

impl AwsCostReconciler {
    pub fn new(client: aws_sdk_costexplorer::Client) -> Self {
        Self {
            client,
            sandbox_id: None,
            accept_estimated: false,
        }
    }

    /// Cost Explorer is a global service served from us-east-1.
    pub async fn connect(profile: Option<&str>) -> Self {
        let cfg = crate::compute::aws::load_sdk_config("us-east-1", profile).await;
        Self::new(aws_sdk_costexplorer::Client::new(&cfg))
    }

    /// Also require this `ratchet:sandbox` tag value. Off by default: agent-launched experiment instances carry only
    /// `ratchet:opt` and `ratchet:agent`, so a sandbox filter would hide them.
    pub fn sandbox_id(mut self, sandbox_id: impl Into<String>) -> Self {
        self.sandbox_id = Some(sandbox_id.into());
        self
    }

    /// Treat days Cost Explorer flags `Estimated` as complete. Default false: such a window is reported incomplete
    /// (`pending`) and nothing is written.
    pub fn accept_estimated(mut self, accept: bool) -> Self {
        self.accept_estimated = accept;
        self
    }

    fn tag_filter(key: &str, value: &str) -> Expression {
        Expression::builder()
            .tags(
                TagValues::builder()
                    .key(key)
                    .values(value)
                    .match_options(MatchOption::Equals)
                    .build(),
            )
            .build()
    }
}

fn add_day(
    day: &ResultByTime,
    out: &mut BTreeMap<SpendCategory, f64>,
    estimated_days: &mut Vec<String>,
    groups: &mut u32,
) -> Result<()> {
    if day.estimated() {
        estimated_days.push(day.time_period().map_or("?", |t| t.start()).to_string());
    }
    for g in day.groups() {
        let raw = g
            .metrics()
            .and_then(|m| m.get("UnblendedCost"))
            .and_then(|m| m.amount());
        // JS `Number(raw)`: blank is 0, anything unparsable or non-finite is refused.
        let usd = raw.map(|r| {
            if r.trim().is_empty() {
                0.0
            } else {
                r.trim().parse::<f64>().unwrap_or(f64::NAN)
            }
        });
        let Some(usd) = usd.filter(|u| u.is_finite()) else {
            let shown = raw.map_or_else(|| "undefined".to_string(), json_str);
            return Err(Error::other(format!(
                "Cost Explorer returned a non-numeric amount {shown}"
            )));
        };
        if usd == 0.0 {
            continue;
        }
        *groups += 1;
        let category = category_for_tags(
            tag_value(g.keys().first(), TAG_ROLE),
            tag_value(g.keys().get(1), TAG_AGENT),
        );
        *out.entry(category).or_insert(0.0) += usd;
    }
    Ok(())
}

#[async_trait]
impl CostReconciler for AwsCostReconciler {
    async fn actuals(&self, q: &CostActualsQuery) -> PortResult<CostActuals> {
        assert_valid_id("optimization", q.optimization_id.as_str())?;
        if let Some(s) = &self.sandbox_id {
            assert_valid_id("sandbox", s)?;
        }
        if q.end_ms < q.start_ms {
            return Err(PortError::failed("cost window ends before it starts"));
        }

        // End is exclusive: the day after the one containing end_ms.
        let start = ymd(q.start_ms);
        let end = ymd(q.end_ms.div_euclid(DAY_MS) * DAY_MS + DAY_MS);
        let opt_filter = Self::tag_filter(TAG_OPT, q.optimization_id.as_str());
        let filter = match &self.sandbox_id {
            None => opt_filter,
            Some(s) => Expression::builder()
                .and(opt_filter)
                .and(Self::tag_filter(TAG_SANDBOX, s))
                .build(),
        };

        let mut by_category = BTreeMap::new();
        let mut groups = 0u32;
        let mut estimated_days: Vec<String> = vec![];
        let mut token: Option<String> = None;
        loop {
            let r = self
                .client
                .get_cost_and_usage()
                .time_period(
                    DateInterval::builder()
                        .start(&start)
                        .end(&end)
                        .build()
                        .map_err(|e| Error::other(e.to_string()))?,
                )
                .granularity(Granularity::Daily)
                .metrics("UnblendedCost")
                .filter(filter.clone())
                .group_by(
                    GroupDefinition::builder()
                        .r#type(GroupDefinitionType::Tag)
                        .key(TAG_ROLE)
                        .build(),
                )
                .group_by(
                    GroupDefinition::builder()
                        .r#type(GroupDefinitionType::Tag)
                        .key(TAG_AGENT)
                        .build(),
                )
                .set_next_page_token(token.clone())
                .send()
                .await
                .map_err(sdk)?;
            for day in r.results_by_time() {
                add_day(day, &mut by_category, &mut estimated_days, &mut groups)?;
            }
            token = r.next_page_token().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        let incomplete = !self.accept_estimated && !estimated_days.is_empty();
        let note = if incomplete {
            Some(format!(
                "Cost Explorer still marks {} day(s) as estimated ({}{}); data is not final",
                estimated_days.len(),
                estimated_days[0],
                if estimated_days.len() > 1 {
                    ", ..."
                } else {
                    ""
                }
            ))
        } else if groups == 0 {
            Some(format!(
                "Cost Explorer returned no cost for tag {TAG_OPT}={} between {start} and {end}: data may not be available yet (up to 24 h), or the ratchet:* tags are not activated as cost-allocation tags (Billing > Cost allocation tags; activation takes up to 24 h more)",
                q.optimization_id
            ))
        } else {
            None
        };
        Ok(CostActuals {
            by_category,
            groups,
            complete: !incomplete,
            note,
        })
    }
}

// ---------------------------------------------------------------- mock

type ActualsFn = Arc<dyn Fn(&CostActualsQuery) -> CostActuals + Send + Sync>;

enum Canned {
    Fixed(CostActuals),
    Computed(ActualsFn),
}

/// In-memory `CostReconciler` for tests and simulation mode; records every query. Default answer: complete, no cost.
#[derive(Default)]
pub struct MockCostReconciler {
    canned: Option<Canned>,
    error: Option<String>,
    calls: Mutex<Vec<CostActualsQuery>>,
}

impl MockCostReconciler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Canned answer.
    pub fn with_actuals(mut self, actuals: CostActuals) -> Self {
        self.canned = Some(Canned::Fixed(actuals));
        self
    }

    /// Answer computed from the query.
    pub fn with_actuals_fn(
        mut self,
        f: impl Fn(&CostActualsQuery) -> CostActuals + Send + Sync + 'static,
    ) -> Self {
        self.canned = Some(Canned::Computed(Arc::new(f)));
        self
    }

    /// Make every call fail (simulates Cost Explorer being unreachable / AccessDenied).
    pub fn with_error(mut self, message: impl Into<String>) -> Self {
        self.error = Some(message.into());
        self
    }

    /// Every query received so far.
    pub fn calls(&self) -> Vec<CostActualsQuery> {
        self.calls.lock().expect("calls poisoned").clone()
    }
}

#[async_trait]
impl CostReconciler for MockCostReconciler {
    async fn actuals(&self, q: &CostActualsQuery) -> PortResult<CostActuals> {
        self.calls.lock().expect("calls poisoned").push(q.clone());
        if let Some(e) = &self.error {
            return Err(PortError::failed(e.clone()));
        }
        Ok(match &self.canned {
            Some(Canned::Fixed(a)) => a.clone(),
            Some(Canned::Computed(f)) => f(q),
            None => CostActuals {
                by_category: BTreeMap::new(),
                groups: 0,
                complete: true,
                note: None,
            },
        })
    }
}

/// Build a `CostActuals` from per-category USD, as Cost Explorer would return it once data is final.
pub fn billed(by_category: &[(SpendCategory, f64)]) -> CostActuals {
    let map: BTreeMap<SpendCategory, f64> = by_category.iter().copied().collect();
    CostActuals {
        groups: map.values().filter(|v| **v != 0.0).count() as u32,
        by_category: map,
        complete: true,
        note: None,
    }
}
