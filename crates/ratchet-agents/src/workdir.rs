//! Working directories for agents: a full clone per agent (cloud runners, simulation), or one local clone per optimization with a
//! `git worktree` per agent ("This machine" mode).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::git::{Git, GitError, SharedGit, exec_git};

#[derive(Debug, thiserror::Error)]
pub enum WorkdirError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("base ref '{0}' not found in {1}")]
    BaseRefNotFound(String, String),
    #[error("base ref '{0}' not found in the local clone")]
    BaseRefNotFoundLocally(String),
    #[error("{0}")]
    Io(String),
}

fn io_err(err: std::io::Error) -> WorkdirError {
    WorkdirError::Io(err.to_string())
}

fn parent_of(dir: &Path) -> PathBuf {
    dir.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub struct PrepareWorkdirOptions {
    pub repo_url: String,
    /// The agent's branch; created from `base_ref` when the remote does not have it yet.
    pub branch: String,
    pub base_ref: String,
    pub dir: PathBuf,
    pub git: Option<SharedGit>,
    /// Extra environment for git (credentials, e.g. `GIT_CONFIG_*` for an http extraheader).
    pub env: Option<HashMap<String, String>>,
}

/// Clone `repo_url` into `dir` and check out the agent's branch.
pub async fn prepare_workdir(options: PrepareWorkdirOptions) -> Result<Git, WorkdirError> {
    let runner = options.git.clone().unwrap_or_else(exec_git);
    let parent_dir = parent_of(&options.dir);
    tokio::fs::create_dir_all(&parent_dir)
        .await
        .map_err(io_err)?;
    let parent = Git::new(&parent_dir, runner.clone(), options.env.clone());
    parent
        .run(&[
            "clone",
            "--quiet",
            &options.repo_url,
            &options.dir.to_string_lossy(),
        ])
        .await?;
    let git = Git::new(&options.dir, runner, options.env.clone());
    let remote_ref = format!("refs/remotes/origin/{}", options.branch);
    if git.rev_parse(&remote_ref).await.is_some() {
        git.run(&["checkout", "--quiet", "-B", &options.branch, &remote_ref])
            .await?;
    } else {
        let base = match git
            .rev_parse(&format!("refs/remotes/origin/{}", options.base_ref))
            .await
        {
            Some(b) => Some(b),
            None => git.rev_parse(&options.base_ref).await,
        };
        let Some(base) = base else {
            return Err(WorkdirError::BaseRefNotFound(
                options.base_ref,
                options.repo_url,
            ));
        };
        git.run(&["checkout", "--quiet", "-B", &options.branch, &base])
            .await?;
    }
    Ok(git)
}

/// Environment that makes git authenticate to the host of `repo_url` with a token, without writing it to disk or the command
/// line. Scoped to the origin of `repo_url`; empty for anything that is not `https://`.
pub fn git_auth_env(repo_url: &str, token: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if !repo_url.starts_with("https://") {
        return env;
    }
    let Ok(url) = url::Url::parse(repo_url) else {
        return env;
    };
    let origin = url.origin().ascii_serialization();
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    env.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
    env.insert(
        "GIT_CONFIG_KEY_0".to_string(),
        format!("http.{origin}/.extraheader"),
    );
    env.insert(
        "GIT_CONFIG_VALUE_0".to_string(),
        format!("AUTHORIZATION: basic {basic}"),
    );
    env
}

// ---------------------------------------------------------------- local mode: one clone, a git worktree per agent

pub struct LocalCloneOptions {
    pub repo_url: String,
    /// Where the clone lives (not the user's own working copy: Ratchet keeps a separate clone under its app data).
    pub dir: PathBuf,
    pub git: Option<SharedGit>,
    pub env: Option<HashMap<String, String>>,
}

/// Make sure `dir` holds a clone of `repo_url`: clone it, or fetch when it is already there (a second run of the same optimization
/// after a restart). Per-worktree git config is switched on so each agent can have its own identity.
pub async fn ensure_local_clone(options: LocalCloneOptions) -> Result<Git, WorkdirError> {
    let runner = options.git.clone().unwrap_or_else(exec_git);
    let parent_dir = parent_of(&options.dir);
    tokio::fs::create_dir_all(&parent_dir)
        .await
        .map_err(io_err)?;
    if options.dir.join(".git").exists() {
        let clone = Git::new(&options.dir, runner, options.env.clone());
        clone
            .run(&["fetch", "--quiet", "--prune", "origin"])
            .await?;
        clone
            .run(&["config", "extensions.worktreeConfig", "true"])
            .await?;
        return Ok(clone);
    }
    let parent = Git::new(&parent_dir, runner.clone(), options.env.clone());
    parent
        .run(&[
            "clone",
            "--quiet",
            &options.repo_url,
            &options.dir.to_string_lossy(),
        ])
        .await?;
    let clone = Git::new(&options.dir, runner, options.env.clone());
    clone
        .run(&["config", "extensions.worktreeConfig", "true"])
        .await?;
    Ok(clone)
}

#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

pub struct AddWorktreeOptions {
    pub clone: Git,
    /// The worktree directory (one per agent).
    pub dir: PathBuf,
    pub branch: String,
    pub base_ref: String,
    /// Commit identity for this agent, stored in the worktree's own config so agents sharing the clone do not overwrite each other.
    pub identity: Identity,
    pub git: Option<SharedGit>,
    pub env: Option<HashMap<String, String>>,
}

/// `git worktree add -B <branch> <dir> <start>`: the agent's branch is the remote's when it exists, else created from `base_ref`
/// (same rule as `prepare_workdir`). Commit signing is switched off in the worktree: an agent must never wait on a pinentry prompt.
pub async fn add_worktree(options: AddWorktreeOptions) -> Result<Git, WorkdirError> {
    let AddWorktreeOptions {
        clone,
        dir,
        branch,
        base_ref,
        identity,
        git,
        env,
    } = options;
    if dir.exists() {
        remove_worktree(&clone, &dir).await; // left over from a crashed run
    }
    let start = match clone
        .rev_parse(&format!("refs/remotes/origin/{branch}"))
        .await
    {
        Some(s) => s,
        None => {
            let base = match clone
                .rev_parse(&format!("refs/remotes/origin/{base_ref}"))
                .await
            {
                Some(b) => Some(b),
                None => clone.rev_parse(&base_ref).await,
            };
            base.ok_or_else(|| WorkdirError::BaseRefNotFoundLocally(base_ref.clone()))?
        }
    };
    tokio::fs::create_dir_all(parent_of(&dir))
        .await
        .map_err(io_err)?;
    clone
        .run(&[
            "worktree",
            "add",
            "--quiet",
            "-B",
            &branch,
            &dir.to_string_lossy(),
            &start,
        ])
        .await?;
    let tree = Git::new(&dir, git.unwrap_or_else(exec_git), env);
    tree.run(&["config", "--worktree", "user.name", &identity.name])
        .await?;
    tree.run(&["config", "--worktree", "user.email", &identity.email])
        .await?;
    tree.run(&["config", "--worktree", "commit.gpgsign", "false"])
        .await?;
    Ok(tree)
}

/// Remove an agent's worktree (and its directory, even if git refuses); never fails.
pub async fn remove_worktree(clone: &Git, dir: &Path) {
    let _ = clone
        .try_run(&["worktree", "remove", "--force", &dir.to_string_lossy()])
        .await;
    let _ = tokio::fs::remove_dir_all(dir).await;
    let _ = clone.try_run(&["worktree", "prune"]).await;
}
