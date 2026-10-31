//! Types of this crate that have no home in `ratchet-core`.
//!
//! Everything the git host, the report and the squash plan exchange (`CandidateCommit`, `FileDiff`,
//! `PullRequestInput`, `BenchStats`, the ids and enums, ...) is `ratchet-core`'s. What is left is the shape of the
//! GitHub repository list the desktop app shows; core has no view-models, they live with the API layer (see PARITY.md).

use serde::{Deserialize, Serialize};

/// A GitHub repository as the app lists it. Same JSON as the TS `RepoVM`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// `owner/name`
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
    pub description: Option<String>,
    pub pushed_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_serializes_like_the_ts_repo_vm() {
        let r = Repo {
            full_name: "a/b".into(),
            default_branch: "main".into(),
            private: true,
            description: None,
            pushed_at: None,
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"fullName":"a/b","defaultBranch":"main","private":true,"description":null,"pushedAt":null}"#
        );
    }
}
