//! The read side over the ledger: every view model the screens render, built from `ratchet-ledger` rows.
//! Port of `packages/desktop/src/main/mappers.ts` (+ the ledger parts of `service.ts`). The simulation uses it as is;
//! the real engine can too, since the ledger is the single source of truth for what a run has done.

use std::collections::BTreeMap;
use std::sync::Arc;

use ratchet_core::enums::{CandidateVerdict, Direction, OptimizationStatus, SpendCategory};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::numfmt::to_fixed;
use ratchet_core::ports::{Clock, FileDiff};
use ratchet_core::rows::{
    ActivityItem, Agent, CandidateWithResults, Optimization, OptimizationSummary, SpendBreakdown,
};
use ratchet_ledger::Ledger;
use sqlx::Row;

use super::format::{MIN_MS, fmt_num, rel_time, unit_for_metric};
use super::kpis::{KpiInput, SpendAt, build_kpis};
use super::metrics::{compare_rows, metric_options, span_rows, stat_of, stats_map};
use super::value::value_of;
use crate::api::*;
use crate::service::{ServiceError, ServiceResult};

/// The pull request URL of an optimization (state the ledger does not hold).
pub type PrUrlLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Read-only view builders over a [`Ledger`].
#[derive(Clone)]
pub struct LedgerViews {
    pub ledger: Ledger,
    pub clock: Arc<dyn Clock>,
    pub pr_url: PrUrlLookup,
}

fn opt_id(id: &str) -> OptimizationId {
    OptimizationId::new(id)
}

fn category_label(c: SpendCategory) -> &'static str {
    match c {
        SpendCategory::AgentRunner => "Agent runners",
        SpendCategory::Experiment => "Experiment instances",
        SpendCategory::BenchRunner => "Bench runner",
        SpendCategory::StorageEgress => "Storage & egress",
        SpendCategory::Tokens => "Model tokens",
    }
}

fn ui_status_str(s: UiStatus) -> &'static str {
    match s {
        UiStatus::Draft => "draft",
        UiStatus::Baselining => "baselining",
        UiStatus::Running => "running",
        UiStatus::Stopping => "stopping",
        UiStatus::Completed => "completed",
        UiStatus::Stopped => "stopped",
        UiStatus::Failed => "failed",
        UiStatus::PrOpen => "pr-open",
    }
}

fn opt_name(o: &impl NameParts) -> String {
    o.name_parts()
}

trait NameParts {
    fn name_parts(&self) -> String;
}

impl NameParts for OptimizationSummary {
    fn name_parts(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{} {}", self.repo, self.metric))
    }
}

impl NameParts for Optimization {
    fn name_parts(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{} {}", self.repo, self.metric))
    }
}

/// Parsed `{ text, data }` payload of an agent event.
struct Payload {
    text: String,
    from: Option<String>,
}

fn parse_payload(raw: Option<&serde_json::Value>) -> Payload {
    match raw {
        None | Some(serde_json::Value::Null) => Payload {
            text: String::new(),
            from: None,
        },
        Some(serde_json::Value::Object(o)) => Payload {
            text: o
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_owned(),
            from: o
                .get("data")
                .and_then(|d| d.get("from"))
                .and_then(|f| f.as_str())
                .map(str::to_owned),
        },
        Some(serde_json::Value::String(s)) => Payload {
            text: s.clone(),
            from: None,
        },
        Some(other) => Payload {
            text: other.to_string(),
            from: None,
        },
    }
}

fn parse_payload_text(raw: Option<String>) -> Payload {
    match raw {
        None => parse_payload(None),
        Some(s) => match serde_json::from_str::<serde_json::Value>(&s) {
            Ok(v) => parse_payload(Some(&v)),
            Err(_) => Payload {
                text: s,
                from: None,
            },
        },
    }
}

impl LedgerViews {
    pub fn new(ledger: Ledger, clock: Arc<dyn Clock>, pr_url: PrUrlLookup) -> Self {
        Self {
            ledger,
            clock,
            pr_url,
        }
    }

    pub fn now(&self) -> i64 {
        self.clock.now()
    }

    async fn require(&self, id: &str) -> ServiceResult<Optimization> {
        self.ledger
            .get_optimization(&opt_id(id))
            .await?
            .ok_or_else(|| ServiceError::not_found("optimization", id))
    }

    // ---- optimizations -----------------------------------------------------------------------------------

