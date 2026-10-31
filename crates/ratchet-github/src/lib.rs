//! GitHub for Ratchet: the Rust port of `packages/github` plus the GitHub parts of the desktop main process.
//!
//! - [`GitHost`] (the port) with three implementations: [`LocalGitHost`] (simulation: git CLI against a local bare repo),
//!   [`GitHubGitHost`] (a GitHub remote: bare mirror + push + REST pull requests) and [`MockGitHost`] (in-memory).
//! - [`GithubApi`]: device-flow sign-in, repos, branches, files (real [`HttpGithubApi`], in-memory [`MockGithubApi`]).
//! - [`GithubSignIn`]: the background device-flow poller.
//! - [`check_github_access`] (token validation), [`build_squash_plan`], [`build_result_report`], [`slug_of`],
//!   [`inspect_local_folder`].
//!
//! # HTTP stack: reqwest, not octocrab
//!
//! The architecture names octocrab. This crate uses `reqwest` + `serde` instead, for the handful of calls it needs:
//! octocrab's device flow targets `https://github.com` fixed in its API (the app and the e2e fake must override the web
//! base), its error types hide the status/message mapping the app shows verbatim, and it pulls in a large tree (hyper,
//! tower, jsonwebtoken) for ~8 REST endpoints. Every base URL here is overridable (`RATCHET_GITHUB_API_URL`,
//! `RATCHET_GITHUB_WEB_URL`). Switching to octocrab later only touches [`api`], [`access`] and the PR calls in
//! [`github_host`].

pub mod access;
pub mod api;
pub mod domain;
pub mod error;
pub mod git;
pub mod github_host;
pub mod http;
pub mod local;
pub mod local_repo;
pub mod mock;
pub mod numfmt;
pub mod port;
pub mod report;
pub mod signin;
pub mod slug;
pub mod squash;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use access::{CheckGitHubAccessOptions, GitHubAccess, check_github_access};
pub use api::{
    DeviceCode, DevicePoll, GITHUB_SCOPE, GithubApi, GithubApiOptions, GithubUser, HttpGithubApi,
    MockGithubApi, PLACEHOLDER_GITHUB_CLIENT_ID, RepoInspection, inspect_github_repo,
};
pub use domain::{
    AgentKind, BenchStats, CandidateCommit, CandidateVerdict, CommitFile, CommitFilesInput,
    Direction, FileDiff, PullRequestInput, PullRequestUrl, Repo, gain_percent,
};
pub use error::{Error, Result};
pub use git::{
    BaseGitHostOptions, CANDIDATE_TRAILER, CommandGitRunner, GitHostCore, GitRunOptions, GitRunner,
    Identity, parse_git_diff,
};
pub use github_host::{GitHubGitHost, GitHubGitHostOptions};
pub use local::{
    BareRemote, CreateBareRemoteOptions, LocalGitHost, LocalGitHostOptions, create_bare_remote,
};
pub use local_repo::{LocalRepoInfo, inspect_local_folder};
pub use mock::MockGitHost;
pub use port::GitHost;
pub use report::{
    ReportInput, ResultReport, ResultReportData, build_result_report,
    build_squash_plan_from_report_input,
};
pub use signin::{AccountStore, FlowState, GithubFlow, GithubSignIn, SignInDeps};
pub use slug::{friendly_missing, github_slug, slug_of};
pub use squash::{SquashPlan, SquashPlanInput, SquashWin, build_squash_plan, subject_of};
