//! The validator every drafted environment goes through (`validate.ts`). STUB.

use std::collections::HashMap;

use ratchet_core::enums::EnvironmentScope;

use crate::cost::MachineSpec;
use crate::types::{EnvironmentCheck, EnvironmentCost, EnvironmentFile, EnvironmentResource};

pub const SANDBOX_BASE_MODULE_SOURCE: &str = "./ratchet-sandbox";
pub const SANDBOX_BASE_MODULE_NAME: &str = "sandbox";
pub const ENVIRONMENT_OBJECT_PREFIX: &str = "env/";
pub const RATCHET_GENERATED_FILES: [&str; 2] = ["ratchet_inputs.tf", "ratchet_outputs.tf"];
pub const ENVIRONMENT_CHECK_IDS: [&str; 13] = [
    "isolated",
    "sealed",
    "same-hardware",
    "teardown",
    "files",
    "syntax",
    "config",
    "scope",
    "base-module",
    "local-access",
    "tagged",
    "cost",
    "plan",
];
pub const MAX_ENVIRONMENT_MACHINES: usize = 8;
pub const MAX_ENVIRONMENT_FILE_BYTES: usize = 256 * 1024;
pub const MAX_ENVIRONMENT_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_VOLUME_GB: f64 = 1000.0;
pub const MACHINE_KEYS: [&str; 7] = [
    "role",
    "instance_type",
    "label",
    "detail",
    "user_data",
    "root_volume_gb",
    "data_volume_gb",
];

#[derive(Debug, Clone)]
pub struct ValidateEnvironmentInput {
    pub scope: EnvironmentScope,
    /// Everything the draft would commit: `ratchet.yaml` (or `config_path`) and the files under `infra/ratchet/`.
    pub files: Vec<EnvironmentFile>,
    /// ratchet.yaml as it is on the base ref: proves the draft changed `environment:` and nothing else.
    pub original_config_yaml: Option<String>,
    pub config_path: Option<String>,
    /// The run's time limit for the cost estimate (default 2 h).
    pub time_limit_seconds: Option<f64>,
    /// The files are the environment module only: what the engine and the sandbox provider re-check when they APPLY
    /// an approved environment. The `config` check is then skipped.
    pub module_only: bool,
    pub prices: Option<HashMap<String, f64>>,
    /// Pinned bench machine for `none` / `single` without OpenTofu (default c7i.large).
    pub hardware: Option<String>,
}

impl ValidateEnvironmentInput {
    pub fn new(scope: EnvironmentScope, files: Vec<EnvironmentFile>) -> Self {
        Self {
            scope,
            files,
            original_config_yaml: None,
            config_path: None,
            time_limit_seconds: None,
            module_only: false,
            prices: None,
            hardware: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentValidation {
    pub ok: bool,
    pub checks: Vec<EnvironmentCheck>,
    pub machines: Vec<MachineSpec>,
    pub resources: Vec<EnvironmentResource>,
    pub cost: EnvironmentCost,
    /// Flat list of every failed check's problems.
    pub problems: Vec<String>,
}

pub fn validate_environment(_input: &ValidateEnvironmentInput) -> EnvironmentValidation {
    todo!()
}
