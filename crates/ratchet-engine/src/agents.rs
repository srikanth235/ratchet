//! Which agent CLIs an optimization uses, in which order (docs/AGENT_HARNESS_PLAN.md section 4.6).
//!
//! The ordered list and its fallback rules live in the key-value store next to the optimization's other settings
//! (`opt:<id>:agents_policy`). An optional `agents:` section in `ratchet.yaml` overrides it. Without either, the list is
//! the optimization's own agents in the order they were created, so an optimization made before this setting existed keeps
//! running exactly the agents it has.
//!
//! The effective policy goes into every `RunnerConfig` at launch. A runner that is already running keeps the chain it
//! started with; a new policy applies from the next launch of an agent.

use ratchet_core::AgentsPolicy;
use ratchet_core::ports::RunnerMessage;

use crate::engine::{Engine, handle, oid};
use crate::errors::{EngineError, EngineResult, assert_valid_id};
use crate::kv::k;

/// The policy in force and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveAgents {
    pub policy: AgentsPolicy,
    /// `ratchet.yaml` sets it; the ledger's setting is ignored until that section is removed.
    pub from_config: bool,
}

impl Engine {
    /// The harness list and fallback rules of an optimization.
    pub async fn agents_policy(&self, id: &str) -> EngineResult<EffectiveAgents> {
        assert_valid_id("optimization id", id)?;
        self.require_opt(id).await?;
        let config = self.config(id).await?;
        if let Some(p) = &config.agents {
            return Ok(EffectiveAgents {
                policy: p.clone(),
                from_config: true,
            });
        }
        if let Some(p) = self
            .kv
            .get_json::<AgentsPolicy>(&k::agents_policy(id))
            .await?
        {
            return Ok(EffectiveAgents {
                policy: p,
                from_config: false,
            });
        }
        let kinds = self
            .ledger
            .list_agents(&oid(id))
            .await?
            .into_iter()
            .map(|a| a.kind);
        Ok(EffectiveAgents {
            policy: AgentsPolicy::new(kinds),
            from_config: false,
        })
    }

    /// Replace the optimization's harness list and rules. Refused while `ratchet.yaml` has an `agents:` section (the file
    /// wins; change it there).
    pub async fn set_agents_policy(&self, id: &str, policy: AgentsPolicy) -> EngineResult<()> {
        if let Some(problem) = policy.problem() {
            return Err(EngineError::invalid_input(problem));
        }
        let current = self.agents_policy(id).await?;
        if current.from_config {
            return Err(EngineError::invalid_state(
                "ratchet.yaml sets `agents:` for this optimization; change it there",
            ));
        }
        self.kv.set_json(&k::agents_policy(id), &policy).await
    }

    /// The agent's `submit_candidate` tool call, on its way to the runner loop that owns the branch.
    pub async fn submit_candidate(
        &self,
        agent_id: &str,
        sha: &str,
        note: Option<&str>,
    ) -> EngineResult<()> {
        assert_valid_id("agent id", agent_id)?;
        let sha = sha.trim();
        if sha.len() < 7 || sha.len() > 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(EngineError::invalid_input(
                "sha must be a commit sha (7 to 64 hex digits)",
            ));
        }
        let agent = self.require_agent(agent_id).await?;
        self.runners_for(agent.optimization_id.as_str())
            .await
            .send(
                &handle(&agent),
                &RunnerMessage::Submit {
                    sha: sha.to_owned(),
                    note: note
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_owned),
                },
            )
            .await?;
        Ok(())
    }
}
