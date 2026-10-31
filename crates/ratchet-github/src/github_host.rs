//! Same git operations as `LocalGitHost` against a GitHub remote. Reads go through a local bare mirror
//! (`refs/remotes/origin/*`); pushes and the PR use the token. The token only ever travels in the environment
//! (`GIT_CONFIG_*` extra header) or in the REST Authorization header, never in argv or a URL.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use regex::Regex;
use serde::Deserialize;
use tokio::sync::OnceCell;

use crate::error::{Error, Result};
use crate::git::{
    BaseGitHostOptions, CommandGitRunner, Git, GitBackend, GitHostCore, GitRunOptions, GitRunner,
};
use crate::http::{DEFAULT_API_URL, authed, default_client, error_message, send, trim_base};
use ratchet_core::ports::{PullRequestInput, PullRequestRef};

#[derive(Clone)]
pub struct GitHubGitHostOptions {
    /// `owner/name`.
    pub repo: String,
    /// Repo-scoped token (fine-grained PAT or GitHub App installation token). Never logged, never in argv.
    pub token: String,
    /// Local bare mirror used to read branches; defaults to a temp dir.
    pub cache_dir: Option<PathBuf>,
    /// Git host name, for GitHub Enterprise. Default `github.com`.
    pub host: Option<String>,
    /// REST API base URL, for GitHub Enterprise and tests. Default `https://api.github.com`.
    pub api_url: Option<String>,
    /// HTTP client for the REST API (tests inject one that bypasses proxies).
    pub http: Option<reqwest::Client>,
    pub base: BaseGitHostOptions,
}

impl GitHubGitHostOptions {
    pub fn new(repo: impl Into<String>, token: impl Into<String>) -> Self {
        GitHubGitHostOptions {
            repo: repo.into(),
            token: token.into(),
            cache_dir: None,
            host: None,
            api_url: None,
            http: None,
            base: BaseGitHostOptions::default(),
        }
    }
}

pub struct GitHubBackend {
    owner: String,
    name: String,
    url: String,
    host: String,
    token: String,
    cache_dir: PathBuf,
    api_url: String,
    http: reqwest::Client,
    ready: OnceCell<()>,
}

/// A GitHub remote: bare mirror for reads, `git push` and the REST API for writes.
pub type GitHubGitHost = GitHostCore<GitHubBackend>;

pub(crate) fn basic_auth(token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"))
}

/// Split `owner/name` (an optional `.git` suffix is dropped).
pub fn parse_repo(repo: &str) -> Result<(String, String)> {
    let re = Regex::new(r"^([\w.-]+)/([\w.-]+?)(?:\.git)?$").expect("static regex");
    let m = re.captures(repo).ok_or_else(|| {
        Error::invalid(format!(
            "repo must look like \"owner/name\", got \"{repo}\""
        ))
    })?;
    Ok((m[1].to_string(), m[2].to_string()))
}

impl GitHubGitHost {
    pub fn new(options: GitHubGitHostOptions) -> Result<Self> {
        let token = options.token.clone();
        if token.is_empty() {
            return Err(Error::invalid("GitHubGitHost requires a token"));
        }
        let (owner, name) = parse_repo(&options.repo)?;
        let host = options.host.clone().unwrap_or_else(|| "github.com".into());
        let cache_dir = match options.cache_dir.clone() {
            Some(d) => d,
            None => tempfile::Builder::new()
                .prefix("ratchet-gh-")
                .tempdir()?
                .keep()
                .join("mirror.git"),
        };
        let runner: Arc<dyn GitRunner> = options.base.runner.clone().unwrap_or_else(|| {
            Arc::new(CommandGitRunner::new([token.clone(), basic_auth(&token)]))
        });
        let backend = GitHubBackend {
            url: format!("https://{host}/{owner}/{name}.git"),
            owner,
            name,
            host,
            token,
            cache_dir,
            api_url: trim_base(options.api_url.as_deref().unwrap_or(DEFAULT_API_URL)),
            http: options.http.clone().unwrap_or_else(default_client),
            ready: OnceCell::new(),
        };
        Ok(GitHostCore::from_backend(
            backend,
            runner,
            options.base.identity,
        ))
    }
}