    pub fn optimization_vm(&self, s: &OptimizationSummary, row: &Optimization) -> OptimizationVm {
        let live = matches!(
            s.status,
            OptimizationStatus::Running | OptimizationStatus::Baselining
        );
        let pr = (self.pr_url)(s.id.as_str());
        let status = if s.status == OptimizationStatus::Completed && pr.is_some() {
            UiStatus::PrOpen
        } else {
            s.status.into()
        };
        OptimizationVm {
            id: s.id.to_string(),
            name: opt_name(s),
            repo: s.repo.clone(),
            metric: s.metric.clone(),
            unit: unit_for_metric(&s.metric).to_owned(),
            direction: s.direction.into(),
            baseline: s.baseline_value,
            best: s.best_value,
            gain_pct: s.gain_pct,
            status,
            agents: s.agent_kinds.clone(),
            spend_usd: s.spend_usd,
            budget_usd: s.budget_usd,
            updated: if live {
                "live".into()
            } else {
                rel_time(
                    s.ended_at.or(s.started_at).unwrap_or(s.created_at),
                    self.now(),
                )
            },
            candidate_count: s.candidate_count,
            verified_wins: s.verified_wins,
            created_at: s.created_at,
            pack: row.pack.clone(),
            evaluator_kind: row.evaluator_kind,
            value: value_of(row, s.baseline_value, s.best_value, s.spend_usd),
        }
    }

    /// Summaries with their rows, newest first.
    pub async fn rows(&self) -> ServiceResult<Vec<(OptimizationSummary, Optimization)>> {
        let summaries = self.ledger.list_optimizations(None).await?;
        let mut out = Vec::with_capacity(summaries.len());
        for s in summaries {
            let row = self
                .ledger
                .get_optimization(&s.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("optimization", s.id.as_str()))?;
            out.push((s, row));
        }
        Ok(out)
    }

    /// Every spend event as `(optimization, at, usd)`.
    pub async fn all_spend(&self) -> ServiceResult<Vec<SpendAt>> {
        let rows = sqlx::query("SELECT optimization_id, at, usd FROM spend_events ORDER BY at, id")
            .fetch_all(self.ledger.pool())
            .await
            .map_err(db)?;
        Ok(rows
            .iter()
            .map(|r| SpendAt {
                opt: r.get::<String, _>("optimization_id"),
                at: r.get::<i64, _>("at"),
                usd: r.get::<f64, _>("usd"),
            })
            .collect())
    }

    pub async fn snapshot(
        &self,
        range: KpiRange,
        engine: EngineInfo,
    ) -> ServiceResult<OptimizationsSnapshot> {
        let now = self.now();
        let rows = self.rows().await?;
        let mut histories = Vec::with_capacity(rows.len());
        for (s, _) in &rows {
            histories.push(self.ledger.ratchet_history(&s.id).await?);
        }
        let inputs: Vec<KpiInput<'_>> = rows
            .iter()
            .zip(&histories)
            .map(|((s, r), h)| KpiInput {
                summary: s,
                row: r,
                history: h,
            })
            .collect();
        let spend = self.all_spend().await?;
        let kpis = build_kpis(&inputs, &spend, now, range);
        let featured = rows
            .iter()
            .find(|(s, _)| {
                matches!(
                    s.status,
                    OptimizationStatus::Running | OptimizationStatus::Baselining
                )
            })
            .or_else(|| rows.iter().find(|(s, _)| s.baseline_value.is_some()))
            .map(|(s, _)| s.id.clone());
        let featured_series = match &featured {
            Some(id) => Some(self.series(id.as_str(), None).await?),
            None => None,
        };
        Ok(OptimizationsSnapshot {
            range,
            kpis,
            featured_id: featured.map(|f| f.to_string()),
            featured_series,
            optimizations: rows
                .iter()
                .map(|(s, r)| self.optimization_vm(s, r))
                .collect(),
            activity: self.activity(8, None).await?,
            engine,
        })
    }

    // ---- series ------------------------------------------------------------------------------------------

