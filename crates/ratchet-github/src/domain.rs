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
    fn the_core_types_the_host_exchanges_keep_the_ts_json_shapes() {
        use ratchet_core::enums::{AgentKind, CandidateVerdict};
        use ratchet_core::ports::{CandidateCommit, CommitFile};
        use ratchet_core::{BenchStats, Direction, gain_percent};

        assert_eq!(
            serde_json::to_string(&AgentKind::ClaudeCode).unwrap(),
            "\"claude-code\""
        );
        assert_eq!(
            serde_json::to_string(&CandidateVerdict::FailedConfirm).unwrap(),
            "\"failed_confirm\""
        );
        let c = CandidateCommit {
            sha: "a".into(),
            branch: "b".into(),
            message: "m".into(),
            committed_at: 5,
        };
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            r#"{"sha":"a","branch":"b","message":"m","committedAt":5}"#
        );
        let f: CommitFile = serde_json::from_str(r#"{"path":"a","content":"b"}"#).unwrap();
        assert!(!f.is_executable());
        let stats = BenchStats::new(412.0).with("p50", 240.5);
        assert_eq!(
            serde_json::to_value(&stats).unwrap(),
            serde_json::json!({"value": 412.0, "p50": 240.5})
        );
        assert_eq!(gain_percent(Direction::Minimize, 100.0, 75.0), Some(25.0));
        assert_eq!(gain_percent(Direction::Minimize, 0.0, 1.0), None);
    }

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
