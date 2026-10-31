//! An in-memory, scripted `GitHost` (the TS `FakeGit` from `packages/engine/src/testing/fakes.ts`).
//!
//! Tests and simulation script the commits a "runner" pushed; nothing touches disk except `checkout`, which writes the
//! configured project files. For a host that behaves like real git, use [`crate::LocalGitHost`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use indexmap::IndexMap;

use ratchet_core::ports::mock::ManualClock;
use ratchet_core::ports::{
    CandidateCommit, Clock, CommitFilesInput, CommitRef, FileDiff, GitHost, PortError, PortResult,
    PullRequestInput, PullRequestRef,
};

/// A shared, ordered log of calls (`git.createBranch:<branch>`, `git.openPullRequest`, …).
pub type CallLog = Arc<Mutex<Vec<String>>>;

#[derive(Debug, Clone)]
pub struct MockCommit {
    pub commit: CandidateCommit,
    pub files: Vec<String>,
}

#[derive(Default)]
pub struct MockGitState {
    /// branch -> from_ref
    pub branches: HashMap<String, String>,
    pub commits: IndexMap<String, Vec<MockCommit>>,
    pub prs: Vec<PullRequestInput>,
    pub fail_changed_files: HashSet<String>,
    /// Project files `checkout` writes (path -> content).
    pub files: BTreeMap<String, String>,
    /// Branches that already exist in the remote (`commit_files` refuses them).
    pub existing_branches: HashSet<String>,
    pub files_committed: Vec<(CommitFilesInput, String)>,
    pub fail_commit_files: bool,
    pub resolved: HashMap<String, String>,
}

pub struct MockGitHost {
    pub log: CallLog,
    clock: Arc<dyn Clock>,
    state: Mutex<MockGitState>,
}

impl MockGitHost {
    pub fn new(files: BTreeMap<String, String>) -> Self {
        Self::with(
            Arc::default(),
            Arc::new(ManualClock::new(1_700_000_000_000)),
            files,
        )
    }

    /// With a shared call log and the clock that stamps `committed_at`.
    pub fn with(log: CallLog, clock: Arc<dyn Clock>, files: BTreeMap<String, String>) -> Self {
        MockGitHost {
            log,
            clock,
            state: Mutex::new(MockGitState {
                files,
                ..Default::default()
            }),
        }
    }

    pub fn state<R>(&self, f: impl FnOnce(&mut MockGitState) -> R) -> R {
        f(&mut self.state.lock().expect("mock git state"))
    }

    fn log(&self, line: impl Into<String>) {
        self.log.lock().expect("call log").push(line.into());
    }

    /// Script a candidate commit on `branch` (what the runner would push).
    pub fn push(&self, branch: &str, sha: &str, changed: &[&str], message: Option<&str>) {
        let committed_at = self.clock.now();
        let message = message
            .map(String::from)
            .unwrap_or_else(|| format!("perf: {sha}\n\nRatchet-Candidate: true"));
        self.state(|s| {
            s.commits
                .entry(branch.to_string())
                .or_default()
                .push(MockCommit {
                    commit: CandidateCommit {
                        sha: sha.into(),
                        branch: branch.into(),
                        message,
                        committed_at,
                    },
                    files: changed.iter().map(|f| f.to_string()).collect(),
                });
        });
    }

    pub fn calls(&self) -> Vec<String> {
        self.log.lock().expect("call log").clone()
    }
}

#[async_trait]
impl GitHost for MockGitHost {
    async fn create_branch(&self, from_ref: &str, branch: &str) -> PortResult<()> {
        self.log(format!("git.createBranch:{branch}"));
        self.state(|s| s.branches.insert(branch.to_string(), from_ref.to_string()));
        Ok(())
    }

