//! Setting the `environment:` keys of a ratchet.yaml text (`config-yaml.ts`). STUB.

use ratchet_core::enums::EnvironmentScope;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentConfigPatch {
    pub scope: EnvironmentScope,
    pub machine: Option<String>,
    pub image: Option<String>,
    pub region: Option<String>,
}

impl EnvironmentConfigPatch {
    pub fn scope(scope: EnvironmentScope) -> Self {
        Self {
            scope,
            machine: None,
            image: None,
            region: None,
        }
    }
}

/// Set the `environment:` keys of a ratchet.yaml text and nothing else (comments, key order and every other block
/// are kept). `Err` carries the message for text that is not valid YAML.
pub fn apply_environment_to_config_yaml(
    _text: &str,
    _patch: &EnvironmentConfigPatch,
) -> Result<String, String> {
    todo!()
}

/// `environment.scope` of a ratchet.yaml text, or `None` (also for text that is not valid YAML).
pub fn read_environment_scope(_text: &str) -> Option<EnvironmentScope> {
    todo!()
}
