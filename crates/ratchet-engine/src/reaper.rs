//! Terminate tagged instances that should not exist. Port of `packages/engine/src/reaper.ts`.

use std::collections::HashMap;

use ratchet_core::enums::{AgentStatus, InstanceRole, OptimizationStatus};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::{InstanceInfo, InstanceState};

use crate::deps::Deps;
use crate::errors::EngineResult;
use crate::kv::k;

fn ended(s: OptimizationStatus) -> bool {
    matches!(
        s,
        OptimizationStatus::Stopped | OptimizationStatus::Completed | OptimizationStatus::Failed
    )
}

fn dead_agent(s: AgentStatus) -> bool {
    matches!(
        s,
        AgentStatus::Stopped | AgentStatus::Done | AgentStatus::Failed
    )
}

/// Terminate tagged instances that should not exist: anything of an ended optimization, anything of a stopped /
/// finished agent, experiments and bench instances past their max lifetime, and agent runners past the time limit
/// plus a grace period. Returns the ids it asked to terminate.
pub async fn reap(
    d: &Deps,
    optimization_id: &str,
    instances: Option<Vec<InstanceInfo>>,
) -> EngineResult<Vec<String>> {
    let opt_id = OptimizationId::new(optimization_id);
    let Some(opt) = d.ledger.get_optimization(&opt_id).await? else {
        return Ok(Vec::new());
    };
    let all = match instances {
        Some(l) => l,
        None => d.compute.list_tagged(Some(&opt_id)).await?,
    };
    let live: Vec<InstanceInfo> = all
        .into_iter()
        .filter(|i| i.optimization_id == opt_id && i.state != InstanceState::Terminated)
        .collect();
    if live.is_empty() {
        return Ok(Vec::new());
    }

    let now = d.clock.now();
    let agents: HashMap<String, _> = d
        .ledger
        .list_agents(&opt_id)
        .await?
        .into_iter()
        .map(|a| (a.id.to_string(), a))
        .collect();
    let deadline = d.kv.get_i64(&k::deadline(optimization_id)).await?;
    let mut doomed: Vec<String> = Vec::new();

    for i in &live {
        let age = now - i.launched_at;
        let agent = i.agent_id.as_ref().and_then(|a| agents.get(a.as_str()));
        let mut kill = ended(opt.status);
        if !kill && agent.is_some_and(|a| dead_agent(a.status)) {
            kill = true;
        }
        if !kill
            && i.role == InstanceRole::Experiment
            && age > d.settings.reaper.max_experiment_lifetime_ms
        {
            kill = true;
        }
        if !kill && i.role == InstanceRole::Bench && age > d.settings.reaper.max_bench_lifetime_ms {
            kill = true;
        }
        if !kill && i.role == InstanceRole::AgentRunner {
            let limit = deadline.unwrap_or(opt.created_at + opt.time_limit_s * 1000)
                + d.settings.reaper.runner_grace_ms;
            if now > limit {
                kill = true;
            }
        }
        if kill {
            doomed.push(i.id.clone());
        }
    }
    if !doomed.is_empty() {
        d.compute.terminate(&doomed).await?;
    }
    Ok(doomed)
}
