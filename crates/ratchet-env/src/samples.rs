//! Deterministic example environments, one per scope (`samples.ts`). STUB.

use ratchet_core::enums::EnvironmentScope;

use crate::types::EnvironmentFile;

pub const MOCK_CONFIG_YAML: &str = "";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleEnvironment {
    pub scope: EnvironmentScope,
    pub reason: String,
    pub files: Vec<EnvironmentFile>,
}

/// What the setup agent would say for each scope.
pub fn sample_reason(_scope: EnvironmentScope) -> &'static str {
    todo!()
}

/// Per-scope simulated drafting cost: `(duration_s, usd_spent)`.
pub fn sample_effort(_scope: EnvironmentScope) -> (u64, f64) {
    todo!()
}

#[derive(Debug, Clone, Default)]
pub struct SampleOptions {
    pub original_config_yaml: Option<String>,
    pub config_path: Option<String>,
    pub reason: Option<String>,
}

pub fn sample_environment(_scope: EnvironmentScope, _opts: &SampleOptions) -> SampleEnvironment {
    todo!()
}
