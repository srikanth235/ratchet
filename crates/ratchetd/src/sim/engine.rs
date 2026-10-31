//! The scripted run: baseline, agent turns, verdicts through the real ratchet rule, spend, events.
//! Port of `packages/desktop/src/main/dev-engine.ts`. Everything it simulates is written to the ledger through the
//! normal API, so the screens read exactly what the real engine will produce.

use std::sync::Arc;
use std::time::Duration;

use ratchet_core::bench::calibrate::calibration::BASELINE_REPEATS;
use ratchet_core::config::{Guardrail, GuardrailResult, guardrail_name};
use ratchet_core::enums::{
    AgentKind, AgentStatus, BenchPhase, CandidateVerdict, Direction, OptimizationStatus,
    SpendCategory, Verdict,
};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ledger::Patch;
use ratchet_core::protected::ProtectedViolation;
use ratchet_core::{
    NoiseFloorOptions, RatchetConfig, RatchetInput, evaluate_ratchet, noise_floor, parse_config,
    summarize,
};
use ratchet_ledger::{
    AppendAgentEventInput, RecordBaselineInput, RecordBenchResultInput, RecordSpendInput,
    SubmitCandidateInput,
};
use serde_json::json;

use super::data::{HYPOTHESES, PROTECTED_TOUCHES, simulated_stats};
use super::{Inner, Run};
use crate::api::{AgentEventVm, ApiEvent};
use crate::service::{ServiceError, ServiceResult};
use crate::views::format::round_to;

const DEFAULT_PLATEAU: u32 = 25;

impl Inner {
    fn oid(id: &str) -> OptimizationId {
        OptimizationId::new(id)
    }

    // ---- lifecycle ---------------------------------------------------------------------------------------

