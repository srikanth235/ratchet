//! Typed identifiers. All of them are opaque strings on the wire (`#[serde(transparent)]`).
//!
//! Generated ids look like the TypeScript ones: `<prefix>_<16 hex chars>` (`opt_3f9a...`).

use std::borrow::Borrow;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        #[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Prefix of generated ids.
            pub const PREFIX: &'static str = $prefix;

            /// Wrap an existing id (no validation: ids are opaque).
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// A fresh random id: `<prefix>_<16 hex chars>`.
            pub fn generate() -> Self {
                Self(new_id($prefix))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl From<&$name> for $name {
            fn from(s: &$name) -> Self {
                s.clone()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                &self.0 == other
            }
        }
    };
}

id_newtype!(
    /// Id of an optimization (`opt_...`).
    OptimizationId,
    "opt"
);
id_newtype!(
    /// Id of an agent (`agt_...`).
    AgentId,
    "agt"
);
id_newtype!(
    /// Id of a candidate (`cand_...`).
    CandidateId,
    "cand"
);
id_newtype!(
    /// Id of a sandbox (free-form, e.g. `sbx-1`).
    SandboxId,
    "sbx"
);

/// `<prefix>_<first 16 hex chars of a v4 uuid>`.
pub fn new_id(prefix: &str) -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &u[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_have_prefix_and_16_hex() {
        let id = OptimizationId::generate();
        let (p, rest) = id.as_str().split_once('_').unwrap();
        assert_eq!(p, "opt");
        assert_eq!(rest.len(), 16);
        assert!(rest.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(OptimizationId::generate(), OptimizationId::generate());
    }

    #[test]
    fn ids_are_plain_strings_in_json() {
        let id = AgentId::new("agt_1");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"agt_1\"");
        let back: AgentId = serde_json::from_str("\"agt_1\"").unwrap();
        assert_eq!(back, id);
        assert_eq!(id, "agt_1");
    }
}
