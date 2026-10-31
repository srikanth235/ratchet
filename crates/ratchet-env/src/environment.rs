//! Where an optimization's environment module lives, from its config alone (`environment.ts`). STUB.

use ratchet_core::config::RatchetConfig;
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::ports::SandboxEnvironmentKind;

/// The slice of a parsed `ratchet.yaml` that decides which environment module (if any) the sandbox is built from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentConfigInput {
    pub infra: Option<String>,
    pub scope: Option<EnvironmentScope>,
}

impl From<&RatchetConfig> for EnvironmentConfigInput {
    fn from(c: &RatchetConfig) -> Self {
        Self {
            infra: c.infra.clone(),
            scope: c.environment.as_ref().and_then(|e| e.scope),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentModuleRef {
    /// `Custom`: `infra:` in ratchet.yaml. `Drafted`: the approved drafted environment under `infra/ratchet/`.
    pub kind: SandboxEnvironmentKind,
    /// Repo-relative directory.
    pub path: String,
    /// False when the scope alone does not say there is a module (`single`).
    pub required: bool,
}

pub fn environment_module_of(_config: &EnvironmentConfigInput) -> Option<EnvironmentModuleRef> {
    todo!()
}

/// Order-independent content hash of an environment's files (path + bytes), 10 hex characters.
pub fn hash_environment_files<'a>(_files: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    todo!()
}

/// Sandbox id of an environment sandbox (`ratchet-env-` + 8 hex).
pub fn environment_sandbox_id(_region: &str, _hash: &str) -> String {
    todo!()
}