impl GitHubBackend {
    /// Environment that authenticates git over HTTPS without putting the token in argv or the URL.
    fn auth_env(&self) -> GitRunOptions {
        GitRunOptions::default()
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("http.https://{}/.extraheader", self.host),
            )
            .env(
                "GIT_CONFIG_VALUE_0",
                format!("AUTHORIZATION: basic {}", basic_auth(&self.token)),
            )
    }

    fn pulls_url(&self) -> String {
        format!("{}/repos/{}/{}/pulls", self.api_url, self.owner, self.name)
    }

    async fn create(&self, input: &PullRequestInput) -> Result<PullRequestRef> {
        let req = self
            .http
            .post(self.pulls_url())
            .json(&serde_json::json!({ "title": input.title, "head": input.head, "base": input.base, "body": input.body }));
        let res = send(authed(req, &self.token), &self.token).await?;
        let status = res.status();
        if !status.is_success() {
            return Err(Error::http(
                Some(status.as_u16()),
                error_message(res, &self.token).await,
            ));
        }
        let pr: Pr = res
            .json()
            .await
            .map_err(|e| Error::http(Some(status.as_u16()), format!("unreadable answer: {e}")))?;
        Ok(PullRequestRef { url: pr.html_url })
    }

    async fn find_open(&self, input: &PullRequestInput) -> Result<Option<Pr>> {
        let req = self.http.get(self.pulls_url()).query(&[
            ("head", format!("{}:{}", self.owner, input.head)),
            ("base", input.base.clone()),
            ("state", "open".to_string()),
        ]);
        let res = send(authed(req, &self.token), &self.token).await?;
        let status = res.status();
        if !status.is_success() {
            return Err(Error::http(
                Some(status.as_u16()),
                error_message(res, &self.token).await,
            ));
        }
        let list: Vec<Pr> = res
            .json()
            .await
            .map_err(|e| Error::http(Some(status.as_u16()), format!("unreadable answer: {e}")))?;
        Ok(list.into_iter().next())
    }

    async fn update(&self, number: u64, input: &PullRequestInput) -> Result<PullRequestRef> {
        let req = self
            .http
            .patch(format!("{}/{number}", self.pulls_url()))
            .json(&serde_json::json!({ "title": input.title, "body": input.body }));
        let res = send(authed(req, &self.token), &self.token).await?;
        let status = res.status();
        if !status.is_success() {
            return Err(Error::http(
                Some(status.as_u16()),
                error_message(res, &self.token).await,
            ));
        }
        let pr: Pr = res
            .json()
            .await
            .map_err(|e| Error::http(Some(status.as_u16()), format!("unreadable answer: {e}")))?;
        Ok(PullRequestRef { url: pr.html_url })
    }
}

#[derive(Deserialize)]
struct Pr {
    html_url: String,
    #[serde(default)]
    number: u64,
}

#[async_trait]
impl GitBackend for GitHubBackend {
    fn remote_url(&self) -> String {
        self.url.clone()
    }

    async fn git_dir(&self, runner: &Arc<dyn GitRunner>) -> Result<PathBuf> {
        self.ready
            .get_or_try_init(|| async {
                if !self.cache_dir.join("HEAD").exists() {
                    tokio::fs::create_dir_all(&self.cache_dir).await?;
                    let args = [
                        "init".to_string(),
                        "--bare".into(),
                        "-q".into(),
                        self.cache_dir.to_string_lossy().into_owned(),
                    ];
                    runner.run(&args, &GitRunOptions::default()).await?;
                }
                Ok::<(), Error>(())
            })
            .await?;
        Ok(self.cache_dir.clone())
    }

    fn branch_prefix(&self) -> &'static str {
        "refs/remotes/origin/"
    }

    async fn sync(&self, git: &Git<'_>) -> Result<()> {
        git.run(
            &[
                "fetch",
                "--prune",
                "--no-tags",
                "-q",
                &self.url,
                "+refs/heads/*:refs/remotes/origin/*",
            ],
            self.auth_env(),
        )
        .await?;
        Ok(())
    }

    async fn publish(&self, git: &Git<'_>, branch: &str, sha: &str, force: bool) -> Result<()> {
        let refspec = format!("{}{sha}:refs/heads/{branch}", if force { "+" } else { "" });
        git.run(&["push", "-q", &self.url, &refspec], self.auth_env())
            .await?;
        // keep the mirror consistent with what we just pushed
        git.run(
            &["update-ref", &format!("refs/remotes/origin/{branch}"), sha],
            GitRunOptions::default(),
        )
        .await?;
        Ok(())
    }

    async fn create_pull_request(&self, input: &PullRequestInput) -> Result<PullRequestRef> {
        match self.create(input).await {
            Ok(pr) => Ok(pr),
            Err(err) if err.status() == Some(422) => {
                // most likely "a pull request already exists for <head>": refresh it instead
                match self.find_open(input).await? {
                    Some(existing) => self.update(existing.number, input).await,
                    None => Err(err),
                }
            }
            Err(err) => Err(err),
        }
    }
}
