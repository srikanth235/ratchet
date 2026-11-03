//! Simulation's GitHub: the real git CLI against a local bare repo. Branches live in `refs/heads/*`; a "pull request" is
//! a Markdown file plus a JSON sidecar.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::git::{
    BaseGitHostOptions, CommandGitRunner, Git, GitBackend, GitHostCore, GitRunOptions, GitRunner,
};
use ratchet_core::ports::{PullRequestInput, PullRequestRef};

#[derive(Clone)]
pub struct LocalGitHostOptions {
    /// Path of the bare repository acting as "GitHub".
    pub remote_dir: PathBuf,
    /// Where PR bodies are written (`<head with / as __>.md` + `.json`). Defaults to `<remote_dir>/ratchet-prs`.
    pub pr_dir: Option<PathBuf>,
    pub base: BaseGitHostOptions,
}

impl LocalGitHostOptions {
    pub fn new(remote_dir: impl Into<PathBuf>) -> Self {
        LocalGitHostOptions {
            remote_dir: remote_dir.into(),
            pr_dir: None,
            base: BaseGitHostOptions::default(),
        }
    }

    pub fn pr_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.pr_dir = Some(dir.into());
        self
    }
}

pub struct LocalBackend {
    remote_dir: PathBuf,
    pr_dir: PathBuf,
}

/// Simulation's GitHub host.
pub type LocalGitHost = GitHostCore<LocalBackend>;

impl LocalGitHost {
    pub fn new(options: LocalGitHostOptions) -> Result<Self> {
        let remote_dir = std::path::absolute(&options.remote_dir)?;
        let pr_dir = match &options.pr_dir {
            Some(d) => std::path::absolute(d)?,
            None => remote_dir.join("ratchet-prs"),
        };
        let runner: Arc<dyn GitRunner> = options
            .base
            .runner
            .unwrap_or_else(|| Arc::new(CommandGitRunner::default()));
        Ok(GitHostCore::from_backend(
            LocalBackend { remote_dir, pr_dir },
            runner,
            options.base.identity,
        ))
    }
}

pub(crate) fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

#[async_trait]
impl GitBackend for LocalBackend {
    fn remote_url(&self) -> String {
        file_url(&self.remote_dir)
    }

    async fn git_dir(&self, _runner: &Arc<dyn GitRunner>) -> Result<PathBuf> {
        Ok(self.remote_dir.clone())
    }

    fn branch_prefix(&self) -> &'static str {
        "refs/heads/"
    }

    async fn publish(
        &self,
        git: &Git<'_>,
        branch: &str,
        sha: &str,
        force: bool,
        expected: Option<&str>,
    ) -> Result<()> {
        let reference = format!("refs/heads/{branch}");
        // old value of all zeros = "must not exist yet"; `expected` = "must still be at this commit"
        let zeros = "0".repeat(40);
        if force {
            git.run(&["update-ref", &reference, sha], GitRunOptions::default())
                .await?;
        } else {
            git.run(
                &["update-ref", &reference, sha, expected.unwrap_or(&zeros)],
                GitRunOptions::default(),
            )
            .await?;
        }
        Ok(())
    }

    async fn create_pull_request(&self, input: &PullRequestInput) -> Result<PullRequestRef> {
        tokio::fs::create_dir_all(&self.pr_dir).await?;
        let stem = input.head.replace(['/', '\\'], "__");
        let body_path = self.pr_dir.join(format!("{stem}.md"));
        tokio::fs::write(&body_path, format!("# {}\n\n{}\n", input.title, input.body)).await?;
        let meta = serde_json::json!({ "base": input.base, "head": input.head, "title": input.title, "shas": input.shas });
        let meta = serde_json::to_string_pretty(&meta).map_err(Error::invalid)?;
        tokio::fs::write(self.pr_dir.join(format!("{stem}.json")), meta).await?;
        Ok(PullRequestRef {
            url: file_url(&body_path),
        })
    }
}

#[derive(Clone, Default)]
pub struct CreateBareRemoteOptions {
    /// Directory to create the bare repo in; defaults to a fresh temp dir.
    pub remote_dir: Option<PathBuf>,
    /// Working checkout location; defaults to a fresh temp dir.
    pub work_dir: Option<PathBuf>,
    pub branch: Option<String>,
    pub message: Option<String>,
    pub runner: Option<Arc<dyn GitRunner>>,
}

