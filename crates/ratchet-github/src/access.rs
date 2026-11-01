//! Token validation: can this token see the repo, and may it push?

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, scrub};
use crate::github_host::parse_repo;
use crate::http::{DEFAULT_API_URL, authed, default_client, error_message, send, trim_base};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccess {
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
    /// The token can push branches (needed for candidate branches and the PR).
    pub can_push: bool,
}

#[derive(Clone)]
pub struct CheckGitHubAccessOptions {
    /// `owner/name`.
    pub repo: String,
    pub token: String,
    /// REST API base URL, for GitHub Enterprise and tests.
    pub api_url: Option<String>,
    pub http: Option<reqwest::Client>,
}

impl CheckGitHubAccessOptions {
    pub fn new(repo: impl Into<String>, token: impl Into<String>) -> Self {
        CheckGitHubAccessOptions {
            repo: repo.into(),
            token: token.into(),
            api_url: None,
            http: None,
        }
    }
}

#[derive(Deserialize)]
struct ApiRepoAccess {
    full_name: String,
    default_branch: String,
    #[serde(default)]
    private: bool,
    permissions: Option<Permissions>,
}

#[derive(Deserialize)]
struct Permissions {
    push: Option<bool>,
    admin: Option<bool>,
}

/// Read-only check that `token` can see `repo` and may push to it (one `GET /repos/{owner}/{name}`). The token is sent
/// only in the Authorization header and never appears in a returned error.
pub async fn check_github_access(opts: CheckGitHubAccessOptions) -> Result<GitHubAccess> {
    let (owner, name) = parse_repo(&opts.repo)?;
    if opts.token.is_empty() {
        return Err(Error::invalid("a GitHub token is required"));
    }
    let http = opts.http.clone().unwrap_or_else(default_client);
    let api = trim_base(opts.api_url.as_deref().unwrap_or(DEFAULT_API_URL));
    let res = match send(
        authed(http.get(format!("{api}/repos/{owner}/{name}")), &opts.token),
        &opts.token,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err(Error::http(
                None,
                format!(
                    "GitHub request failed: {}",
                    scrub(&e.to_string(), &opts.token)
                ),
            ));
        }
    };
    let status = res.status();
    match status.as_u16() {
        200..=299 => {}
        401 => {
            return Err(Error::http(
                Some(401),
                "GitHub rejected the token (401): it is wrong, expired or revoked",
            ));
        }
        404 => {
            return Err(Error::http(
                Some(404),
                format!("repository {owner}/{name} not found, or the token cannot see it (404)"),
            ));
        }
        code => {
            let message = error_message(res, &opts.token).await;
            return Err(Error::http(
                Some(code),
                format!("GitHub request failed ({code}): {message}"),
            ));
        }
    }
    let data: ApiRepoAccess = res.json().await.map_err(|e| {
        Error::http(
            Some(status.as_u16()),
            format!(
                "GitHub request failed: unreadable answer: {}",
                scrub(&e.to_string(), &opts.token)
            ),
        )
    })?;
    let can_push = data
        .permissions
        .is_some_and(|p| p.push == Some(true) || p.admin == Some(true));
    Ok(GitHubAccess {
        full_name: data.full_name,
        default_branch: data.default_branch,
        private: data.private,
        can_push,
    })
}
