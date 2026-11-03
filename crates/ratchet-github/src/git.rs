//! Git over the `git` CLI: the runner, and the logic shared by every remote (`GitHostCore`).
//!
//! Ported from `packages/github/src/git.ts`. Everything is plumbing against a bare repository (`--git-dir`), so no
//! working tree is needed and nothing but refs is touched.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{Error, Result};
use ratchet_core::ports::{
    CandidateCommit, CommitFilesInput, CommitRef, FileDiff, GitHost, PortResult, PullRequestInput,
    PullRequestRef,
};

/// The commit trailer that marks a candidate.
pub const CANDIDATE_TRAILER: &str = "Ratchet-Candidate";

const US: char = '\x1f';
const RS: char = '\x1e';

#[derive(Debug, Clone, Default)]
pub struct GitRunOptions {
    pub cwd: Option<PathBuf>,
    /// Extra environment variables (merged over the process environment). Secrets go here, never in argv.
    pub env: Vec<(String, String)>,
    /// Written to the process's stdin.
    pub input: Option<Vec<u8>>,
}

impl GitRunOptions {
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn input(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.input = Some(input.into());
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

/// Runs `git <args>` and returns stdout. Fails on a non-zero exit.
#[async_trait]
pub trait GitRunner: Send + Sync {
    async fn run(&self, args: &[String], opts: &GitRunOptions) -> Result<String>;
}

/// The real git CLI. Strings in `redact` are scrubbed from error messages.
#[derive(Debug, Clone, Default)]
pub struct CommandGitRunner {
    redact: Vec<String>,
}

impl CommandGitRunner {
    pub fn new(redact: impl IntoIterator<Item = String>) -> Self {
        CommandGitRunner {
            redact: redact.into_iter().filter(|s| !s.is_empty()).collect(),
        }
    }

    fn scrub(&self, s: &str) -> String {
        self.redact.iter().fold(s.to_string(), |acc, secret| {
            acc.replace(secret.as_str(), "***")
        })
    }
}

#[async_trait]
impl GitRunner for CommandGitRunner {
    async fn run(&self, args: &[String], opts: &GitRunOptions) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        cmd.kill_on_drop(true);
        let fail = |detail: String| {
            Error::Git(self.scrub(&format!("git {} failed: {}", args.join(" "), detail.trim())))
        };
        let mut child = cmd.spawn().map_err(|e| fail(e.to_string()))?;
        let mut stdin = child.stdin.take();
        let input = opts.input.clone();
        let writer = tokio::spawn(async move {
            if let Some(mut stdin) = stdin.take() {
                if let Some(input) = input {
                    // a process that exits early closes the pipe; its exit code tells the story
                    let _ = stdin.write_all(&input).await;
                }
                let _ = stdin.shutdown().await;
            }
        });
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| fail(e.to_string()))?;
        let _ = writer.await;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let detail = if stderr.trim().is_empty() {
                format!("exit status {}", out.status)
            } else {
                stderr
            };
            return Err(fail(detail));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Author and committer of the commits Ratchet writes (squashes, template commits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Default for Identity {
    fn default() -> Self {
        Identity {
            name: "Ratchet".into(),
            email: "ratchet@users.noreply.github.com".into(),
        }
    }
}

impl Identity {
    fn env(&self) -> GitRunOptions {
        GitRunOptions::default()
            .env("GIT_AUTHOR_NAME", &self.name)
            .env("GIT_AUTHOR_EMAIL", &self.email)
            .env("GIT_COMMITTER_NAME", &self.name)
            .env("GIT_COMMITTER_EMAIL", &self.email)
    }
}

/// Git commands against one (bare) repository.
pub struct Git<'a> {
    pub runner: &'a Arc<dyn GitRunner>,
    pub dir: &'a Path,
}

impl Git<'_> {
    /// `git --git-dir <dir> <args…>`
    pub async fn run(&self, args: &[&str], opts: GitRunOptions) -> Result<String> {
        let mut full: Vec<String> =
            vec!["--git-dir".into(), self.dir.to_string_lossy().into_owned()];
        full.extend(args.iter().map(|a| a.to_string()));
        self.runner.run(&full, &opts).await
    }
}

/// Where a remote's branches live and how it syncs, publishes and opens pull requests. `GitHostCore` supplies the rest.
#[async_trait]
pub trait GitBackend: Send + Sync {
    fn remote_url(&self) -> String;
    /// Directory of the (bare) repo that git commands operate on.
    async fn git_dir(&self, runner: &Arc<dyn GitRunner>) -> Result<PathBuf>;
    /// Namespace holding the remote's branches inside the git dir.
    fn branch_prefix(&self) -> &'static str;
    /// Bring the git dir up to date with the remote before reading.
    async fn sync(&self, _git: &Git<'_>) -> Result<()> {
        Ok(())
    }
    /// Make the local `sha` visible as `refs/heads/<branch>` on the remote.
    ///
    /// With `expected` the branch must still be at that commit (an update of an existing branch, never a force).
    async fn publish(
        &self,
        git: &Git<'_>,
        branch: &str,
        sha: &str,
        force: bool,
        expected: Option<&str>,
    ) -> Result<()>;
    async fn create_pull_request(&self, input: &PullRequestInput) -> Result<PullRequestRef>;
}

#[derive(Clone, Default)]
pub struct BaseGitHostOptions {
    pub runner: Option<Arc<dyn GitRunner>>,
    /// Author/committer for the squash commit.
    pub identity: Identity,
}

/// Shared git logic over a (bare) repository; the backend decides where branches live and how they sync.
pub struct GitHostCore<B: GitBackend> {
    backend: B,
    runner: Arc<dyn GitRunner>,
    identity: Identity,
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl<B: GitBackend> GitHostCore<B> {
    pub(crate) fn from_backend(backend: B, runner: Arc<dyn GitRunner>, identity: Identity) -> Self {
        GitHostCore {
            backend,
            runner,
            identity,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    async fn dir(&self) -> Result<PathBuf> {
        self.backend.git_dir(&self.runner).await
    }

    async fn git(&self, args: &[&str], opts: GitRunOptions) -> Result<String> {
        let dir = self.dir().await?;
        Git {
            runner: &self.runner,
            dir: &dir,
        }
        .run(args, opts)
        .await
    }

    async fn sync(&self) -> Result<()> {
        let dir = self.dir().await?;
        self.backend
            .sync(&Git {
                runner: &self.runner,
                dir: &dir,
            })
            .await
    }

    async fn publish(
        &self,
        branch: &str,
        sha: &str,
        force: bool,
        expected: Option<&str>,
    ) -> Result<()> {
        let dir = self.dir().await?;
        self.backend
            .publish(
                &Git {
                    runner: &self.runner,
                    dir: &dir,
                },
                branch,
                sha,
                force,
                expected,
            )
            .await
    }

    /// Resolve a branch name, SHA or ref to a full commit SHA.
    async fn resolve(&self, reference: &str) -> Result<String> {
        if is_sha(reference) {
            return Ok(reference.to_string());
        }
        let attempts: Vec<String> = if reference.starts_with("refs/") {
            vec![reference.to_string()]
        } else {
            vec![
                format!("{}{}", self.backend.branch_prefix(), reference),
                reference.to_string(),
            ]
        };
        let mut last_err: Option<Error> = None;
        for candidate in attempts {
            match self
                .git(
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("{candidate}^{{commit}}"),
                    ],
                    Default::default(),
                )
                .await
            {
                Ok(out) => return Ok(out.trim().to_string()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(Error::Git(format!(
            "cannot resolve ref \"{reference}\"{}",
            last_err.map(|e| format!(": {e}")).unwrap_or_default()
        )))
    }

    /// Create `input.head` as a single commit holding the final verified tree, parented at the merge-base with `base`.
    async fn write_squash(&self, input: &PullRequestInput) -> Result<()> {
        let mut shas = Vec::with_capacity(input.shas.len());
        for s in &input.shas {
            shas.push(self.resolve(s).await?);
        }
        let fin = shas
            .last()
            .cloned()
            .ok_or_else(|| Error::invalid("no verified commits to squash"))?;
        let base = self.resolve(&input.base).await?;
        let parent = match self
            .git(&["merge-base", &base, &fin], Default::default())
            .await
        {
            Ok(out) => out.trim().to_string(),
            Err(_) => {
                return Err(Error::Git(format!(
                    "verified commit {fin} shares no history with {}",
                    input.base
                )));
            }
        };
        let tree = self
            .git(
                &["rev-parse", &format!("{fin}^{{tree}}")],
                Default::default(),
            )
            .await?
            .trim()
            .to_string();
        let message = format!(
            "{}\n",
            input
                .commit_message
                .as_deref()
                .unwrap_or(&input.title)
                .trim_end()
        );
        let commit = self
            .git(
                &["commit-tree", &tree, "-p", &parent, "-F", "-"],
                self.identity.env().input(message),
            )
            .await?
            .trim()
            .to_string();
        self.publish(&input.head, &commit, true, None).await
    }
}

#[async_trait]
impl<B: GitBackend> GitHost for GitHostCore<B> {
    async fn resolve_ref(&self, reference: &str) -> PortResult<String> {
        self.sync().await?;
        Ok(self.resolve(reference).await?)
    }

    async fn create_branch(&self, from_ref: &str, branch: &str) -> PortResult<()> {
        self.sync().await?;
        let sha = self.resolve(from_ref).await?;
        Ok(self.publish(branch, &sha, false, None).await?)
    }

    async fn list_candidates(
        &self,
        branch: &str,
        since_sha: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>> {
        self.sync().await?;
        let tip = self.resolve(branch).await?;
        let range = match since_sha {
            Some(since) => format!("{}..{tip}", self.resolve(since).await?),
            None => tip,
        };
        // %(trailers:...) matches keys case-insensitively and understands real trailer blocks only.
        let format = [
            "%H".to_string(),
            "%ct".to_string(),
            format!("%(trailers:key={CANDIDATE_TRAILER},valueonly,separator=%x2C)"),
            "%B".to_string(),
        ]
        .join(&US.to_string());
        let out = self
            .git(
                &[
                    "log",
                    "--reverse",
                    &format!("--format={format}{RS}"),
                    &range,
                ],
                Default::default(),
            )
            .await?;
        let mut commits = Vec::new();
        for record in out.split(RS) {
            let rec = record.strip_prefix('\n').unwrap_or(record);
            if rec.trim().is_empty() {
                continue;
            }
            let mut parts = rec.split(US);
            let (Some(sha), Some(ct)) = (parts.next(), parts.next()) else {
                continue;
            };
            if sha.is_empty() || ct.is_empty() {
                continue;
            }
            let trailer_values = parts.next().unwrap_or("");
            let message = parts.collect::<Vec<_>>().join(&US.to_string());
            // if the trailer is repeated, the last one wins
            let last = trailer_values
                .split([',', '\n'])
                .map(str::trim)
                .rfind(|v| !v.is_empty());
            if !last.is_some_and(|v| v.eq_ignore_ascii_case("true")) {
                continue;
            }
            commits.push(CandidateCommit {
                sha: sha.to_string(),
                branch: branch.to_string(),
                message: message.trim().to_string(),
                committed_at: ct.trim().parse::<i64>().unwrap_or(0) * 1000,
            });
        }
        Ok(commits)
    }

    async fn changed_files(&self, base_ref: &str, sha: &str) -> PortResult<Vec<String>> {
        self.sync().await?;
        let base = self.resolve(base_ref).await?;
        let head = self.resolve(sha).await?;
        let out = self
            .git(
                &["diff", "--name-only", "-z", &base, &head],
                Default::default(),
            )
            .await?;
        Ok(out
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect())
    }

    async fn diff(&self, base_ref: &str, sha: &str) -> PortResult<Vec<FileDiff>> {
        self.sync().await?;
        let base = self.resolve(base_ref).await?;
        let head = self.resolve(sha).await?;
        let from = match self
            .git(&["merge-base", &base, &head], Default::default())
            .await
        {
            Ok(out) if !out.trim().is_empty() => out.trim().to_string(),
            // unrelated histories: compare the trees directly
            _ => base,
        };
        let opts = ["--no-color", "--no-ext-diff", "--no-renames"];
        let numstat = self
            .git(
                &[
                    "diff",
                    opts[0],
                    opts[1],
                    opts[2],
                    "--numstat",
                    "-z",
                    &from,
                    &head,
                ],
                Default::default(),
            )
            .await?;
        let patch = self
            .git(
                &["diff", opts[0], opts[1], opts[2], "-U3", &from, &head],
                Default::default(),
            )
            .await?;
        Ok(parse_git_diff(&numstat, &patch))
    }

    /// Materialize the tree of `sha` into `dir` (no `.git`, no side effects on the repo).
    async fn checkout(&self, sha: &str, dir: &Path) -> PortResult<()> {
        self.sync().await?;
        let commit = self.resolve(sha).await?;
        tokio::fs::create_dir_all(dir).await?;
        let work = std::path::absolute(dir)?;
        let scratch = tempfile::Builder::new()
            .prefix("ratchet-index-")
            .tempdir()?;
        let opts = || {
            GitRunOptions::default()
                .env(
                    "GIT_INDEX_FILE",
                    scratch.path().join("index").to_string_lossy(),
                )
                .env("GIT_WORK_TREE", work.to_string_lossy())
                .cwd(work.clone())
        };
        self.git(&["read-tree", &commit], opts()).await?;
        self.git(&["checkout-index", "-a", "-f"], opts()).await?;
        Ok(())
    }

    async fn open_pull_request(&self, input: &PullRequestInput) -> PortResult<PullRequestRef> {
        self.sync().await?;
        if !input.shas.is_empty() {
            self.write_squash(input).await?;
        }
        Ok(self.backend.create_pull_request(input).await?)
    }

    fn remote_url(&self) -> String {
        self.backend.remote_url()
    }

    /// Branch `input.branch` off `input.base_ref` with one commit that writes `input.files` over the base tree.
    /// Built with plumbing (a scratch index, `hash-object`, `write-tree`, `commit-tree`), so no working tree is needed
    /// and nothing but the new ref is touched. Refuses an existing branch (publish is not forced) and unsafe paths.
    /// With `update` the branch must exist and be at `base_ref`: the commit is added on top of it (a fast-forward).
    async fn commit_files(&self, input: &CommitFilesInput) -> PortResult<CommitRef> {
        if input.files.is_empty() {
            return Err(Error::invalid("commitFiles: no files").into());
        }
        let mut seen = std::collections::HashSet::new();
        for f in &input.files {
            let unsafe_path = f.path.is_empty()
                || f.path.contains('\\')
                || f.path.contains('\0')
                || f.path.split('/').any(|x| {
                    x.is_empty() || x == "." || x == ".." || x.eq_ignore_ascii_case(".git")
                });
            if unsafe_path {
                return Err(Error::invalid(format!(
                    "commitFiles: unsafe path {}",
                    serde_json::to_string(&f.path).unwrap_or_default()
                ))
                .into());
            }
            if !seen.insert(f.path.as_str()) {
                return Err(
                    Error::invalid(format!("commitFiles: duplicate path {}", f.path)).into(),
                );
            }
        }
        self.sync().await?;
        let base = self.resolve(&input.base_ref).await?;
        if input.update {
            // Straight onto an existing branch (a protected ref): only on top of the tip the caller based the files on.
            let tip = self.resolve(&input.branch).await.map_err(|_| {
                Error::invalid(format!(
                    "commitFiles: branch {} does not exist",
                    input.branch
                ))
            })?;
            if tip != base {
                return Err(Error::invalid(format!(
                    "commitFiles: {} is not at {}; refusing to move it",
                    input.branch, input.base_ref
                ))
                .into());
            }
        }
        let scratch = tempfile::Builder::new()
            .prefix("ratchet-commit-")
            .tempdir()?;
        let index = scratch.path().join("index").to_string_lossy().into_owned();
        let with_index = || GitRunOptions::default().env("GIT_INDEX_FILE", index.clone());
        self.git(&["read-tree", &base], with_index()).await?;
        for f in &input.files {
            let blob = self
                .git(
                    &["hash-object", "-w", "--stdin"],
                    with_index().input(f.content.clone()),
                )
                .await?;
            let mode = if f.is_executable() {
                "100755"
            } else {
                "100644"
            };
            self.git(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("{mode},{},{}", blob.trim(), f.path),
                ],
                with_index(),
            )
            .await?;
        }
        let tree = self
            .git(&["write-tree"], with_index())
            .await?
            .trim()
            .to_string();
        let message = format!("{}\n", input.message.trim_end());
        let sha = self
            .git(
                &["commit-tree", &tree, "-p", &base, "-F", "-"],
                self.identity.env().input(message),
            )
            .await?
            .trim()
            .to_string();
        let expected = input.update.then_some(base.as_str());
        self.publish(&input.branch, &sha, false, expected).await?;
        Ok(CommitRef { sha })
    }
}

/// Combine `git diff --numstat -z` (paths and counts, robust to odd file names) with the unified `git diff` of the same
/// range (hunks). Both list files in the same order, so sections are matched by position.
pub fn parse_git_diff(numstat: &str, patch: &str) -> Vec<FileDiff> {
    let stat_re = Regex::new(r"^(-|\d+)\t(-|\d+)\t([\s\S]+)$").expect("static regex");
    let header_re = Regex::new(r"(?m)^diff --git .*$").expect("static regex");
    let hunk_re = Regex::new(r"(?m)^@@").expect("static regex");

    struct Stat {
        path: String,
        additions: u64,
        deletions: u64,
        binary: bool,
    }
    let stats: Vec<Stat> = numstat
        .split('\0')
        .filter_map(|f| {
            let f = f.strip_prefix('\n').unwrap_or(f);
            let m = stat_re.captures(f)?;
            let binary = &m[1] == "-" && &m[2] == "-";
            Some(Stat {
                path: m[3].to_string(),
                additions: if binary { 0 } else { m[1].parse().unwrap_or(0) },
                deletions: if binary { 0 } else { m[2].parse().unwrap_or(0) },
                binary,
            })
        })
        .collect();
    let sections: Vec<&str> = header_re.split(patch).skip(1).collect();
    stats
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let body = sections.get(i).copied().unwrap_or("");
            let patch = match hunk_re.find(body) {
                Some(at) if !s.binary => {
                    let hunks = &body[at.start()..];
                    hunks.strip_suffix('\n').unwrap_or(hunks).to_string()
                }
                _ => String::new(),
            };
            FileDiff {
                path: s.path,
                additions: s.additions,
                deletions: s.deletions,
                binary: s.binary,
                patch,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat_and_patch_sections() {
        let numstat = "3\t1\tsrc/search.ts\0-\t-\tlogo.png\0";
        let patch = "diff --git a/src/search.ts b/src/search.ts\nindex 1..2 100644\n--- a/src/search.ts\n+++ b/src/search.ts\n@@ -1,2 +1,4 @@\n-a\n+b\n+c\n+d\n x\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n";
        let files = parse_git_diff(numstat, patch);
        assert_eq!(
            files,
            vec![
                FileDiff {
                    path: "src/search.ts".into(),
                    additions: 3,
                    deletions: 1,
                    binary: false,
                    patch: "@@ -1,2 +1,4 @@\n-a\n+b\n+c\n+d\n x".into()
                },
                FileDiff {
                    path: "logo.png".into(),
                    additions: 0,
                    deletions: 0,
                    binary: true,
                    patch: String::new()
                },
            ]
        );
    }

    #[test]
    fn sha_detection_is_lowercase_hex_40() {
        assert!(is_sha(&"a".repeat(40)));
        assert!(!is_sha(&"A".repeat(40)));
        assert!(!is_sha("main"));
    }
}
