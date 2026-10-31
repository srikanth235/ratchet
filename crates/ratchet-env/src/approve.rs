//! Writing an approved draft to the protected ref (`approve.ts`).

use std::path::PathBuf;
use std::sync::Arc;

use ratchet_core::config::DEFAULT_ENVIRONMENT_INFRA_DIR;
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::numfmt::to_fixed;
use ratchet_core::ports::{CommitFile, CommitFilesInput, GitHost, PortError, PullRequestInput};

use crate::error::{EnvError, EnvResult};
use crate::types::{
    EnvironmentApproval, EnvironmentApprovalMode, EnvironmentDraft, EnvironmentDraftRecord,
    draft_passed,
};

fn usd(n: f64) -> String {
    format!("${}", to_fixed(n, 2))
}

/// The pull request description of an approved environment: what it is, what it costs, what Ratchet checked, what
/// changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPullRequest {
    pub title: String,
    pub body: String,
    pub commit_message: String,
}

pub fn build_environment_pull_request(
    goal: &str,
    draft_id: &str,
    draft: &EnvironmentDraft,
) -> EnvironmentPullRequest {
    let goal = goal.trim();
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Ratchet environment for: {goal}"));
    lines.push(String::new());
    lines.push(format!("**Scope: {}.** {}", draft.scope, draft.reason));
    lines.push(String::new());
    if !draft.resources.is_empty() {
        lines.push("| What gets created | Machine | $/h |".into());
        lines.push("|---|---|---|".into());
        for r in &draft.resources {
            lines.push(format!(
                "| {}: {} | {} | {} |",
                r.name,
                r.detail,
                r.machine,
                usd(r.usd_per_hour)
            ));
        }
        lines.push(String::new());
    }
    lines.push(format!(
        "**Cost:** {}/h, ready in ~{} min, about {} for the run's time limit.",
        usd(draft.cost.usd_per_hour),
        draft.cost.startup_minutes,
        usd(draft.cost.estimate_for_run_usd)
    ));
    lines.push(String::new());
    lines.push("**Ratchet's checks**".into());
    for c in &draft.checks {
        lines.push(format!(
            "- {}: {}. {}",
            if c.ok { "passed" } else { "FAILED" },
            c.label,
            c.detail
        ));
    }
    lines.push(String::new());
    lines.push("**Files**".into());
    for f in &draft.files {
        lines.push(format!("- `{}`", f.path));
    }
    lines.push(String::new());
    lines.push(format!(
        "Once this is on the protected ref the environment is locked: `{DEFAULT_ENVIRONMENT_INFRA_DIR}/**` and `ratchet.yaml` join Ratchet's protected paths and any candidate that touches them is blocked. Agents change code only."
    ));
    lines.push(String::new());
    lines.push(format!(
        "Drafted by {} in {} s (draft {draft_id}).",
        draft.agent, draft.duration_s
    ));
    let what = match draft.scope {
        EnvironmentScope::None => "no extra infrastructure",
        EnvironmentScope::Single => "one pinned machine",
        EnvironmentScope::Full => "full-stack environment",
    };
    let short: String = goal.chars().take(60).collect();
    EnvironmentPullRequest {
        title: format!("Ratchet: {what} for \"{short}\""),
        body: lines.join("\n"),
        commit_message: format!("Ratchet: {} environment\n\n{}", draft.scope, draft.reason),
    }
}

pub struct CommitDraftOptions {
    /// `Commit`: straight onto `base_ref`. `Pr`: a branch and a pull request the user merges. Default `Pr`.
    pub mode: Option<EnvironmentApprovalMode>,
    /// Refuse when the base ref's ratchet.yaml is not this one (the draft carries a whole ratchet.yaml).
    pub expected_config_yaml: Option<String>,
    pub config_path: String,
    pub scratch_dir: Option<PathBuf>,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Suffix for the PR branch (default: the draft id).
    pub branch_suffix: String,
}

fn to_env(e: PortError) -> EnvError {
    match e {
        PortError::Unsupported(_) => EnvError::unavailable(
            "this git host cannot commit files, so the environment cannot be written to the repository",
        ),
        other => other.into(),
    }
}