#[derive(Debug, Clone)]
pub struct BareRemote {
    pub remote_dir: PathBuf,
    /// Clone of the remote an agent (or test) commits in; `origin` points at `remote_dir`.
    pub work_dir: PathBuf,
    pub branch: String,
    /// Commit holding the initial copy of the source.
    pub base_sha: String,
}

/// Copy `src_dir` (minus .git/node_modules/dist) into a fresh repo, commit it and push it to a new bare remote.
///
/// When a temp dir is created for the defaults it is kept (the caller owns cleanup), like the TS version.
pub async fn create_bare_remote(
    src_dir: &Path,
    options: CreateBareRemoteOptions,
) -> Result<BareRemote> {
    let run: Arc<dyn GitRunner> = options
        .runner
        .clone()
        .unwrap_or_else(|| Arc::new(CommandGitRunner::default()));
    let branch = options.branch.clone().unwrap_or_else(|| "main".into());
    let root = if options.remote_dir.is_some() && options.work_dir.is_some() {
        None
    } else {
        Some(
            tempfile::Builder::new()
                .prefix("ratchet-remote-")
                .tempdir()?
                .keep(),
        )
    };
    let remote_dir = std::path::absolute(match &options.remote_dir {
        Some(d) => d.clone(),
        None => root
            .as_ref()
            .expect("root exists when remote_dir is unset")
            .join("remote.git"),
    })?;
    let work_dir = std::path::absolute(match &options.work_dir {
        Some(d) => d.clone(),
        None => root
            .as_ref()
            .expect("root exists when work_dir is unset")
            .join("work"),
    })?;
    let env = |o: GitRunOptions| {
        o.env("GIT_AUTHOR_NAME", "Ratchet")
            .env("GIT_AUTHOR_EMAIL", "ratchet@localhost")
            .env("GIT_COMMITTER_NAME", "Ratchet")
            .env("GIT_COMMITTER_EMAIL", "ratchet@localhost")
    };
    let git = |args: &[&str], opts: GitRunOptions| {
        let run = run.clone();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        async move { run.run(&args, &opts).await }
    };
    let (src, dst) = (src_dir.to_path_buf(), work_dir.clone());
    tokio::fs::create_dir_all(&work_dir).await?;
    tokio::task::spawn_blocking(move || copy_tree(&src, &dst))
        .await
        .map_err(|e| Error::invalid(format!("copy failed: {e}")))??;
    let w = work_dir.to_string_lossy().into_owned();
    let r = remote_dir.to_string_lossy().into_owned();
    git(&["init", "-q", "-b", &branch, &w], Default::default()).await?;
    git(&["add", "-A"], GitRunOptions::default().cwd(&work_dir)).await?;
    let message = options
        .message
        .clone()
        .unwrap_or_else(|| "Initial import".into());
    git(
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", &message],
        env(GitRunOptions::default().cwd(&work_dir)),
    )
    .await?;
    let base_sha = git(
        &["rev-parse", "HEAD"],
        GitRunOptions::default().cwd(&work_dir),
    )
    .await?
    .trim()
    .to_string();
    git(&["clone", "-q", "--bare", &w, &r], Default::default()).await?;
    git(
        &["remote", "add", "origin", &r],
        GitRunOptions::default().cwd(&work_dir),
    )
    .await?;
    git(
        &["fetch", "-q", "origin"],
        GitRunOptions::default().cwd(&work_dir),
    )
    .await?;
    git(
        &[
            "branch",
            "-q",
            "--set-upstream-to",
            &format!("origin/{branch}"),
            &branch,
        ],
        GitRunOptions::default().cwd(&work_dir),
    )
    .await?;
    Ok(BareRemote {
        remote_dir,
        work_dir,
        branch,
        base_sha,
    })
}

/// Recursive copy that skips anything named `.git`, `node_modules` or `dist`.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git" | "node_modules" | "dist")) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_symlink() {
            // follow like `cpSync` without `dereference` would copy the link; keep the content instead
            if let Ok(meta) = std::fs::metadata(&from) {
                if meta.is_dir() {
                    copy_tree(&from, &to)?;
                } else {
                    std::fs::copy(&from, &to)?;
                }
            }
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