    /// The ratchet chart. For the objective: baseline step, one step per ratchet move, a point per benched
    /// candidate. For another metric of the objective's family: the same commits plotted by that metric's median.
    pub async fn series(&self, id: &str, metric: Option<&str>) -> ServiceResult<RatchetSeries> {
        let opt = self.require(id).await?;
        let oid = opt_id(id);
        let t0 = opt.started_at.unwrap_or(opt.created_at);
        let minutes = |at: i64| ((at - t0) as f64 / MIN_MS as f64).max(0.0);
        let baseline = self.ledger.get_baseline(&oid).await?;
        let moves = self.ledger.ratchet_history(&oid).await?;
        let cands = self.ledger.candidates_with_results(&oid).await?;
        let options = baseline
            .as_ref()
            .map(|b| metric_options(&opt.metric, &opt.config_yaml, &stats_map(&b.stats)))
            .unwrap_or_default();
        let key = match metric {
            Some(m) if options.iter().any(|o| o.key == m) => m.to_owned(),
            _ => opt.metric.clone(),
        };
        let objective = key == opt.metric;
        let bench_stats = |c: &CandidateWithResults| {
            c.results
                .iter()
                .rev()
                .find(|r| r.phase == ratchet_core::BenchPhase::Bench)
                .map(|r| stats_map(&r.stats))
        };
        let value_of_cand = |c: &CandidateWithResults| {
            if objective {
                c.bench_value
            } else {
                stat_of(bench_stats(c).as_ref(), &key, &opt.metric)
            }
        };
        let by_id: BTreeMap<&str, &CandidateWithResults> =
            cands.iter().map(|c| (c.candidate.id.as_str(), c)).collect();

        let mut steps = Vec::new();
        let base = baseline.as_ref().and_then(|b| {
            if objective {
                Some(b.value)
            } else {
                stat_of(Some(&stats_map(&b.stats)), &key, &opt.metric)
            }
        });
        if let Some(v) = base {
            steps.push(StepPoint { t: 0.0, value: v });
        }
        for m in &moves {
            let v = if objective {
                Some(m.to_value)
            } else {
                by_id
                    .get(m.candidate_id.as_str())
                    .and_then(|c| value_of_cand(c))
            };
            if let Some(v) = v {
                steps.push(StepPoint {
                    t: minutes(m.at),
                    value: v,
                });
            }
        }

        let points: Vec<RatchetPoint> = cands
            .iter()
            .filter_map(|c| {
                let v = value_of_cand(c)?;
                Some(RatchetPoint {
                    t: minutes(c.candidate.submitted_at),
                    value: v,
                    agent: c.agent_kind,
                    verified: c.moved_ratchet,
                    label: Some(format!(
                        "{} \u{00b7} {}",
                        short_sha(&c.candidate.sha),
                        if c.candidate.message.is_empty() {
                            c.candidate.verdict.to_string()
                        } else {
                            c.candidate.message.clone()
                        }
                    )),
                })
            })
            .collect();

        let last = steps
            .iter()
            .map(|s| s.t)
            .chain(points.iter().map(|p| p.t))
            .fold(0.0, f64::max);
        let live = matches!(
            opt.status,
            OptimizationStatus::Running | OptimizationStatus::Baselining
        );
        Ok(RatchetSeries {
            unit: unit_for_metric(&key).to_owned(),
            options: if options.is_empty() {
                vec![MetricOption {
                    key: opt.metric.clone(),
                    label: opt.metric.clone(),
                    unit: unit_for_metric(&opt.metric).to_owned(),
                    objective: true,
                }]
            } else {
                options
            },
            metric: key,
            direction: opt.direction.into(),
            baseline: base,
            steps,
            points,
            noise_floor: if objective {
                baseline.as_ref().map(|b| b.noise_floor)
            } else {
                None
            },
            t_max: if live {
                last.max(minutes(self.now()))
            } else {
                last
            },
        })
    }

    // ---- candidates --------------------------------------------------------------------------------------

    pub async fn candidates(&self, id: &str) -> ServiceResult<Vec<CandidateVm>> {
        let opt = self.require(id).await?;
        let oid = opt_id(id);
        let baseline = self.ledger.get_baseline(&oid).await?.map(|b| b.value);
        let t0 = opt.started_at.unwrap_or(opt.created_at);
        Ok(self
            .ledger
            .candidates_with_results(&oid)
            .await?
            .iter()
            .map(|c| candidate_vm(c, t0, baseline, opt.direction))
            .collect())
    }

    // ---- spend -------------------------------------------------------------------------------------------

    pub async fn spend(&self, id: &str) -> ServiceResult<SpendVm> {
        let opt = self.require(id).await?;
        let b = self.ledger.spend_breakdown(&opt_id(id)).await?;
        let series = self.spend_series(&opt, 40).await?;
        Ok(spend_vm(&b, opt.budget_usd, series))
    }

    /// Cumulative spend, bucketed to at most `points` samples, in minutes since the optimization started.
    async fn spend_series(
        &self,
        opt: &Optimization,
        points: usize,
    ) -> ServiceResult<Vec<SpendPoint>> {
        let t0 = opt.started_at.unwrap_or(opt.created_at);
        let rows = sqlx::query(
            "SELECT at, usd FROM spend_events WHERE optimization_id = ?1 ORDER BY at, id",
        )
        .bind(opt.id.as_str())
        .fetch_all(self.ledger.pool())
        .await
        .map_err(db)?;
        let mut cum = 0.0;
        let series: Vec<SpendPoint> = rows
            .iter()
            .map(|r| {
                cum += r.get::<f64, _>("usd");
                SpendPoint {
                    t: ((r.get::<i64, _>("at") - t0) as f64 / MIN_MS as f64).max(0.0),
                    usd: cum,
                }
            })
            .collect();
        if series.len() <= points {
            return Ok(series);
        }
        Ok((0..points)
            .map(|i| {
                let idx = ((i * (series.len() - 1)) as f64 / (points - 1) as f64).round() as usize;
                series[idx].clone()
            })
            .collect())
    }

