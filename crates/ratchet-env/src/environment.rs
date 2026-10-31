//! Where an optimization's environment module lives, from its config alone (`environment.ts`).

use ratchet_core::config::{DEFAULT_ENVIRONMENT_INFRA_DIR, RatchetConfig};
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::ports::SandboxEnvironmentKind;
use ratchet_core::protected::normalize_repo_path;
use sha2::{Digest, Sha256};

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
    /// `Custom`: `infra:` in ratchet.yaml, a standalone module that replaces the standard one (must satisfy every
    /// contract output). `Drafted`: the approved drafted environment under `infra/ratchet/`, built on the base module.
    pub kind: SandboxEnvironmentKind,
    /// Repo-relative directory.
    pub path: String,
    /// False when the scope alone does not say there is a module (`single`): the directory may be absent, and then
    /// there is none.
    pub required: bool,
}

/// Where an optimization's environment module lives, from its config alone; `None` for scope `none` / no scope (the
/// standard sandbox, nothing extra). The engine then reads the directory from the protected ref.
pub fn environment_module_of(config: &EnvironmentConfigInput) -> Option<EnvironmentModuleRef> {
    if let Some(infra) = config.infra.as_deref().filter(|i| !i.is_empty()) {
        return Some(EnvironmentModuleRef {
            kind: SandboxEnvironmentKind::Custom,
            path: normalize_repo_path(infra),
            required: true,
        });
    }
    match config.scope {
        Some(EnvironmentScope::Full) => Some(EnvironmentModuleRef {
            kind: SandboxEnvironmentKind::Drafted,
            path: DEFAULT_ENVIRONMENT_INFRA_DIR.to_owned(),
            required: true,
        }),
        Some(EnvironmentScope::Single) => Some(EnvironmentModuleRef {
            kind: SandboxEnvironmentKind::Drafted,
            path: DEFAULT_ENVIRONMENT_INFRA_DIR.to_owned(),
            required: false,
        }),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Order-independent content hash of an environment's files (path + bytes), 10 hex characters.
pub fn hash_environment_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut files: Vec<(&str, &str)> = files.into_iter().collect();
    // JavaScript compares strings by UTF-16 code unit
    files.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    let mut h = Sha256::new();
    for (path, content) in files {
        h.update(format!("{path}\0{}\0", content.len()).as_bytes());
        h.update(content.as_bytes());
        h.update(b"\0");
    }
    hex(&h.finalize())[..10].to_owned()
}

/// Sandbox id of an environment sandbox. Two optimizations share a sandbox only when region AND environment are the
/// same; `[a-z0-9-]`, 2-32 chars (`ratchet-env-` + 8 hex = 20).
pub fn environment_sandbox_id(region: &str, hash: &str) -> String {
    let d = Sha256::digest(format!("{region}\0{hash}").as_bytes());
    format!("ratchet-env-{}", &hex(&d)[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EnvironmentCheck;

    fn cfg(infra: Option<&str>, scope: Option<EnvironmentScope>) -> EnvironmentConfigInput {
        EnvironmentConfigInput {
            infra: infra.map(str::to_owned),
            scope,
        }
    }

    // "none / unset: the standard sandbox, no module"
    #[test]
    fn none_or_unset_has_no_module() {
        assert_eq!(environment_module_of(&cfg(None, None)), None);
        assert_eq!(
            environment_module_of(&cfg(None, Some(EnvironmentScope::None))),
            None
        );
    }

    // "full: the drafted module, required; single: the drafted module if there is one"
    #[test]
    fn full_is_required_single_is_optional() {
        let d = |required| EnvironmentModuleRef {
            kind: SandboxEnvironmentKind::Drafted,
            path: "infra/ratchet".into(),
            required,
        };
        assert_eq!(
            environment_module_of(&cfg(None, Some(EnvironmentScope::Full))),
            Some(d(true))
        );
        assert_eq!(
            environment_module_of(&cfg(None, Some(EnvironmentScope::Single))),
            Some(d(false))
        );
    }

    // "infra: is the advanced override and wins"
    #[test]
    fn infra_wins() {
        assert_eq!(
            environment_module_of(&cfg(Some("./ops//sandbox"), Some(EnvironmentScope::Full))),
            Some(EnvironmentModuleRef {
                kind: SandboxEnvironmentKind::Custom,
                path: "ops/sandbox".into(),
                required: true
            })
        );
    }

    // "is stable, order independent and sensitive to paths and bytes"
    #[test]
    fn hash_is_stable_order_independent_and_sensitive() {
        let a = [("main.tf", "a"), ("x.sh", "b")];
        let h = hash_environment_files(a);
        assert_eq!(h.len(), 10);
        assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(h, hash_environment_files([a[1], a[0]]));
        assert_ne!(h, hash_environment_files([("main.tf", "a2"), a[1]]));
        assert_ne!(h, hash_environment_files([("main2.tf", "a"), a[1]]));
        assert_ne!(
            hash_environment_files([("ab", "c")]),
            hash_environment_files([("a", "bc")])
        );
    }

    // "the sandbox id is a valid, region- and environment-specific id"
    #[test]
    fn sandbox_id_is_region_and_environment_specific() {
        let id = environment_sandbox_id("us-east-1", "abc1234567");
        assert!(id.starts_with("ratchet-env-") && id.len() == 20);
        assert!(
            id["ratchet-env-".len()..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        assert_ne!(environment_sandbox_id("eu-west-1", "abc1234567"), id);
        assert_ne!(environment_sandbox_id("us-east-1", "abc1234568"), id);
        assert_eq!(environment_sandbox_id("us-east-1", "abc1234567"), id);
    }

    // "needs checks and all of them ok"
    #[test]
    fn draft_passed_needs_checks_and_all_ok() {
        let c = |ok| EnvironmentCheck {
            id: "a".into(),
            label: "a".into(),
            ok,
            detail: String::new(),
        };
        assert!(!crate::types::checks_passed(&[]));
        assert!(crate::types::checks_passed(&[c(true)]));
        assert!(!crate::types::checks_passed(&[c(true), c(false)]));
    }
}
