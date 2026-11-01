//! The parsed `ratchet.yaml`: [`RatchetConfig`] and its parts.
//!
//! Field names follow the YAML file (snake_case), as the zod-parsed TypeScript object did. Build one from
//! YAML text with [`parse_config`](super::parse_config); deserializing JSON into these types does not
//! validate.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::guardrails::Guardrail;
use crate::enums::{DepsMode, Direction, EnvironmentScope, EvaluatorKind, ValueBasis, ValuePeriod};

/// Bench machine used when neither `evaluator.hardware`, `environment.machine` nor a template names one.
pub const DEFAULT_BENCH_MACHINE: &str = "c7i.large";
/// Protected ref holding the trusted evaluator when `evaluator.ref` is omitted.
pub const DEFAULT_EVALUATOR_REF: &str = "ratchet/bench";
/// The agents' brief when `agent.instructions` is not set.
pub const DEFAULT_AGENT_INSTRUCTIONS: &str = "RATCHET.md";

/// How a metric change converts to money (the business case): `usd_per_unit` dollars per one unit of
/// improvement in `objective.metric`, per `period`. `basis: estimated` until it has been measured in production.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ValueModel {
    pub usd_per_unit: f64,
    pub period: ValuePeriod,
    pub basis: ValueBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `objective:` what to improve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Objective {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Key in the evaluator's JSON output, as a dotted path (`http_req_duration.p95`).
    pub metric: String,
    pub direction: Direction,
    /// Fraction of the best that a win must beat. Absent: calibrated from the baseline noise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_gain: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ValueModel>,
}

/// `evaluator:` (legacy alias `bench:`), fully resolved: template defaults are filled in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Evaluator {
    /// The template's kind, else `benchmark`.
    pub kind: EvaluatorKind,
    /// Protected ref holding the trusted evaluator. Default `ratchet/bench`.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The evaluation command (from the template when `run` was absent).
    pub run: String,
    /// Id of an evaluator template, when one was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Runs per candidate. Absent: calibrated from the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeats: Option<u64>,
    /// The k of the noise floor `k x stddev(baseline)`. Absent: 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_factor: Option<f64>,
    /// Pinned bench hardware: `environment.machine`, else the template's machine, else `c7i.large`.
    pub hardware: String,
    pub deps: DepsMode,
    /// Sealed holdout the agents never see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holdout: Option<String>,
    /// Deprecated alias of `holdout`; always equal to it when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_trace: Option<String>,
}

/// Where a drafted (setup-agent written, user approved) environment lives in the repo. Protected once
/// `environment.scope` is set.
pub const DEFAULT_ENVIRONMENT_INFRA_DIR: &str = "infra/ratchet";

/// `environment:` what the repo needs; how it is isolated stays with Ratchet. Every key is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Environment {
    /// How much infrastructure the task needs, decided by the setup agent and approved by the user. `full` (and
    /// `single` with OpenTofu files) means a drafted module under `infra/ratchet/` that the engine applies as the
    /// sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<EnvironmentScope>,
    /// Dockerfile or devcontainer.json in the repo (a protected path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Bench instance type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// AWS region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Extra egress hosts for agents / the dependency job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<Vec<String>>,
}

/// `agent:` how agents are briefed and what they may launch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AgentConfig {
    /// The brief; default `RATCHET.md`.
    pub instructions: String,
    /// Instance types agents may launch; default: the bench machine.
    pub allowed_instance_types: Vec<String>,
}

/// A validated `ratchet.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct RatchetConfig {
    /// Always 0, the only supported version.
    pub version: u8,
    /// Template category (`perf`, `forecasting`, `llm`). Informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    pub objective: Objective,
    pub evaluator: Evaluator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    /// Advanced: path of a custom OpenTofu module. Read only from the protected ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infra: Option<String>,
    /// The template's defaults when `guardrails:` was absent.
    #[serde(default)]
    pub guardrails: Vec<Guardrail>,
    /// The user's own additions to the automatically protected set.
    #[serde(default)]
    pub protected: Vec<String>,
    pub agent: AgentConfig,
}

impl RatchetConfig {
    /// The evaluator under its legacy name (`bench`).
    pub fn bench(&self) -> &Evaluator {
        &self.evaluator
    }

    pub fn direction(&self) -> Direction {
        self.objective.direction
    }
}
