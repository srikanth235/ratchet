//! Subprocess seam, so tests (and simulation) never spawn `tofu`. (TS `sandbox/process.ts`)

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Added to (and overriding) the parent environment.
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Injectable process runner. Cancel by dropping the future: the child is killed.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessResult>;
}

/// Runs the process with tokio. stdin is closed, stdout/stderr are captured, and the child is killed when the future is
/// dropped.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioProcessRunner;

#[async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessResult> {
        let out = Command::new(&spec.command)
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await?;
        Ok(ProcessResult {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}