    /// USD per category of the live estimates only (`estimated = 1`), without the corrections reconciling wrote.
    pub async fn estimated_by_category(
        &self,
        id: &str,
    ) -> ServiceResult<Vec<(SpendCategory, f64)>> {
        let rows = sqlx::query(
            "SELECT category, SUM(usd) AS usd FROM spend_events WHERE optimization_id = ?1 AND estimated = 1 GROUP BY category",
        )
        .bind(id)
        .fetch_all(self.ledger.pool())
        .await
        .map_err(db)?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let c: String = r.get("category");
                Some((c.parse::<SpendCategory>().ok()?, r.get::<f64, _>("usd")))
            })
            .collect())
    }

    // ---- activity ----------------------------------------------------------------------------------------

    pub async fn activity(
        &self,
        limit: usize,
        optimization_id: Option<&str>,
    ) -> ServiceResult<Vec<ActivityVm>> {
        let units: BTreeMap<String, &'static str> = self
            .ledger
            .list_optimizations(None)
            .await?
            .iter()
            .map(|o| (o.id.to_string(), unit_for_metric(&o.metric)))
            .collect();
        let oid = optimization_id.map(opt_id);
        let items = self
            .ledger
            .recent_activity((limit * 2) as i64, oid.as_ref())
            .await?;
        // A verified verdict is shown through its ratchet move; drop the duplicate verdict row.
        let moved: Vec<&str> = items
            .iter()
            .filter(|i| matches!(i, ActivityItem::RatchetMove { .. }))
            .map(|i| i.candidate_id().as_str())
            .collect();
        let now = self.now();
        Ok(items
            .iter()
            .filter(|i| {
                !matches!(i, ActivityItem::Verdict { verdict: CandidateVerdict::Verified, .. }
                    if moved.contains(&i.candidate_id().as_str()))
            })
            .take(limit)
            .map(|i| {
                let unit = units
                    .get(i.optimization_id().as_str())
                    .copied()
                    .unwrap_or("");
                activity_vm(i, unit, now)
            })
            .collect())
    }

    // ---- agents ------------------------------------------------------------------------------------------

    /// Agent log page. No cursor: the newest `limit` events (oldest first); with a cursor: events newer than it.
    pub async fn agent_events(
        &self,
        agent_id: &str,
        cursor: Option<i64>,
        limit: u32,
    ) -> ServiceResult<AgentEventsPage> {
        let aid = AgentId::new(agent_id);
        if self.ledger.get_agent(&aid).await?.is_none() {
            return Err(ServiceError::not_found("agent", agent_id));
        }
        let limit = i64::from(limit.clamp(1, 1000));
        let events: Vec<AgentEventVm> = match cursor {
            Some(c) => self
                .ledger
                .agent_events(&aid, c, limit)
                .await?
                .into_iter()
                .map(|e| AgentEventVm {
                    id: e.id,
                    at: e.at,
                    event_type: e.event_type,
                    text: parse_payload(e.payload.as_ref()).text,
                })
                .collect(),
            None => {
                let rows = sqlx::query(
                    "SELECT * FROM (SELECT id, at, type, payload FROM agent_events WHERE agent_id = ?1 ORDER BY id DESC LIMIT ?2) ORDER BY id ASC",
                )
                .bind(agent_id)
                .bind(limit)
                .fetch_all(self.ledger.pool())
                .await
                .map_err(db)?;
                rows.iter()
                    .map(|r| AgentEventVm {
                        id: r.get("id"),
                        at: r.get("at"),
                        event_type: r.get("type"),
                        text: parse_payload_text(r.get("payload")).text,
                    })
                    .collect()
            }
        };
        let cursor = events.last().map(|e| e.id).or(cursor).unwrap_or(0);
        Ok(AgentEventsPage { events, cursor })
    }

    async fn latest_agent_message(&self, agent_id: &str) -> ServiceResult<Option<String>> {
        let rows = sqlx::query(
            "SELECT payload FROM agent_events WHERE agent_id = ?1 AND type = 'message' ORDER BY id DESC LIMIT 20",
        )
        .bind(agent_id)
        .fetch_all(self.ledger.pool())
        .await
        .map_err(db)?;
        for r in rows {
            let p = parse_payload_text(r.get("payload"));
            if p.from.as_deref() != Some("user") && !p.text.is_empty() {
                return Ok(Some(p.text));
            }
        }
        Ok(None)
    }

    async fn last_event_id(&self, agent_id: &str) -> ServiceResult<i64> {
        let r = sqlx::query("SELECT max(id) AS m FROM agent_events WHERE agent_id = ?1")
            .bind(agent_id)
            .fetch_one(self.ledger.pool())
            .await
            .map_err(db)?;
        Ok(r.get::<Option<i64>, _>("m").unwrap_or(0))
    }

    async fn agent_vm(
        &self,
        a: &Agent,
        cands: &[CandidateWithResults],
        spend_usd: f64,
    ) -> ServiceResult<AgentVm> {
        let mine: Vec<&CandidateWithResults> = cands
            .iter()
            .filter(|c| c.candidate.agent_id == a.id)
            .collect();
        let with_bench: Vec<&&CandidateWithResults> =
            mine.iter().filter(|c| c.bench_value.is_some()).collect();
        let recent = with_bench
            .iter()
            .skip(with_bench.len().saturating_sub(10))
            .map(|c| AgentRecentEval {
                value: c.bench_value.unwrap_or_default(),
                verdict: c.candidate.verdict,
                label: format!("{} {}", short_sha(&c.candidate.sha), c.candidate.verdict),
            })
            .collect();
        Ok(AgentVm {
            id: a.id.to_string(),
            kind: a.kind,
            branch: a.branch.clone(),
            status: a.status,
            hypothesis: self.latest_agent_message(a.id.as_str()).await?,
            recent,
            stats: AgentStats {
                candidates: mine.len() as i64,
                verified: mine.iter().filter(|c| c.moved_ratchet).count() as i64,
                rejected: mine
                    .iter()
                    .filter(|c| {
                        matches!(
                            c.candidate.verdict,
                            CandidateVerdict::Rejected | CandidateVerdict::FailedConfirm
                        )
                    })
                    .count() as i64,
                blocked: mine
                    .iter()
                    .filter(|c| c.candidate.verdict == CandidateVerdict::Blocked)
                    .count() as i64,
                spend_usd,
            },
            last_event_id: self.last_event_id(a.id.as_str()).await?,
            runner_instance_id: a.runner_instance_id.clone(),
        })
    }

    // ---- detail ------------------------------------------------------------------------------------------

    /// The detail screen. `receipt` and `calibration` come from the engine; the caller fills them in.
    pub async fn detail(&self, id: &str) -> ServiceResult<OptimizationDetail> {
        let oid = opt_id(id);
        let opt = self.require(id).await?;
        let summary = self
            .ledger
            .list_optimizations(None)
            .await?
            .into_iter()
            .find(|o| o.id == oid)
            .ok_or_else(|| ServiceError::not_found("optimization", id))?;
        let baseline = self.ledger.get_baseline(&oid).await?;
        let cands = self.ledger.candidates_with_results(&oid).await?;
        let spend = self.ledger.spend_breakdown(&oid).await?;
        let agent_spend: BTreeMap<Option<String>, f64> = spend
            .by_agent
            .iter()
            .map(|a| (a.agent_id.as_ref().map(|i| i.to_string()), a.total))
            .collect();
        let mut agents = Vec::new();
        for a in self.ledger.list_agents(&oid).await? {
            let s = agent_spend
                .get(&Some(a.id.to_string()))
                .copied()
                .unwrap_or(0.0);
            agents.push(self.agent_vm(&a, &cands, s).await?);
        }
        let last_move = self.ledger.ratchet_history(&oid).await?.pop();
        let best_stats = last_move.as_ref().and_then(|m| {
            cands
                .iter()
                .find(|c| c.candidate.id == m.candidate_id)
                .and_then(|c| {
                    c.results
                        .iter()
                        .rev()
                        .find(|r| r.phase == ratchet_core::BenchPhase::Bench)
                        .map(|r| stats_map(&r.stats))
                })
        });
        let base_stats = baseline
            .as_ref()
            .map(|b| stats_map(&b.stats))
            .unwrap_or_default();
        let now = self.now();
        let elapsed = opt.started_at.map_or(0.0, |s| {
            ((opt.ended_at.unwrap_or(now) - s) as f64 / 1000.0).max(0.0)
        });
        Ok(OptimizationDetail {
            summary: self.optimization_vm(&summary, &opt),
            base_ref: opt.base_ref.clone(),
            min_gain: opt.min_gain,
            time_limit_s: opt.time_limit_s,
            started_at: opt.started_at,
            ended_at: opt.ended_at,
            elapsed_s: elapsed,
            baseline_samples: baseline
                .as_ref()
                .map(|b| b.samples.clone())
                .unwrap_or_default(),
            baseline_stats: base_stats.clone(),
            noise_floor: baseline.as_ref().map(|b| b.noise_floor),
            agents,
            pipeline: build_pipeline(&cands),
            pr_url: (self.pr_url)(id),
            receipt: None,
            calibration: None,
            config_yaml: opt.config_yaml.clone(),
            metrics: if baseline.is_some() {
                metric_options(&opt.metric, &opt.config_yaml, &base_stats)
            } else {
                Vec::new()
            },
            compare: if baseline.is_some() {
                compare_rows(
                    &opt.metric,
                    opt.direction,
                    &opt.config_yaml,
                    &base_stats,
                    best_stats.as_ref(),
                )
            } else {
                Vec::new()
            },
            spans: span_rows(&base_stats, best_stats.as_ref()),
        })
    }

    // ---- winning change ----------------------------------------------------------------------------------

    /// The squashed result as the pull request will show it. `files` is the diff of the best verified commit
    /// against `base_ref` (`GitHost::diff`); `unavailable` explains a missing diff instead of inventing one.
    pub async fn winning_change(
        &self,
        id: &str,
        files: Option<Vec<FileDiff>>,
        unavailable: Option<String>,
    ) -> ServiceResult<WinningChangeVm> {
        let opt = self.require(id).await?;
        let moves = self.ledger.ratchet_history(&opt_id(id)).await?;
        let Some(best) = moves.last() else {
            return Ok(WinningChangeVm {
                commits: 0,
                sha: None,
                base_ref: opt.base_ref,
                files: vec![],
                additions: 0,
                deletions: 0,
                excerpt: None,
                unavailable: Some("No verified win yet.".into()),
            });
        };
        let commits = moves.len() as i64;
        let sha = Some(short_sha(&best.sha).to_owned());
        let Some(files) = files else {
            return Ok(WinningChangeVm {
                commits,
                sha,
                base_ref: opt.base_ref,
                files: vec![],
                additions: 0,
                deletions: 0,
                excerpt: None,
                unavailable: Some(unavailable.unwrap_or_else(|| "Diff unavailable.".into())),
            });
        };
        let top = files
            .iter()
            .filter(|f| !f.binary && !f.patch.is_empty())
            .max_by_key(|f| f.additions + f.deletions);
        Ok(WinningChangeVm {
            commits,
            sha,
            base_ref: opt.base_ref,
            additions: files.iter().map(|f| f.additions).sum(),
            deletions: files.iter().map(|f| f.deletions).sum(),
            excerpt: top.map(|f| DiffExcerpt {
                path: f.path.clone(),
                lines: hunk_excerpt(&f.patch),
            }),
            files: files
                .iter()
                .map(|f| DiffFileVm {
                    path: f.path.clone(),
                    additions: f.additions,
                    deletions: f.deletions,
                    binary: f.binary,
                })
                .collect(),
            unavailable: None,
        })
    }

    // ---- report ------------------------------------------------------------------------------------------

    pub async fn report(&self, id: &str) -> ServiceResult<ReportVm> {
        let d = self.detail(id).await?;
        let s = &d.summary;
        let series = self.series(id, None).await?;
        let cands = self.candidates(id).await?;
        let spend = self.spend(id).await?;
        let u = if s.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", s.unit)
        };
        let n = |v: Option<f64>| v.map_or("\u{2014}".to_owned(), |v| format!("{}{u}", fmt_num(v)));
        let wins: Vec<&CandidateVm> = cands.iter().filter(|c| c.moved_ratchet).collect();
        let mut lines: Vec<String> = vec![
            format!("# Ratchet report: {}", s.name),
            String::new(),
            format!("Repository: {} ({})  ", s.repo, d.base_ref),
            format!(
                "Metric: {} ({} is better)  ",
                s.metric,
                match series.direction {
                    Better::Lower => "lower",
                    Better::Higher => "higher",
                }
            ),
            format!("Status: {}", ui_status_str(s.status)),
            String::new(),
            "## Result".into(),
            String::new(),
            format!(
                "**{} \u{2192} {}**{} with {} verified win{} out of {} candidates.",
                n(s.baseline),
                n(s.best),
                s.gain_pct
                    .map_or(String::new(), |g| format!(" ({}% better)", to_fixed(g, 1))),
                s.verified_wins,
                if s.verified_wins == 1 { "" } else { "s" },
                s.candidate_count
            ),
            String::new(),
            "## Verified wins".into(),
            String::new(),
        ];
        if wins.is_empty() {
            lines.push("_None yet._".into());
        } else {
            for c in &wins {
                lines.push(format!(
                    "- `{}` ({}) {}: {}, hidden-trace {}",
                    c.sha,
                    c.agent,
                    c.message,
                    n(c.bench_value),
                    n(c.confirm_value)
                ));
            }
        }
        lines.extend([
            String::new(),
            "## Spend".into(),
            String::new(),
            format!(
                "Total ${} of ${} budget ({}).",
                to_fixed(spend.total, 2),
                to_fixed(spend.budget_usd, 2),
                if spend.estimated > 0.0 {
                    "includes estimates"
                } else {
                    "reconciled"
                }
            ),
            String::new(),
        ]);
        for c in spend.by_category.iter().filter(|c| c.usd > 0.0) {
            lines.push(format!("- {}: ${}", c.label, to_fixed(c.usd, 2)));
        }
        lines.extend([
            String::new(),
            "## All candidates".into(),
            String::new(),
            "| sha | agent | verdict | bench | vs baseline |".into(),
            "|---|---|---|---|---|".into(),
        ]);
        for c in &cands {
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                c.sha,
                c.agent,
                c.verdict,
                n(c.bench_value),
                c.vs_baseline_pct
                    .map_or("\u{2014}".into(), |p| format!("{}%", to_fixed(p, 1)))
            ));
        }
        lines.push(String::new());
        Ok(ReportVm {
            file_name: format!("ratchet-report-{id}.md"),
            markdown: lines.join("\n"),
        })
    }
}

