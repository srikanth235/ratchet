//! Dynamic environments (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"): the environment an optimization is
//! measured in is sized to the task by a setup agent BEFORE the run, validated against the sandbox contract, costed,
//! and locked once the user approves it.
//!
//! Port of `packages/core/src/environment/*` and `packages/engine/src/environment/*` (see `PARITY.md`). The crate
//! depends on `ratchet-core` only: the setup agent is an [`ratchet_core::ports::AgentAdapter`] handed in by a
//! factory (the `ratchet-agents` local adapters in the daemon), the git host is a [`ratchet_core::ports::GitHost`],
//! and `tofu plan` is an [`EnvironmentChecker`] implemented by `ratchet-sandbox`. That keeps `ratchet-sandbox`
//! (which re-validates an approved environment when it applies it) free to depend on this crate.

pub mod approve;
pub mod brief;
pub mod config_yaml;
pub mod cost;
pub mod drafter;
pub mod environment;
pub mod error;
pub mod hcl;
pub mod mock;
pub mod samples;
pub mod tree;
pub mod types;
pub mod validate;

pub use approve::{
    CommitDraftOptions, EnvironmentPullRequest, build_environment_pull_request, commit_draft,
};
pub use brief::{
    ANSWER_MARKER, AgentAnswer, MAIN_TF_SKELETON, SetupBriefInput, build_revise_brief,
    build_setup_brief, format_agent_answer, parse_agent_answer,
};
pub use config_yaml::{
    EnvironmentConfigPatch, apply_environment_to_config_yaml, read_environment_scope,
};
pub use cost::{
    BASE_STARTUP_MINUTES, CostEstimate, CostEstimateOptions, GP3_USD_PER_GB_MONTH,
    INSTANCE_USD_PER_HOUR, MachineSpec, STARTUP_MINUTES_PER_MACHINE, default_machines,
    estimate_environment_cost_live, estimate_machines_cost, static_hourly_usd, volume_usd_per_hour,
};

pub use drafter::{
    AdapterFactory, AgentEnvironmentDrafter, AgentEnvironmentDrafterOptions, DEFAULT_TOKEN_PRICING,
    ErrorHook, GitFactory, TokenPricing, fixed_git,
};
pub use environment::{
    EnvironmentConfigInput, EnvironmentModuleRef, environment_module_of, environment_sandbox_id,
    hash_environment_files,
};
pub use error::{EnvError, EnvErrorKind, EnvResult};

pub use mock::{
    MockEnvironmentDrafter, MockEnvironmentDrafterOptions, ScopeChoice, scope_from_goal,
};
pub use samples::{
    MOCK_CONFIG_YAML, SampleEnvironment, SampleOptions, sample_effort, sample_environment,
    sample_reason,
};
pub use tree::{TreeError, read_environment, read_environment_tree};
pub use types::*;
pub use validate::{
    ENVIRONMENT_CHECK_IDS, ENVIRONMENT_OBJECT_PREFIX, EnvironmentValidation, MACHINE_KEYS,
    MAX_ENVIRONMENT_FILE_BYTES, MAX_ENVIRONMENT_MACHINES, MAX_ENVIRONMENT_TOTAL_BYTES,
    MAX_VOLUME_GB, RATCHET_GENERATED_FILES, SANDBOX_BASE_MODULE_NAME, SANDBOX_BASE_MODULE_SOURCE,
    ValidateEnvironmentInput, validate_environment,
};
