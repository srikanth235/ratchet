//! What "Local folder…" learns about a folder: its GitHub remote, default branch and `ratchet.yaml`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{Error, Result};
use crate::slug::github_slug;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepoInfo {
    pub path: PathBuf,
    /// `owner/name` of `origin` when it is on GitHub.
    pub repo: Option<String>,
    pub base_ref: String,
    pub config_yaml: Option<String>,
    pub problem: Option<String>,
}

/// Runs `git <args>` in `cwd` and returns trimmed stdout.
#[async_trait]
pub trait LocalGit: Send + Sync {
    async fn run(&self, cwd: &Path, args: &[&str]) -> Result<String>;
}

/// The real git CLI with a 10 s timeout and no terminal prompts.
pub struct CliLocalGit;

#[async_trait]
impl LocalGit for CliLocalGit {
    async fn run(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
            .await
            .map_err(|_| Error::Git(format!("git {} timed out", args.join(" "))))??;
        if !out.status.success() {
            return Err(Error::Git(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

pub async fn inspect_local_folder(path: &Path) -> LocalRepoInfo {
    inspect_local_folder_with(path, &CliLocalGit).await
}

pub async fn inspect_local_folder_with(path: &Path, git: &dyn LocalGit) -> LocalRepoInfo {
    let dir = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let base = LocalRepoInfo {
        path: dir.clone(),
        repo: None,
        base_ref: "main".into(),
        config_yaml: None,
        problem: None,
    };
    match std::fs::metadata(&dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return LocalRepoInfo {
                problem: Some(format!("{} is not a folder.", dir.display())),
                ..base
            };
        }
        Err(_) => {
            return LocalRepoInfo {
                problem: Some(format!("{} does not exist.", dir.display())),
                ..base
            };
        }
    }
    if !dir.join(".git").exists() {
        return LocalRepoInfo {
            problem: Some("This folder is not a git repository.".into()),
            ..base
        };
    }

    let config_yaml = std::fs::read_to_string(dir.join("ratchet.yaml")).ok();

    let remote = git
        .run(&dir, &["remote", "get-url", "origin"])
        .await
        .unwrap_or_default();
    let repo = if remote.is_empty() {
        None
    } else {
        github_slug(&remote)
    };

    let mut base_ref = "main".to_string();
    match git
        .run(
            &dir,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        )
        .await
    {
        Ok(out) => {
            let name = out.strip_prefix("origin/").unwrap_or(&out).to_string();
            if !name.is_empty() {
                base_ref = name;
            }
        }
        Err(_) => {
            if let Ok(out) = git.run(&dir, &["symbolic-ref", "--short", "HEAD"]).await
                && !out.is_empty()
            {
                base_ref = out;
            }
        }
    }

    let problem = if repo.is_some() {
        None
    } else if !remote.is_empty() {
        Some("The origin remote is not on GitHub. Ratchet pushes candidate branches to GitHub and the cloud evaluator fetches them from there.".to_string())
    } else {
        Some("This folder has no GitHub remote (origin). Ratchet pushes candidate branches to GitHub and the cloud evaluator fetches them from there.".to_string())
    };
    LocalRepoInfo {
        path: dir,
        repo,
        base_ref,
        config_yaml,
        problem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Std;

    fn git(cwd: &Path, args: &[&str]) {
        let st = Std::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // machine.test.ts: reads the GitHub remote, the branch and ratchet.yaml of a real repository
    #[tokio::test]
    async fn reads_the_github_remote_the_branch_and_ratchet_yaml_of_a_real_repository() {
        let dir = tmp();
        git(dir.path(), &["init", "-q", "-b", "trunk"]);
        git(
            dir.path(),
            &["remote", "add", "origin", "git@github.com:acme/search.git"],
        );
        std::fs::write(dir.path().join("ratchet.yaml"), "version: 0\n").unwrap();
        let r = inspect_local_folder(dir.path()).await;
        assert_eq!(r.repo.as_deref(), Some("acme/search"));
        assert_eq!(r.base_ref, "trunk");
        assert_eq!(r.config_yaml.as_deref(), Some("version: 0\n"));
        assert_eq!(r.problem, None);
    }

    // machine.test.ts: explains a folder that cannot be used
    #[tokio::test]
    async fn explains_a_folder_that_cannot_be_used() {
        let plain = tmp();
        assert!(
            inspect_local_folder(plain.path())
                .await
                .problem
                .unwrap()
                .contains("not a git repository")
        );
        assert!(
            inspect_local_folder(&plain.path().join("missing"))
                .await
                .problem
                .unwrap()
                .contains("does not exist")
        );

        let no_remote = tmp();
        git(no_remote.path(), &["init", "-q"]);
        let r = inspect_local_folder(no_remote.path()).await;
        assert_eq!(r.repo, None);
        assert!(r.problem.unwrap().contains("no GitHub remote"));

        let elsewhere = tmp();
        git(elsewhere.path(), &["init", "-q"]);
        git(
            elsewhere.path(),
            &["remote", "add", "origin", "https://gitlab.com/acme/x.git"],
        );
        assert!(
            inspect_local_folder(elsewhere.path())
                .await
                .problem
                .unwrap()
                .contains("not on GitHub")
        );
    }

    #[tokio::test]
    async fn a_file_is_not_a_folder() {
        let dir = tmp();
        let f = dir.path().join("x");
        std::fs::write(&f, "").unwrap();
        assert!(
            inspect_local_folder(&f)
                .await
                .problem
                .unwrap()
                .contains("is not a folder")
        );
    }
}