    async fn list_candidates(
        &self,
        branch: &str,
        since_sha: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>> {
        Ok(self.state(|s| {
            let list = s.commits.get(branch).cloned().unwrap_or_default();
            let start = since_sha
                .and_then(|sha| list.iter().position(|c| c.commit.sha == sha))
                .map_or(0, |i| i + 1);
            list.into_iter().skip(start).map(|c| c.commit).collect()
        }))
    }

    async fn changed_files(&self, _base_ref: &str, sha: &str) -> PortResult<Vec<String>> {
        self.state(|s| {
            if s.fail_changed_files.contains(sha) {
                return Err(PortError::failed(format!("cannot diff {sha}")));
            }
            s.commits
                .values()
                .flatten()
                .find(|c| c.commit.sha == sha)
                .map(|c| c.files.clone())
                .ok_or_else(|| PortError::failed(format!("unknown sha {sha}")))
        })
    }

    async fn diff(&self, base_ref: &str, sha: &str) -> PortResult<Vec<FileDiff>> {
        Ok(self
            .changed_files(base_ref, sha)
            .await?
            .into_iter()
            .map(|path| FileDiff {
                path,
                additions: 1,
                deletions: 0,
                binary: false,
                patch: "@@ -0,0 +1 @@\n+changed".into(),
            })
            .collect())
    }

    async fn checkout(&self, _sha: &str, dir: &Path) -> PortResult<()> {
        let files = self.state(|s| s.files.clone());
        for (path, body) in files {
            let full = dir.join(path);
            if let Some(parent) = full.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(full, body).await?;
        }
        Ok(())
    }

    async fn open_pull_request(&self, input: &PullRequestInput) -> PortResult<PullRequestRef> {
        self.log("git.openPullRequest");
        let n = self.state(|s| {
            s.prs.push(input.clone());
            s.prs.len()
        });
        Ok(PullRequestRef {
            url: format!("https://example.test/pr/{n}"),
        })
    }

    async fn resolve_ref(&self, reference: &str) -> PortResult<String> {
        Ok(self
            .state(|s| s.resolved.get(reference).cloned())
            .unwrap_or_else(|| format!("{reference}-sha")))
    }

    fn remote_url(&self) -> String {
        "https://example.test/acme/repo.git".into()
    }

    async fn commit_files(&self, input: &CommitFilesInput) -> PortResult<CommitRef> {
        self.log(format!("git.commitFiles:{}", input.branch));
        self.state(|s| {
            if s.fail_commit_files || (!input.update && s.existing_branches.contains(&input.branch))
            {
                return Err(PortError::failed(format!(
                    "branch {} already exists",
                    input.branch
                )));
            }
            if input.update && !s.existing_branches.contains(&input.branch) {
                return Err(PortError::failed(format!(
                    "commitFiles: branch {} does not exist",
                    input.branch
                )));
            }
            s.existing_branches.insert(input.branch.clone());
            let sha = format!("files-{}", s.files_committed.len() + 1);
            s.files_committed.push((input.clone(), sha.clone()));
            Ok(CommitRef { sha })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratchet_core::ports::CommitFile;

    fn host() -> MockGitHost {
        MockGitHost::new(BTreeMap::from([("src/a.txt".to_string(), "a".to_string())]))
    }

    // Behaviour of FakeGit that engine tests rely on (no dedicated TS test file).
    #[tokio::test]
    async fn lists_scripted_candidates_after_a_sha() {
        let h = host();
        h.push("b", "s1", &["x"], None);
        h.push("b", "s2", &["y"], Some("m"));
        let all = h.list_candidates("b", None).await.unwrap();
        assert_eq!(
            all.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(),
            ["s1", "s2"]
        );
        assert!(all[0].message.contains("Ratchet-Candidate: true"));
        assert_eq!(h.list_candidates("b", Some("s1")).await.unwrap().len(), 1);
        assert!(h.list_candidates("b", Some("s2")).await.unwrap().is_empty());
        assert!(h.list_candidates("nope", None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn changed_files_diff_and_failures() {
        let h = host();
        h.push("b", "s1", &["x", "y"], None);
        assert_eq!(h.changed_files("main", "s1").await.unwrap(), ["x", "y"]);
        assert_eq!(h.diff("main", "s1").await.unwrap().len(), 2);
        assert!(
            h.changed_files("main", "zzz")
                .await
                .unwrap_err()
                .to_string()
                .contains("unknown sha zzz")
        );
        h.state(|s| s.fail_changed_files.insert("s1".into()));
        assert!(
            h.changed_files("main", "s1")
                .await
                .unwrap_err()
                .to_string()
                .contains("cannot diff s1")
        );
    }

    #[tokio::test]
    async fn records_branches_prs_and_a_call_log() {
        let h = host();
        h.create_branch("main", "ratchet/o/claude").await.unwrap();
        let pr = h
            .open_pull_request(&PullRequestInput {
                base: "main".into(),
                head: "h".into(),
                title: "t".into(),
                body: "b".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(pr.url, "https://example.test/pr/1");
        assert_eq!(
            h.calls(),
            ["git.createBranch:ratchet/o/claude", "git.openPullRequest"]
        );
        assert_eq!(h.resolve_ref("main").await.unwrap(), "main-sha");
        h.state(|s| s.resolved.insert("main".into(), "abc".into()));
        assert_eq!(h.resolve_ref("main").await.unwrap(), "abc");
    }

    #[tokio::test]
    async fn commit_files_refuses_existing_branches_and_can_be_made_to_fail() {
        let h = host();
        let input = CommitFilesInput {
            base_ref: "main".into(),
            branch: "ratchet/t".into(),
            message: "m".into(),
            files: vec![CommitFile::new("a", "b")],
            update: false,
        };
        assert_eq!(h.commit_files(&input).await.unwrap().sha, "files-1");
        assert!(
            h.commit_files(&input)
                .await
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        h.state(|s| s.fail_commit_files = true);
        let other = CommitFilesInput {
            branch: "ratchet/u".into(),
            ..input
        };
        assert!(h.commit_files(&other).await.is_err());
    }

    #[tokio::test]
    async fn checkout_writes_the_project_files() {
        let dir = tempfile::tempdir().unwrap();
        host().checkout("s1", dir.path()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/a.txt")).unwrap(),
            "a"
        );
    }
}
