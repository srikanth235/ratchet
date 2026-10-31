//! The reaper: enforces the limits IAM cannot express (`max_instance_lifetime_hours`, `max_instances_per_agent`,
//! terminate-on-shutdown) and cleans up after ended optimizations and stopped agents. (TS `reaper.ts`)
//!
//! Safety: stateless and idempotent. It reads the cloud, terminates, and a second run finds nothing left. It only ever
//! touches instances carrying the `ratchet:opt` tag (what `Compute::list_tagged` returns), skips instances already
//! `stopping`/`terminated`, and a terminate failure is reported, never panicked on or retried in a loop.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::{AgentId, Clock, InstanceInfo, InstanceRole, InstanceState, OptimizationId};
use crate::error::Result;
use crate::ports::Compute;
use crate::util::SystemClock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReapReason {
    OptimizationEnded,
    AgentStopped,
    MaxLifetime,
    StoppedInstance,
    OverQuota,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReapedInstance {
    pub instance_id: String,
    pub optimization_id: OptimizationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    pub role: InstanceRole,
    pub reason: ReapReason,
    pub age_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverQuotaAgent {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub running: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaperReport {
    pub at: i64,
    pub scanned: usize,
    pub terminated: Vec<ReapedInstance>,
    /// Agents that exceeded max_instances_per_agent (candidates for the kill switch on repeat offence).
    pub over_quota_agents: Vec<OverQuotaAgent>,
    pub errors: Vec<String>,
}

/// What the reaper asks the engine: which optimizations and agents are over.
#[async_trait]
pub trait ReaperPolicy: Send + Sync {
    async fn is_optimization_ended(&self, optimization_id: &str) -> bool;
    async fn is_agent_stopped(&self, optimization_id: &str, agent_id: &str) -> bool;
}

pub struct ReaperOptions {
    pub compute: Arc<dyn Compute>,
    pub policy: Arc<dyn ReaperPolicy>,
    pub clock: Arc<dyn Clock>,
    /// `limits.max_instance_lifetime_hours` from the sandbox, in hours.
    pub max_instance_lifetime_hours: f64,
    /// `limits.max_instances_per_agent`; `None` skips quota enforcement.
    pub max_instances_per_agent: Option<usize>,
}

impl ReaperOptions {
    pub fn new(
        compute: Arc<dyn Compute>,
        policy: Arc<dyn ReaperPolicy>,
        max_instance_lifetime_hours: f64,
    ) -> Self {
        Self {
            compute,
            policy,
            clock: Arc::new(SystemClock),
            max_instance_lifetime_hours,
            max_instances_per_agent: None,
        }
    }

    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn max_instances_per_agent(mut self, n: usize) -> Self {
        self.max_instances_per_agent = Some(n);
        self
    }
}

/// Terminates tagged instances that are past max lifetime, belong to a stopped agent, or outlived their optimization.
pub struct Reaper {
    opts: ReaperOptions,
}

fn is_live(s: InstanceState) -> bool {
    matches!(
        s,
        InstanceState::Pending | InstanceState::Running | InstanceState::Stopped
    )
}

impl Reaper {
    pub fn new(opts: ReaperOptions) -> Self {
        Self { opts }
    }

    pub async fn reap(&self, optimization_id: Option<&str>) -> Result<ReaperReport> {
        let now = self.opts.clock.now_ms();
        let max_age = self.opts.max_instance_lifetime_hours * 3_600_000.0;
        let all = self.opts.compute.list_tagged(optimization_id).await?;
        // 'stopping'/'terminated' are already on their way out: skipping keeps reruns idempotent.
        let candidates: Vec<&InstanceInfo> = all.iter().filter(|i| is_live(i.state)).collect();
        let mut report = ReaperReport {
            at: now,
            scanned: all.len(),
            terminated: vec![],
            over_quota_agents: vec![],
            errors: vec![],
        };

        let mut opt_ended: HashMap<String, bool> = HashMap::new();
        let mut agent_stopped: HashMap<(String, String), bool> = HashMap::new();
        let mut doomed: HashMap<String, ReapReason> = HashMap::new();

        for i in &candidates {
            if !opt_ended.contains_key(&i.optimization_id) {
                let ended = self
                    .opts
                    .policy
                    .is_optimization_ended(&i.optimization_id)
                    .await;
                opt_ended.insert(i.optimization_id.clone(), ended);
            }
            if opt_ended[&i.optimization_id] {
                doomed.insert(i.id.clone(), ReapReason::OptimizationEnded);
                continue;
            }
            if let Some(agent) = &i.agent_id {
                let k = (i.optimization_id.clone(), agent.clone());
                if !agent_stopped.contains_key(&k) {
                    let stopped = self
                        .opts
                        .policy
                        .is_agent_stopped(&i.optimization_id, agent)
                        .await;
                    agent_stopped.insert(k.clone(), stopped);
                }
                if agent_stopped[&k] {
                    doomed.insert(i.id.clone(), ReapReason::AgentStopped);
                    continue;
                }
            }
            if (now - i.launched_at) as f64 > max_age {
                doomed.insert(i.id.clone(), ReapReason::MaxLifetime);
            } else if i.state == InstanceState::Stopped {
                doomed.insert(i.id.clone(), ReapReason::StoppedInstance);
            }
        }

        if let Some(quota) = self.opts.max_instances_per_agent {
            // (optimization, agent) groups in first-seen order, like a JS Map.
            let mut order: Vec<(String, String)> = vec![];
            let mut by_agent: HashMap<(String, String), Vec<&InstanceInfo>> = HashMap::new();
            for i in &candidates {
                let Some(agent) = &i.agent_id else { continue };
                if i.role != InstanceRole::Experiment
                    || doomed.contains_key(&i.id)
                    || i.state == InstanceState::Stopped
                {
                    continue;
                }
                let k = (i.optimization_id.clone(), agent.clone());
                if !by_agent.contains_key(&k) {
                    order.push(k.clone());
                }
                by_agent.entry(k).or_default().push(i);
            }
            for k in order {
                let list = &by_agent[&k];
                if list.len() <= quota {
                    continue;
                }
                report.over_quota_agents.push(OverQuotaAgent {
                    optimization_id: k.0.clone(),
                    agent_id: k.1.clone(),
                    running: list.len(),
                });
                // Keep the oldest `quota` instances, terminate the newest excess.
                let mut sorted = list.clone();
                sorted.sort_by_key(|i| i.launched_at);
                for i in sorted.into_iter().skip(quota) {
                    doomed.insert(i.id.clone(), ReapReason::OverQuota);
                }
            }
        }

        let targets: Vec<&InstanceInfo> = candidates
            .iter()
            .copied()
            .filter(|i| doomed.contains_key(&i.id))
            .collect();
        if !targets.is_empty() {
            let ids: Vec<String> = targets.iter().map(|i| i.id.clone()).collect();
            match self.opts.compute.terminate(&ids).await {
                Ok(()) => {
                    for i in targets {
                        tracing::info!(instance = %i.id, reason = ?doomed[&i.id], "reaper terminated instance");
                        report.terminated.push(ReapedInstance {
                            instance_id: i.id.clone(),
                            optimization_id: i.optimization_id.clone(),
                            agent_id: i.agent_id.clone(),
                            role: i.role,
                            reason: doomed[&i.id],
                            age_ms: now - i.launched_at,
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "reaper terminate failed");
                    report.errors.push(format!("terminate failed: {e}"));
                }
            }
        }
        Ok(report)
    }
}
