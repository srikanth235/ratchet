//! Thin `git` wrapper: an injectable runner (so tests and callers can fake or audit git), a `Git` bound to one directory, and the
//! `Ratchet-Candidate: true` commit trailer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl GitResult {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    pub fn failed(stderr: impl Into<String>) -> Self {
        Self {
            code: 1,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GitCall {
    pub cwd: PathBuf,
    pub env: Option<HashMap<String, String>>,
}

/// Runs `git <args>` in `call.cwd`.
#[async_trait]
pub trait GitRunner: Send + Sync {
    async fn run(&self, args: &[String], call: &GitCall) -> GitResult;
}

pub type SharedGit = Arc<dyn GitRunner>;

/// The real `git` CLI. Non-interactive: a missing credential must fail, not prompt.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExecGit;

#[async_trait]
impl GitRunner for ExecGit {
    async fn run(&self, args: &[String], call: &GitCall) -> GitResult {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(args)
            .current_dir(&call.cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(env) = &call.env {
            cmd.envs(env);
        }
        match cmd.output().await {
            Ok(out) => GitResult {
                code: out.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(err) => GitResult {
                code: 1,
                stdout: String::new(),
                stderr: err.to_string(),
            },
        }
    }
}

pub fn exec_git() -> SharedGit {
    Arc::new(ExecGit)
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("git {} failed ({}): {}", .args.join(" "), .result.code, .message)]
pub struct GitError {
    pub args: Vec<String>,
    pub result: GitResult,
    message: String,
}

impl GitError {
    pub fn new(args: Vec<String>, result: GitResult) -> Self {
        let text = if result.stderr.is_empty() {
            &result.stdout
        } else {
            &result.stderr
        };
        let message = text.trim().to_string();
        Self {
            args,
            result,
            message,
        }
    }
}

/// Thin wrapper bound to one working directory.
#[derive(Clone)]
pub struct Git {
    cwd: PathBuf,
    runner: SharedGit,
    env: Option<HashMap<String, String>>,
}

impl Git {
    pub fn new(
        cwd: impl Into<PathBuf>,
        runner: SharedGit,
        env: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            runner,
            env,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn runner(&self) -> &SharedGit {
        &self.runner
    }

    pub fn env(&self) -> Option<&HashMap<String, String>> {
        self.env.as_ref()
    }

    pub async fn try_run(&self, args: &[&str]) -> GitResult {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.runner
            .run(
                &owned,
                &GitCall {
                    cwd: self.cwd.clone(),
                    env: self.env.clone(),
                },
            )
            .await
    }

    pub async fn run(&self, args: &[&str]) -> Result<String, GitError> {
        let res = self.try_run(args).await;
        if res.code != 0 {
            return Err(GitError::new(
                args.iter().map(|s| s.to_string()).collect(),
                res,
            ));
        }
        Ok(res.stdout)
    }

    pub async fn head(&self) -> Result<String, GitError> {
        Ok(self.run(&["rev-parse", "HEAD"]).await?.trim().to_string())
    }

    pub async fn rev_parse(&self, reference: &str) -> Option<String> {
        let res = self
            .try_run(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ])
            .await;
        (res.code == 0).then(|| res.stdout.trim().to_string())
    }

    pub async fn is_dirty(&self) -> Result<bool, GitError> {
        Ok(!self
            .run(&["status", "--porcelain"])
            .await?
            .trim()
            .is_empty())
    }

    pub async fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        self.try_run(&["merge-base", "--is-ancestor", ancestor, descendant])
            .await
            .code
            == 0
    }

    pub async fn changed_files(&self, base: &str, head: &str) -> Result<Vec<String>, GitError> {
        let out = self.run(&["diff", "--name-only", base, head]).await?;
        Ok(out
            .split('\n')
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub async fn message(&self, reference: &str) -> Result<String, GitError> {
        self.run(&["log", "-1", "--format=%B", reference]).await
    }
}

pub const CANDIDATE_TRAILER: &str = "Ratchet-Candidate: true";

/// `^[A-Za-z][A-Za-z0-9-]*: \S`
fn is_trailer_line(line: &str) -> bool {
    let mut chars = line.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    let Some(colon) = line.find(": ") else {
        return false;
    };
    let key = &line[..colon];
    key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && line[colon + 2..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace())
}

/// Split on blank (whitespace-only) lines, like the regex `\n\s*\n`: returns the last paragraph.
fn last_paragraph(body: &str) -> &str {
    let mut last_start = 0;
    let mut offset = 0;
    let mut prev_blank_end: Option<usize> = None;
    for line in body.split_inclusive('\n') {
        let blank = line.trim().is_empty();
        if blank {
            prev_blank_end = Some(offset + line.len());
        } else if let Some(end) = prev_blank_end.take() {
            last_start = end;
        }
        offset += line.len();
    }
    &body[last_start..]
}

/// Add the candidate trailer to a commit message (idempotent), separated by a blank line from the body.
pub fn with_candidate_trailer(message: &str) -> String {
    let body = message.trim_end();
    if has_candidate_trailer(body) {
        return format!("{body}\n");
    }
    let is_trailer_block =
        body.contains("\n\n") && last_paragraph(body).split('\n').all(is_trailer_line);
    format!(
        "{body}{}{CANDIDATE_TRAILER}\n",
        if is_trailer_block { "\n" } else { "\n\n" }
    )
}

/// `^Ratchet-Candidate:\s*true\s*$` (case-insensitive) on any line.
pub fn has_candidate_trailer(message: &str) -> bool {
    message.split('\n').any(|l| {
        let l = l.trim_end();
        const KEY: &str = "Ratchet-Candidate:";
        match l.get(..KEY.len()) {
            Some(prefix) if prefix.eq_ignore_ascii_case(KEY) => {
                l[KEY.len()..].trim_start().eq_ignore_ascii_case("true")
            }
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_is_added_once_after_a_blank_line() {
        assert_eq!(
            with_candidate_trailer("subject"),
            "subject\n\nRatchet-Candidate: true\n"
        );
        assert_eq!(
            with_candidate_trailer("subject\n\nbody\n"),
            "subject\n\nbody\n\nRatchet-Candidate: true\n"
        );
        assert_eq!(
            with_candidate_trailer("s\n\nRatchet-Candidate: true\n"),
            "s\n\nRatchet-Candidate: true\n"
        );
    }

    #[test]
    fn trailer_joins_an_existing_trailer_block() {
        assert_eq!(
            with_candidate_trailer("s\n\nbody\n\nSigned-off-by: A <a@b>\n"),
            "s\n\nbody\n\nSigned-off-by: A <a@b>\nRatchet-Candidate: true\n"
        );
    }

    #[test]
    fn trailer_detection() {
        assert!(has_candidate_trailer("s\n\nRatchet-Candidate: true"));
        assert!(has_candidate_trailer("s\n\nratchet-candidate:   TRUE  "));
        assert!(!has_candidate_trailer("s\n\nRatchet-Candidate: false"));
        assert!(!has_candidate_trailer("Ratchet-Candidate: truely"));
    }

    #[test]
    fn a_single_paragraph_is_never_a_trailer_block() {
        // `s: x` alone looks like a trailer line, but with no body before it the TS rule still appends after a blank line
        assert_eq!(
            with_candidate_trailer("fix: thing"),
            "fix: thing\n\nRatchet-Candidate: true\n"
        );
    }
}
