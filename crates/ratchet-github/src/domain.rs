//! Domain types this crate shares with the rest of Ratchet.
//!
//! TODO(core): these are minimal local copies of types that belong in `ratchet-core` (`ports` and `ledger` in the TS
//! `@ratchet/core`). Their serde shapes are the TS JSON shapes (camelCase fields, lowercase/kebab-case enums), so they
//! can be swapped for the core types without touching callers.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};

use crate::numfmt::js_number;

// TODO(core)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Minimize,
    Maximize,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Minimize => "minimize",
            Direction::Maximize => "maximize",
        }
    }
}

// TODO(core)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "mock")]
    Mock,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Mock => "mock",
        }
    }
}

// TODO(core)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateVerdict {
    Pending,
    Verified,
    Rejected,
    FailedConfirm,
    Blocked,
}

/// Benchmark summary statistics in insertion order (`value` is the headline number). Serializes integral values
/// without a fractional part, like a JS number.
// TODO(core)
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(transparent)]
pub struct BenchStats(pub IndexMap<String, f64>);

impl BenchStats {
    pub fn new(pairs: impl IntoIterator<Item = (&'static str, f64)>) -> Self {
        BenchStats(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<f64> {
        self.0.get(key).copied()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }
}

impl Serialize for BenchStats {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, &js_number(*v))?;
        }
        m.end()
    }
}

/// Percent improvement of `to` over `from`; positive = better. `None` if `from` is 0.
// TODO(core)
pub fn gain_percent(direction: Direction, from: f64, to: f64) -> Option<f64> {
    if from == 0.0 {
        return None;
    }
    let delta = match direction {
        Direction::Minimize => from - to,
        Direction::Maximize => to - from,
    };
    Some((delta / from.abs()) * 100.0)
}

/// A commit on a candidate branch carrying the `Ratchet-Candidate: true` trailer.
// TODO(core)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateCommit {
    pub sha: String,
    pub branch: String,
    pub message: String,
    /// Commit time, milliseconds since the epoch.
    pub committed_at: i64,
}

/// One file of a diff between two commits (renames are reported as a delete plus an add).
// TODO(core)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
    /// True for binary files: no line counts, no patch.
    pub binary: bool,
    /// This file's unified diff (`@@` hunks only); empty for binary files.
    pub patch: String,
}

/// A file for `commit_files`. `path` is repo-relative POSIX; `executable` sets mode 100755.
// TODO(core)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFile {
    pub path: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
}

impl CommitFile {
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        CommitFile {
            path: path.into(),
            content: content.into(),
            executable: false,
        }
    }

    pub fn executable(mut self) -> Self {
        self.executable = true;
        self
    }
}

// TODO(core)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFilesInput {
    /// Branch (or sha) the new commit is parented on and whose tree it extends.
    pub base_ref: String,
    /// The branch to create. It must not exist yet.
    pub branch: String,
    pub message: String,
    /// Written over the base tree (added, or replaced when the path exists).
    pub files: Vec<CommitFile>,
}

/// A GitHub repository as the app lists it. Same JSON as the TS `RepoVM`.
// TODO(core)
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

/// Input for `GitHost::open_pull_request`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestInput {
    pub base: String,
    /// Branch to open the PR from, e.g. `ratchet/<opt>/result`.
    pub head: String,
    pub title: String,
    pub body: String,
    /// Verified SHAs in ratchet order. When non-empty, `head` is created as one squashed commit whose tree is the
    /// final SHA's tree (later wins are rebased onto earlier ones). When empty, `head` must already exist.
    #[serde(default)]
    pub shas: Vec<String>,
    /// Squash commit message; defaults to `title`.
    #[serde(default)]
    pub commit_message: Option<String>,
}

/// Result of opening (or refreshing) a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestUrl {
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_shapes_match_ts_json() {
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
        let f: CommitFile = serde_json::from_str(r#"{"path":"a","content":"b"}"#).unwrap();
        assert!(!f.executable);
        let s = BenchStats::new([("value", 412.0), ("p50", 240.5)]);
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"value":412,"p50":240.5}"#
        );
    }

    #[test]
    fn gain_percent_is_direction_aware() {
        assert_eq!(gain_percent(Direction::Minimize, 100.0, 75.0), Some(25.0));
        assert_eq!(gain_percent(Direction::Maximize, 100.0, 110.0), Some(10.0));
        assert_eq!(gain_percent(Direction::Minimize, 0.0, 1.0), None);
    }
}