/// Writes an approved draft to the protected ref: straight onto `base_ref` (`Commit`) or on a branch with a pull
/// request the user merges (`Pr`, the default). Refuses a draft whose checks did not all pass, and (when
/// `expected_config_yaml` is given) a base whose ratchet.yaml changed since the draft was made, because the draft
/// carries a whole ratchet.yaml.
pub async fn commit_draft(
    git: &dyn GitHost,
    record: &EnvironmentDraftRecord,
    opts: CommitDraftOptions,
) -> EnvResult<EnvironmentApproval> {
    let Some(draft) = record.draft.as_ref() else {
        return Err(EnvError::invalid_state(format!(
            "draft {} has nothing to approve",
            record.id
        )));
    };
    if !draft_passed(draft) {
        let failed: Vec<&str> = draft
            .checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.label.as_str())
            .collect();
        return Err(EnvError::invalid_state(format!(
            "draft {} cannot be approved: {}",
            record.id,
            if failed.is_empty() {
                "it has no checks".to_owned()
            } else {
                format!("these checks did not pass: {}", failed.join(", "))
            }
        )));
    }
    let mode = opts.mode.unwrap_or_default();

    if let Some(expected) = &opts.expected_config_yaml {
        let scratch = match &opts.scratch_dir {
            Some(d) => {
                tokio::fs::create_dir_all(d).await?;
                tempfile::Builder::new()
                    .prefix("ratchet-approve-")
                    .tempdir_in(d)?
            }
            None => tempfile::Builder::new()
                .prefix("ratchet-approve-")
                .tempdir()?,
        };
        git.checkout(&record.base_ref, scratch.path())
            .await
            .map_err(EnvError::from)?;
        let now = tokio::fs::read_to_string(scratch.path().join(&opts.config_path))
            .await
            .ok();
        if now.as_deref() != Some(expected.as_str()) {
            return Err(EnvError::invalid_state(format!(
                "{} changed on {} since this environment was drafted; draft it again so it is based on the current file",
                opts.config_path, record.base_ref
            )));
        }
    }

    let pr = build_environment_pull_request(&record.goal, &record.id, draft);
    let files: Vec<CommitFile> = draft
        .files
        .iter()
        .map(|f| CommitFile::new(f.path.clone(), f.content.clone()))
        .collect();
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    if mode == EnvironmentApprovalMode::Commit {
        let sha = git
            .commit_files(&CommitFilesInput {
                base_ref: record.base_ref.clone(),
                branch: record.base_ref.clone(),
                message: pr.commit_message.clone(),
                files,
                update: true,
            })
            .await
            .map_err(to_env)?
            .sha;
        return Ok(EnvironmentApproval {
            mode,
            sha,
            branch: None,
            pr_url: None,
            files: paths,
            locked: true,
            at: (opts.now)(),
        });
    }
    let commit = |branch: &str| CommitFilesInput {
        base_ref: record.base_ref.clone(),
        branch: branch.to_owned(),
        message: pr.commit_message.clone(),
        files: files.clone(),
        update: false,
    };
    let mut branch = format!("ratchet/environment-{}", opts.branch_suffix);
    let sha = match git.commit_files(&commit(&branch)).await {
        Ok(r) => r.sha,
        Err(first) => {
            if matches!(first, PortError::Unsupported(_)) {
                return Err(to_env(first));
            }
            // most likely the branch exists from an earlier attempt: take a fresh name once, then give up
            branch = format!("{branch}-{}", to_base36((opts.now)()));
            match git.commit_files(&commit(&branch)).await {
                Ok(r) => r.sha,
                Err(_) => return Err(to_env(first)),
            }
        }
    };
    let opened = git
        .open_pull_request(&PullRequestInput {
            base: record.base_ref.clone(),
            head: branch.clone(),
            title: pr.title.clone(),
            body: pr.body.clone(),
            shas: vec![],
            commit_message: None,
        })
        .await
        .map_err(to_env)?;
    Ok(EnvironmentApproval {
        mode,
        sha,
        branch: Some(branch),
        pr_url: Some(opened.url),
        files: paths,
        locked: true,
        at: (opts.now)(),
    })
}

/// `Number.prototype.toString(36)` of a non-negative whole number.
pub(crate) fn to_base36(mut n: i64) -> String {
    if n <= 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}