fn db(e: sqlx::Error) -> ServiceError {
    ServiceError::Internal(format!("ledger query failed: {e}"))
}

pub fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

pub fn candidate_vm(
    c: &CandidateWithResults,
    t0: i64,
    baseline: Option<f64>,
    direction: Direction,
) -> CandidateVm {
    let bench = c
        .results
        .iter()
        .rev()
        .find(|r| r.phase == ratchet_core::BenchPhase::Bench);
    let vs = match (baseline, c.bench_value) {
        (Some(b), Some(v)) if b != 0.0 => {
            let delta = match direction {
                Direction::Minimize => b - v,
                Direction::Maximize => v - b,
            };
            Some((delta / b.abs()) * 100.0)
        }
        _ => None,
    };
    CandidateVm {
        id: c.candidate.id.to_string(),
        sha: short_sha(&c.candidate.sha).to_owned(),
        agent_id: c.candidate.agent_id.to_string(),
        agent: c.agent_kind,
        message: c.candidate.message.clone(),
        submitted_at: c.candidate.submitted_at,
        t: ((c.candidate.submitted_at - t0) as f64 / MIN_MS as f64).max(0.0),
        verdict: c.candidate.verdict,
        reasons: c.candidate.reasons.clone(),
        bench_value: c.bench_value,
        confirm_value: c.confirm_value,
        vs_baseline_pct: vs,
        moved_ratchet: c.moved_ratchet,
        samples: bench.map(|r| r.samples.clone()).unwrap_or_default(),
        stats: bench.map(|r| stats_map(&r.stats)).unwrap_or_default(),
    }
}