    pub(crate) async fn start(self: &Arc<Self>, id: &str) -> ServiceResult<()> {
        let oid = Self::oid(id);
        let opt = self
            .ledger
            .get_optimization(&oid)
            .await?
            .ok_or_else(|| ServiceError::not_found("optimization", id))?;
        if self.st().runs.contains_key(id) {
            return Ok(());
        }
        if opt.status.is_terminal() {
            return Err(ServiceError::Conflict(format!(
                "optimization {id} is {} and cannot be started again",
                opt.status
            )));
        }
        let config = parse_config(&opt.config_yaml)
            .map_err(|_| ServiceError::Internal("stored ratchet.yaml is invalid".into()))?;
        self.st().runs.insert(
            id.to_owned(),
            Run {
                config: config.clone(),
                no_eval: 0,
                turn: 0,
                task: None,
            },
        );
        self.ensure_sandbox(id).await?;

        if self.ledger.get_baseline(&oid).await?.is_some() {
            return self.begin_running(id).await;
        }
        self.ledger
            .set_optimization_status(&oid, OptimizationStatus::Baselining, None)
            .await?;
        for a in self.ledger.list_agents(&oid).await? {
            self.ledger
                .set_agent_status(
                    &a.id,
                    AgentStatus::Launching,
                    Patch::Set(format!("i-sim{}", tail(a.id.as_str(), 8))),
                )
                .await?;
        }
        self.emit(ApiEvent::Optimization {
            optimization_id: id.to_owned(),
            status: OptimizationStatus::Baselining,
        });
        self.publish_run_active().await;
        if self.opts.auto_run {
            let me = Arc::clone(self);
            let key = id.to_owned();
            let ms = self.opts.baseline_ms;
            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                if let Err(e) = me.finish_baseline(&key).await {
                    tracing::warn!(optimization = %key, "simulated baseline failed: {e}");
                }
            });
            if let Some(r) = self.st().runs.get_mut(id) {
                r.task = Some(handle);
            }
        } else {
            self.finish_baseline(id).await?;
        }
        Ok(())
    }

    async fn finish_baseline(self: &Arc<Self>, id: &str) -> ServiceResult<()> {
        if !self.st().runs.contains_key(id) {
            return Ok(()); // stopped meanwhile
        }
        self.record_baseline(id).await?;
        self.begin_running(id).await
    }

    pub(crate) async fn stop(self: &Arc<Self>, id: &str) -> ServiceResult<()> {
        let oid = Self::oid(id);
        if self.ledger.get_optimization(&oid).await?.is_none() {
            return Err(ServiceError::not_found("optimization", id));
        }
        self.halt(id);
        self.ledger
            .set_optimization_status(&oid, OptimizationStatus::Stopping, None)
            .await?;
        for a in self.ledger.list_agents(&oid).await? {
            self.ledger
                .set_agent_status(&a.id, AgentStatus::Stopped, Patch::Keep)
                .await?;
        }
        self.ledger
            .set_optimization_status(&oid, OptimizationStatus::Stopped, None)
            .await?;
        self.emit(ApiEvent::Optimization {
            optimization_id: id.to_owned(),
            status: OptimizationStatus::Stopped,
        });
        self.publish_run_active().await;
        self.after_run_ended(id).await;
        Ok(())
    }

    async fn record_baseline(&self, id: &str) -> ServiceResult<()> {
        let oid = Self::oid(id);
        let cfg = self.run_config(id)?;
        let maximize = cfg.objective.direction == Direction::Maximize;
        let (centre, n) = {
            let mut rng = self.rng();
            let centre = if maximize {
                40.0 + rng.next_f64() * 10.0
            } else {
                150.0 + rng.next_f64() * 60.0
            };
            let n = cfg.evaluator.repeats.unwrap_or(BASELINE_REPEATS).max(5) as usize;
            (centre, n)
        };
        let samples: Vec<f64> = {
            let mut rng = self.rng();
            (0..n)
                .map(|_| round_to(centre * (1.0 + (rng.next_f64() - 0.5) * 0.04), 2))
                .collect()
        };
        let s = summarize(&samples).map_err(|e| ServiceError::Internal(e.to_string()))?;
        let floor = noise_floor(&samples, NoiseFloorOptions::default())
            .map_err(|e| ServiceError::Internal(e.to_string()))?;
        let t = self.clock.now();
        self.ledger
            .record_baseline(RecordBaselineInput {
                optimization_id: oid.clone(),
                samples,
                noise_floor: floor,
                stats: simulated_stats(&cfg.objective.metric, s.median, &[("mean", s.mean)]),
                value: None,
                recorded_at: Some(t),
            })
            .await?;
        let mut spend =
            RecordSpendInput::new(oid, SpendCategory::BenchRunner, 0.62, "sim:baseline");
        spend.at = Some(t);
        self.ledger.record_spend(spend).await?;
        Ok(())
    }

    async fn begin_running(self: &Arc<Self>, id: &str) -> ServiceResult<()> {
        let oid = Self::oid(id);
        self.ledger
            .set_optimization_status(&oid, OptimizationStatus::Running, None)
            .await?;
        for a in self.ledger.list_agents(&oid).await? {
            if !matches!(a.status, AgentStatus::Running | AgentStatus::Paused) {
                let runner = a
                    .runner_instance_id
                    .clone()
                    .unwrap_or_else(|| format!("i-sim{}", tail(a.id.as_str(), 8)));
                self.ledger
                    .set_agent_status(&a.id, AgentStatus::Running, Patch::Set(runner))
                    .await?;
            }
            self.log_event(
                &a.id,
                "status",
                "Agent runner online; reading RATCHET.md",
                None,
            )
            .await?;
        }
        self.emit(ApiEvent::Optimization {
            optimization_id: id.to_owned(),
            status: OptimizationStatus::Running,
        });
        self.publish_run_active().await;
        if self.opts.auto_run {
            let me = Arc::clone(self);
            let key = id.to_owned();
            let every = Duration::from_millis(self.opts.tick_ms.max(1));
            let handle = tokio::spawn(async move {
                let mut timer = tokio::time::interval(every);
                timer.tick().await; // the first tick fires at once
                loop {
                    timer.tick().await;
                    if !me.st().runs.contains_key(&key) {
                        break;
                    }
                    if let Err(e) = me.tick(&key).await {
                        tracing::warn!(optimization = %key, "simulated turn failed: {e}");
                    }
                }
            });
            if let Some(r) = self.st().runs.get_mut(id)
                && let Some(old) = r.task.replace(handle)
            {
                old.abort();
            }
        }
        Ok(())
    }

    fn halt(&self, id: &str) {
        if let Some(run) = self.st().runs.remove(id)
            && let Some(t) = run.task
        {
            t.abort();
        }
    }

    pub(crate) fn run_config(&self, id: &str) -> ServiceResult<RatchetConfig> {
        self.st()
            .runs
            .get(id)
            .map(|r| r.config.clone())
            .ok_or_else(|| ServiceError::Conflict(format!("optimization {id} is not running")))
    }

    // ---- one turn ----------------------------------------------------------------------------------------

    /// Run one simulated agent turn for every running agent of `id` (called by the timer; public for tests).
    pub async fn tick(self: &Arc<Self>, id: &str) -> ServiceResult<()> {
        let oid = Self::oid(id);
        let Some(opt) = self.ledger.get_optimization(&oid).await? else {
            return Ok(());
        };
        if opt.status != OptimizationStatus::Running {
            return Ok(());
        }
        let no_eval = {
            let mut st = self.st();
            let Some(run) = st.runs.get_mut(id) else {
                return Ok(());
            };
            run.turn += 1;
            run.no_eval
        };
        let elapsed = self.clock.now() - opt.started_at.unwrap_or(opt.created_at);
        let spent = self.ledger.total_spend(&oid).await?;
        if spent >= opt.budget_usd {
            return self.finish(id, "budget exhausted").await;
        }
        if elapsed >= opt.time_limit_s * 1000 {
            return self.finish(id, "time limit reached").await;
        }
        let plateau = self
            .st()
            .plateau
            .get(id)
            .copied()
            .unwrap_or(DEFAULT_PLATEAU);
        if no_eval >= plateau {
            return self
                .finish(id, "plateau: no verified win in the last evaluations")
                .await;
        }
        for agent in self.ledger.list_agents(&oid).await? {
            if agent.status != AgentStatus::Running {
                continue;
            }
            self.agent_turn(id, &agent.id, agent.kind).await?;
            let status = self.ledger.get_optimization(&oid).await?.map(|o| o.status);
            if status != Some(OptimizationStatus::Running) {
                break;
            }
        }
        Ok(())
    }

    async fn agent_turn(&self, id: &str, agent_id: &AgentId, kind: AgentKind) -> ServiceResult<()> {
        let oid = Self::oid(id);
        let opt = self
            .ledger
            .get_optimization(&oid)
            .await?
            .ok_or_else(|| ServiceError::not_found("optimization", id))?;
        let cfg = self.run_config(id)?;
        let baseline = self
            .ledger
            .get_baseline(&oid)
            .await?
            .ok_or_else(|| ServiceError::Conflict("no baseline yet".into()))?;
        let best = self
            .ledger
            .current_best(&oid)
            .await?
            .unwrap_or(baseline.value);
        let dir = cfg.objective.direction;
        let sign = if dir == Direction::Minimize {
            -1.0
        } else {
            1.0
        };

        let queued = self
            .st()
            .inbox
            .get_mut(agent_id.as_str())
            .and_then(std::collections::VecDeque::pop_front);
        let hypothesis = match queued {
            Some(q) => format!("Following your note: {q}"),
            None => (*self.rng().pick(&HYPOTHESES)).to_owned(),
        };
        self.log_event(agent_id, "message", &hypothesis, None)
            .await?;
        self.log_event(agent_id, "tool", "edit src/search/index.ts", None)
            .await?;

        let sha = self.rng().hex(40);
        self.log_event(
            agent_id,
            "commit",
            &format!("Ratchet-Candidate: {}", &sha[..7]),
            Some(json!({"sha": sha})),
        )
        .await?;
        let t0 = self.clock.now();
        let mut sub = SubmitCandidateInput::new(oid.clone(), agent_id.clone(), sha.clone());
        sub.message = Some(hypothesis.clone());
        sub.submitted_at = Some(t0);
        let cand = self.ledger.submit_candidate(sub).await?;

        // Effect: mostly small, sometimes a real win, sometimes a regression.
        let (effect, repeats, samples, guardrails, touched) = {
            let mut rng = self.rng();
            let r = rng.next_f64();
            let effect = if r < 0.22 {
                0.04 + rng.next_f64() * 0.05
            } else if r < 0.75 {
                (rng.next_f64() - 0.4) * 0.03
            } else {
                -0.02 - rng.next_f64() * 0.03
            };
            let centre = best * (1.0 + sign * effect);
            let repeats = cfg.evaluator.repeats.unwrap_or(BASELINE_REPEATS) as usize;
            let samples: Vec<f64> = (0..repeats)
                .map(|_| round_to(centre * (1.0 + (rng.next_f64() - 0.5) * 0.03), 2))
                .collect();
            let guardrails: Vec<GuardrailResult> = cfg
                .guardrails
                .iter()
                .map(|g| {
                    let name = match g {
                        Guardrail::Command(c) => c.run.clone(),
                        other => guardrail_name(other),
                    };
                    if rng.next_f64() < 0.04 {
                        GuardrailResult {
                            name,
                            passed: false,
                            detail: Some("regressed".into()),
                        }
                    } else {
                        GuardrailResult {
                            name,
                            passed: true,
                            detail: None,
                        }
                    }
                })
                .collect();
            let touched = if rng.next_f64() < 0.04 {
                Some((*rng.pick(&PROTECTED_TOUCHES)).to_owned())
            } else {
                None
            };
            (effect, repeats, samples, guardrails, touched)
        };
        let centre = best * (1.0 + sign * effect);
        let protected_violations: Vec<ProtectedViolation> = touched
            .into_iter()
            .map(|path| ProtectedViolation {
                pattern: if path.starts_with("bench") {
                    "bench/**".into()
                } else {
                    "tests/contract/**".into()
                },
                path,
            })
            .collect();
        let stats_of = |xs: &[f64]| {
            let s = summarize(xs).ok();
            simulated_stats(
                &cfg.objective.metric,
                s.as_ref().map_or(0.0, |s| s.median),
                &[("mean", s.as_ref().map_or(0.0, |s| s.mean))],
            )
        };

        let mut input = RatchetInput {
            direction: dir,
            min_gain: opt.min_gain,
            noise_floor: baseline.noise_floor,
            current_best: best,
            candidate: samples.clone(),
            confirm: None,
            confirm_best: None,
            guardrails: guardrails.clone(),
            protected_violations,
        };
        let bench0 = self.clock.now();
        let mut outcome =
            evaluate_ratchet(&input).map_err(|e| ServiceError::Internal(e.to_string()))?;
        self.ledger
            .record_bench_result(RecordBenchResultInput {
                candidate_id: cand.id.clone(),
                phase: BenchPhase::Bench,
                samples: samples.clone(),
                stats: stats_of(&samples),
                guardrails,
                started_at: bench0,
                finished_at: self.clock.now(),
            })
            .await?;
        let (bench_usd, tokens_usd, runner_usd, exp_usd, egress) = {
            let mut rng = self.rng();
            (
                0.18 + rng.next_f64() * 0.12,
                0.25 + rng.next_f64() * 0.55,
                0.06 + rng.next_f64() * 0.04,
                rng.next_f64() * 0.1,
                rng.next_f64() < 0.2,
            )
        };
        self.spend(
            &oid,
            agent_id,
            SpendCategory::BenchRunner,
            bench_usd,
            "sim:bench",
        )
        .await?;
        self.spend(
            &oid,
            agent_id,
            SpendCategory::Tokens,
            tokens_usd,
            &format!("sim:{kind}"),
        )
        .await?;
        self.spend(
            &oid,
            agent_id,
            SpendCategory::AgentRunner,
            runner_usd,
            "sim:runner",
        )
        .await?;
        self.spend(
            &oid,
            agent_id,
            SpendCategory::Experiment,
            exp_usd,
            "sim:experiment",
        )
        .await?;
        if egress {
            self.spend(&oid, agent_id, SpendCategory::StorageEgress, 0.01, "sim:s3")
                .await?;
        }

        if outcome.verdict == Verdict::PendingConfirm {
            // Hidden-trace confirm: usually holds, occasionally does not.
            let confirm: Vec<f64> = {
                let mut rng = self.rng();
                let holds = rng.next_f64() > 0.15;
                let c = if holds {
                    centre
                } else {
                    best * (1.0 + (-sign) * 0.01)
                };
                (0..repeats)
                    .map(|_| round_to(c * (1.0 + (rng.next_f64() - 0.5) * 0.03), 2))
                    .collect()
            };
            let c0 = self.clock.now();
            input.confirm = Some(confirm.clone());
            outcome =
                evaluate_ratchet(&input).map_err(|e| ServiceError::Internal(e.to_string()))?;
            self.ledger
                .record_bench_result(RecordBenchResultInput {
                    candidate_id: cand.id.clone(),
                    phase: BenchPhase::Confirm,
                    samples: confirm.clone(),
                    stats: stats_of(&confirm),
                    guardrails: vec![],
                    started_at: c0,
                    finished_at: self.clock.now(),
                })
                .await?;
            self.spend(
                &oid,
                agent_id,
                SpendCategory::BenchRunner,
                0.2,
                "sim:confirm",
            )
            .await?;
        }

        let verdict = match outcome.verdict {
            Verdict::PendingConfirm => CandidateVerdict::Rejected,
            v => v.to_candidate_verdict(),
        };
        self.ledger
            .set_verdict(&cand.id, verdict, &outcome.reasons, None)
            .await?;
        self.emit(ApiEvent::Candidate {
            optimization_id: id.to_owned(),
            candidate_id: cand.id.to_string(),
            verdict,
        });
        if verdict == CandidateVerdict::Verified {
            self.ledger
                .move_ratchet(&oid, &cand.id, outcome.candidate_value, None)
                .await?;
            self.emit(ApiEvent::Ratchet {
                optimization_id: id.to_owned(),
                from: best,
                to: outcome.candidate_value,
            });
            if let Some(r) = self.st().runs.get_mut(id) {
                r.no_eval = 0;
            }
        } else if let Some(r) = self.st().runs.get_mut(id) {
            r.no_eval += 1;
        }
        self.log_event(
            agent_id,
            "verdict",
            &format!(
                "{verdict}: {}",
                outcome.reasons.first().map(String::as_str).unwrap_or("")
            ),
            Some(
                json!({"sha": sha, "verdict": verdict.as_str(), "value": outcome.candidate_value}),
            ),
        )
        .await?;
        self.emit(ApiEvent::Spend {
            optimization_id: id.to_owned(),
            total_usd: self.ledger.total_spend(&oid).await?,
        });
        Ok(())
    }

    async fn finish(self: &Arc<Self>, id: &str, why: &str) -> ServiceResult<()> {
        let oid = Self::oid(id);
        self.halt(id);
        for a in self.ledger.list_agents(&oid).await? {
            self.ledger
                .set_agent_status(&a.id, AgentStatus::Done, Patch::Keep)
                .await?;
            self.log_event(&a.id, "status", &format!("Stopped: {why}"), None)
                .await?;
        }
        self.ledger
            .set_optimization_status(&oid, OptimizationStatus::Completed, None)
            .await?;
        self.emit(ApiEvent::Optimization {
            optimization_id: id.to_owned(),
            status: OptimizationStatus::Completed,
        });
        self.publish_run_active().await;
        self.after_run_ended(id).await;
        Ok(())
    }

    async fn spend(
        &self,
        oid: &OptimizationId,
        agent_id: &AgentId,
        category: SpendCategory,
        usd: f64,
        source: &str,
    ) -> ServiceResult<()> {
        let mut s = RecordSpendInput::new(oid.clone(), category, round_to(usd, 4), source);
        s.agent_id = Some(agent_id.clone());
        s.at = Some(self.clock.now());
        self.ledger.record_spend(s).await?;
        Ok(())
    }

    /// Append to the agent's log and publish it.
    pub(crate) async fn log_event(
        &self,
        agent_id: &AgentId,
        event_type: &str,
        text: &str,
        data: Option<serde_json::Value>,
    ) -> ServiceResult<()> {
        let agent = self
            .ledger
            .get_agent(agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.as_str()))?;
        let at = self.clock.now();
        let payload = match data {
            Some(d) => json!({"text": text, "data": d}),
            None => json!({"text": text}),
        };
        let rec = self
            .ledger
            .append_agent_event(AppendAgentEventInput {
                agent_id: agent_id.clone(),
                event_type: event_type.to_owned(),
                payload: Some(payload),
                at: Some(at),
            })
            .await?;
        self.emit(ApiEvent::AgentEvent {
            optimization_id: agent.optimization_id.to_string(),
            agent_id: agent_id.to_string(),
            event: Some(AgentEventVm {
                id: rec.id,
                at,
                event_type: event_type.to_owned(),
                text: text.to_owned(),
            }),
        });
        Ok(())
    }

    pub(crate) fn halt_all(&self) {
        let runs: Vec<Run> = self.st().runs.drain().map(|(_, r)| r).collect();
        for r in runs {
            if let Some(t) = r.task {
                t.abort();
            }
        }
    }
}

fn tail(s: &str, n: usize) -> &str {
    &s[s.len().saturating_sub(n)..]
}
