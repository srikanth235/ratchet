//! `ratchet.yaml`: parsing, validation, guardrails.

pub mod guardrails;
pub mod load;
pub mod parse;
pub mod schema;

pub use guardrails::{
    CommandGuardrail, Comparator, Guardrail, GuardrailResult, ThresholdGuardrail,
    evaluate_command_result, evaluate_threshold, guardrail_name, parse_threshold,
};
pub use load::{ConfigResult, format_config_error, load_config, parse_config};
pub use parse::{ConfigError, PathSegment, format_path};
pub use schema::{
    AgentConfig, DEFAULT_AGENT_INSTRUCTIONS, DEFAULT_BENCH_MACHINE, DEFAULT_EVALUATOR_REF,
    Environment, Evaluator, Objective, RatchetConfig, ValueModel,
};