pub fn spend_vm(b: &SpendBreakdown, budget_usd: f64, series: Vec<SpendPoint>) -> SpendVm {
    let mut by_category: Vec<SpendLine> = SpendCategory::ALL
        .iter()
        .map(|c| SpendLine {
            id: c.as_str().to_owned(),
            label: category_label(*c).to_owned(),
            usd: b.by_category.get(*c),
        })
        .collect();
    by_category.sort_by(|a, b| {
        b.usd
            .partial_cmp(&a.usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    SpendVm {
        total: b.total,
        estimated: b.estimated,
        actual: b.actual,
        budget_usd,
        by_category,
        by_agent: b
            .by_agent
            .iter()
            .map(|a| SpendByAgent {
                agent_id: a.agent_id.as_ref().map(|i| i.to_string()),
                kind: a.kind,
                branch: a.branch.clone(),
                usd: a.total,
            })
            .collect(),
        series,
    }
}

fn activity_vm(a: &ActivityItem, unit: &str, now: i64) -> ActivityVm {
    match a {
        ActivityItem::RatchetMove {
            at,
            optimization_id,
            candidate_id,
            agent_kind,
            sha,
            message,
            from_value,
            to_value,
            ..
        } => ActivityVm {
            id: format!("m-{candidate_id}"),
            optimization_id: optimization_id.to_string(),
            agent: Some(*agent_kind),
            text: format!(
                "Verified win: {} \u{00b7} {} \u{2192} {}{}",
                if message.is_empty() {
                    short_sha(sha)
                } else {
                    message
                },
                fmt_num(*from_value),
                fmt_num(*to_value),
                if unit.is_empty() {
                    String::new()
                } else {
                    format!(" {unit}")
                }
            ),
            at: rel_time(*at, now),
            at_ms: *at,
            tone: ActivityTone::Mint,
        },
        ActivityItem::Verdict {
            at,
            optimization_id,
            candidate_id,
            agent_kind,
            sha,
            message,
            verdict,
            reasons,
            ..
        } => {
            let short = short_sha(sha);
            let why = reasons.first().map(String::as_str).unwrap_or("");
            let label = if message.is_empty() { short } else { message };
            let text = match verdict {
                CandidateVerdict::Verified => format!("Candidate verified: {label}"),
                CandidateVerdict::Rejected => {
                    format!(
                        "Candidate rejected: {}",
                        if why.is_empty() { label } else { why }
                    )
                }
                CandidateVerdict::FailedConfirm => {
                    format!("Failed confirm on the hidden trace: {label}")
                }
                CandidateVerdict::Blocked => {
                    format!("Blocked: {}", if why.is_empty() { label } else { why })
                }
                CandidateVerdict::Pending => format!("{verdict}: {short}"),
            };
            ActivityVm {
                id: format!("v-{candidate_id}"),
                optimization_id: optimization_id.to_string(),
                agent: Some(*agent_kind),
                text,
                at: rel_time(*at, now),
                at_ms: *at,
                tone: match verdict {
                    CandidateVerdict::Verified => ActivityTone::Mint,
                    CandidateVerdict::Blocked | CandidateVerdict::FailedConfirm => {
                        ActivityTone::Rose
                    }
                    _ => ActivityTone::Neutral,
                },
            }
        }
    }
}

/// First hunk of `patch` as typed lines (at most 14).
pub fn hunk_excerpt(patch: &str) -> Vec<DiffLine> {
    const EXCERPT_LINES: usize = 14;
    let lines: Vec<&str> = patch.split('\n').collect();
    let Some(start) = lines.iter().position(|l| l.starts_with("@@")) else {
        return Vec::new();
    };
    let mut out = vec![DiffLine {
        kind: DiffLineKind::Hunk,
        text: lines[start].to_owned(),
    }];
    for l in &lines[start + 1..] {
        if l.starts_with("@@") || out.len() >= EXCERPT_LINES {
            break;
        }
        if l.starts_with('\\') {
            continue; // "\ No newline at end of file"
        }
        let kind = if l.starts_with('+') {
            DiffLineKind::Add
        } else if l.starts_with('-') {
            DiffLineKind::Del
        } else {
            DiffLineKind::Ctx
        };
        out.push(DiffLine {
            kind,
            text: l.chars().skip(1).collect(),
        });
    }
    out
}

/// The bench pipeline, derived from the newest candidate.
pub fn build_pipeline(cands: &[CandidateWithResults]) -> BenchPipelineVm {
    let stage = |id, label: &str, state, detail: String| PipelineStage {
        id,
        label: label.to_owned(),
        state,
        detail,
    };
    let Some(latest) = cands.last() else {
        return BenchPipelineVm {
            candidate: None,
            queued: 0,
            stages: vec![
                stage(
                    StageId::Checkout,
                    "Check out sha",
                    StageState::Idle,
                    "waiting for a candidate".into(),
                ),
                stage(
                    StageId::Guardrails,
                    "Guardrails",
                    StageState::Idle,
                    String::new(),
                ),
                stage(
                    StageId::Bench,
                    "Bench \u{00d7}N",
                    StageState::Idle,
                    String::new(),
                ),
                stage(
                    StageId::Confirm,
                    "Confirm on hidden trace",
                    StageState::Idle,
                    String::new(),
                ),
                stage(StageId::Verdict, "Verdict", StageState::Idle, String::new()),
            ],
        };
    };
    use ratchet_core::BenchPhase;
    let bench = latest
        .results
        .iter()
        .rev()
        .find(|r| r.phase == BenchPhase::Bench);
    let confirm = latest
        .results
        .iter()
        .rev()
        .find(|r| r.phase == BenchPhase::Confirm);
    let verdict = latest.candidate.verdict;
    let pending = verdict == CandidateVerdict::Pending;
    let guard_failed = bench.is_some_and(|b| b.guardrails.iter().any(|g| !g.passed));
    let stages = vec![
        stage(
            StageId::Checkout,
            "Check out sha",
            StageState::Done,
            short_sha(&latest.candidate.sha).to_owned(),
        ),
        stage(
            StageId::Guardrails,
            "Guardrails",
            match bench {
                None if pending => StageState::Active,
                None => StageState::Idle,
                Some(_) if guard_failed => StageState::Failed,
                Some(_) => StageState::Done,
            },
            match bench {
                None => "running\u{2026}".into(),
                Some(b) if guard_failed => b
                    .guardrails
                    .iter()
                    .filter(|g| !g.passed)
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                Some(b) => format!("{} passed", b.guardrails.len()),
            },
        ),
        stage(
            StageId::Bench,
            &format!(
                "Bench \u{00d7}{}",
                bench.map_or("N".to_owned(), |b| b.samples.len().to_string())
            ),
            if bench.is_some() {
                StageState::Done
            } else if guard_failed {
                StageState::Idle
            } else if pending {
                StageState::Active
            } else {
                StageState::Idle
            },
            bench.map_or("queued".into(), |b| {
                format!("median {}", fmt_num(b.stats.value))
            }),
        ),
        stage(
            StageId::Confirm,
            "Confirm on hidden trace",
            match confirm {
                Some(_) if verdict == CandidateVerdict::FailedConfirm => StageState::Failed,
                Some(_) => StageState::Done,
                None if bench.is_some() && pending && !guard_failed => StageState::Active,
                None => StageState::Idle,
            },
            match confirm {
                Some(c) => format!("median {}", fmt_num(c.stats.value)),
                None if pending => "running\u{2026}".into(),
                None => "skipped".into(),
            },
        ),
        stage(
            StageId::Verdict,
            "Verdict",
            if pending {
                StageState::Active
            } else if verdict == CandidateVerdict::Verified {
                StageState::Done
            } else {
                StageState::Failed
            },
            if pending {
                "deciding\u{2026}".into()
            } else {
                verdict.to_string().replace('_', " ")
            },
        ),
    ];
    BenchPipelineVm {
        candidate: Some(PipelineCandidate {
            id: latest.candidate.id.to_string(),
            sha: short_sha(&latest.candidate.sha).to_owned(),
            agent: latest.agent_kind,
        }),
        queued: (cands
            .iter()
            .filter(|c| c.candidate.verdict == CandidateVerdict::Pending)
            .count() as i64
            - 1)
        .max(0),
        stages,
    }
}
