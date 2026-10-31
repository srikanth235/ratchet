//! The `GitHost` port: the git remote Ratchet reads candidates from and opens pull requests on.
//!
//! TODO(core): this trait belongs in `ratchet-core` next to the other ports (`GitHost` in `packages/core/src/ports`).
//! Implementations here: [`crate::LocalGitHost`] (simulation: a local bare repo), [`crate::GitHubGitHost`] (a GitHub
//! remote) and [`crate::MockGitHost`] (in-memory, scripted; the TS `FakeGit`).

use std::path::Path;

use async_trait::async_trait;

use crate::domain::{
    CandidateCommit, CommitFilesInput, FileDiff, PullRequestInput, PullRequestUrl,
};
use crate::error::{Error, Result};

#[async_trait]
pub trait GitHost: Send + Sync {
    /// Create `branch` at `from_ref` in the remote. Fails when the branch already exists.
    async fn create_branch(&self, from_ref: &str, branch: &str) -> Result<()>;

    /// Commits on `branch` carrying the `Ratchet-Candidate: true` trailer, oldest first, after `since_sha`.
    async fn list_candidates(
        &self,
        branch: &str,
        since_sha: Option<&str>,
    ) -> Result<Vec<CandidateCommit>>;

    async fn changed_files(&self, base_ref: &str, sha: &str) -> Result<Vec<String>>;

    /// Per-file diff of `sha` against its merge base with `base_ref` (what a PR from `sha` into `base_ref` would show).
    async fn diff(&self, base_ref: &str, sha: &str) -> Result<Vec<FileDiff>>;

    /// Materialize `sha` (code) into `dir`; used by the bench runner.
    async fn checkout(&self, sha: &str, dir: &Path) -> Result<()>;

    /// Squash the verified `input.shas` (ratchet order; last one's tree wins) onto `input.base` as branch `input.head`
    /// and open a PR. With no `shas`, `head` must already exist (see [`GitHost::commit_files`]).
    async fn open_pull_request(&self, input: &PullRequestInput) -> Result<PullRequestUrl>;

    /// Resolve a ref (branch, tag, sha) to a full commit sha.
    async fn resolve_ref(&self, reference: &str) -> Result<String>;

    fn remote_url(&self) -> String;

    /// Create `input.branch` in the remote as one new commit on top of `input.base_ref` that adds `input.files` (a
    /// template, say) and return that commit's sha. Fails when the branch already exists. To open the PR from the
    /// branch call [`GitHost::open_pull_request`] with no `shas`. Optional: the default says it is unsupported.
    async fn commit_files(&self, _input: &CommitFilesInput) -> Result<String> {
        Err(Error::Unsupported(
            "this git host cannot commit files".into(),
        ))
    }
}
