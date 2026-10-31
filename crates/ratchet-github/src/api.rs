//! GitHub for the signed-in account: the OAuth device flow, the account's repositories, branches and a file on a branch.
//!
//! Plain `reqwest` against the REST API (ported from `packages/desktop/src/main/github.ts`). A token only ever travels
//! in the Authorization header and never appears in an error message.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;
use base64::Engine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::Repo;
use crate::error::{Error, Result, scrub};
use crate::http::{
    DEFAULT_API_URL, DEFAULT_WEB_URL, authed, default_client, describe, send, trim_base,
};

/// Placeholder OAuth app client id. Register an OAuth app with the device flow enabled and set
/// `RATCHET_GITHUB_CLIENT_ID`.
pub const PLACEHOLDER_GITHUB_CLIENT_ID: &str = "Ov23liRatchetPlaceholder";
/// `repo` lets Ratchet push candidate branches and open pull requests (including on private repositories).
pub const GITHUB_SCOPE: &str = "repo";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until the code expires.
    pub expires_in: u64,
    /// Minimum seconds between polls.
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DevicePoll {
    Ok { token: String },
    Pending,
    SlowDown { interval: u64 },
    Expired,
    Denied,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubUser {
    pub login: String,
}

#[async_trait]
pub trait GithubApi: Send + Sync {
    /// True when a real client id is configured (not the placeholder).
    fn client_id_configured(&self) -> bool;
    async fn device_start(&self) -> Result<DeviceCode>;
    async fn device_poll(&self, device_code: &str) -> Result<DevicePoll>;
    async fn user(&self, token: &str) -> Result<GithubUser>;
    /// The account's repositories, most recently pushed first, filtered by `query` (substring of owner/name or
    /// description).
    async fn repos(&self, token: &str, query: Option<&str>) -> Result<Vec<Repo>>;
    async fn repo(&self, token: &str, full_name: &str) -> Result<Repo>;
    /// Text of `path` at `git_ref`, or `None` when the file does not exist.
    async fn file(
        &self,
        token: &str,
        full_name: &str,
        git_ref: &str,
        path: &str,
    ) -> Result<Option<String>>;
    /// Branch names of a repository (first 300). Not in the TS port; the New optimization flow needs it.
    async fn branches(&self, token: &str, full_name: &str) -> Result<Vec<String>>;
}

#[derive(Clone, Default)]
pub struct GithubApiOptions {
    pub client_id: Option<String>,
    /// REST API base (GitHub Enterprise, tests). Default `https://api.github.com`.
    pub api_url: Option<String>,
    /// Web base for the device flow. Default `https://github.com`.
    pub web_url: Option<String>,
    pub http: Option<reqwest::Client>,
}

impl GithubApiOptions {
    /// Options from the environment the desktop app read: `RATCHET_GITHUB_CLIENT_ID`, `RATCHET_GITHUB_API_URL`,
    /// `RATCHET_GITHUB_WEB_URL` (GitHub Enterprise, and the fake GitHub the end-to-end tests run against).
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        GithubApiOptions {
            client_id: var("RATCHET_GITHUB_CLIENT_ID"),
            api_url: var("RATCHET_GITHUB_API_URL"),
            web_url: var("RATCHET_GITHUB_WEB_URL"),
            http: None,
        }
    }
}

#[derive(Deserialize)]
struct ApiRepo {
    full_name: String,
    default_branch: String,
    private: bool,
    description: Option<String>,
    pushed_at: Option<String>,
}

impl From<ApiRepo> for Repo {
    fn from(r: ApiRepo) -> Self {
        Repo {
            full_name: r.full_name,
            default_branch: r.default_branch,
            private: r.private,
            description: r.description,
            pushed_at: r.pushed_at,
        }
    }
}

/// `encodeURIComponent`
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

fn encode_component(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}

/// The real GitHub (or GitHub Enterprise, or a fake) over HTTP.
pub struct HttpGithubApi {
    http: reqwest::Client,
    api: String,
    web: String,
    client_id: String,
    client_id_configured: bool,
}

impl HttpGithubApi {
    pub fn new(opts: GithubApiOptions) -> Self {
        let client_id = opts
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(PLACEHOLDER_GITHUB_CLIENT_ID)
            .to_string();
        HttpGithubApi {
            http: opts.http.unwrap_or_else(default_client),
            api: trim_base(opts.api_url.as_deref().unwrap_or(DEFAULT_API_URL)),
            web: trim_base(opts.web_url.as_deref().unwrap_or(DEFAULT_WEB_URL)),
            client_id_configured: client_id != PLACEHOLDER_GITHUB_CLIENT_ID,
            client_id,
        }
    }

    async fn get(&self, token: &str, path: &str) -> Result<reqwest::Response> {
        send(
            authed(self.http.get(format!("{}{path}", self.api)), token),
            token,
        )
        .await
    }

    fn fail(status: u16, what: &str) -> Error {
        match status {
            401 => Error::http(
                Some(401),
                "GitHub rejected the sign-in (401): sign in again in Settings.",
            ),
            403 => Error::http(
                Some(403),
                format!(
                    "GitHub refused {what} (403): the token may lack access, or the rate limit was hit."
                ),
            ),
            404 => Error::http(
                Some(404),
                format!("{what}: not found, or the account cannot see it (404)."),
            ),
            _ => Error::http(Some(status), format!("GitHub {what} failed ({status}).")),
        }
    }

    async fn form(&self, url: &str, body: &[(&str, &str)]) -> Result<Value> {
        let res = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("User-Agent", "Ratchet")
            .form(body)
            .send()
            .await
            .map_err(|e| Error::http(None, format!("Could not reach GitHub: {}", describe(&e))))?;
        let status = res.status();
        if !status.is_success() && status.as_u16() != 400 {
            let hint = if status.as_u16() == 404 {
                " (is the OAuth client id right? set RATCHET_GITHUB_CLIENT_ID)"
            } else {
                ""
            };
            return Err(Error::http(
                Some(status.as_u16()),
                format!("GitHub answered {}{hint}.", status.as_u16()),
            ));
        }
        res.json::<Value>().await.map_err(|e| {
            Error::http(
                Some(status.as_u16()),
                format!("GitHub answered something unreadable: {}", describe(&e)),
            )
        })
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "undefined".into(),
        other => other.to_string(),
    }
}

fn number(v: Option<&Value>, default: u64) -> u64 {
    match v {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .unwrap_or(default),
        Some(Value::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

#[async_trait]
impl GithubApi for HttpGithubApi {
    fn client_id_configured(&self) -> bool {
        self.client_id_configured
    }

    async fn device_start(&self) -> Result<DeviceCode> {
        let j = self
            .form(
                &format!("{}/login/device/code", self.web),
                &[("client_id", &self.client_id), ("scope", GITHUB_SCOPE)],
            )
            .await?;
        if let Some(Value::String(err)) = j.get("error") {
            let msg = j
                .get("error_description")
                .and_then(Value::as_str)
                .unwrap_or(err);
            return Err(Error::http(None, format!("GitHub: {msg}")));
        }
        Ok(DeviceCode {
            device_code: text(j.get("device_code").unwrap_or(&Value::Null)),
            user_code: text(j.get("user_code").unwrap_or(&Value::Null)),
            verification_uri: j
                .get("verification_uri")
                .map(text)
                .unwrap_or_else(|| format!("{}/login/device", self.web)),
            expires_in: number(j.get("expires_in"), 900),
            interval: number(j.get("interval"), 5),
        })
    }

    async fn device_poll(&self, device_code: &str) -> Result<DevicePoll> {
        let j = self
            .form(
                &format!("{}/login/oauth/access_token", self.web),
                &[
                    ("client_id", &self.client_id),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ],
            )
            .await?;
        if let Some(Value::String(token)) = j.get("access_token") {
            return Ok(DevicePoll::Ok {
                token: token.clone(),
            });
        }
        Ok(match j.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => DevicePoll::Pending,
            Some("slow_down") => DevicePoll::SlowDown {
                interval: number(j.get("interval"), 10),
            },
            Some("expired_token") => DevicePoll::Expired,
            Some("access_denied") => DevicePoll::Denied,
            other => DevicePoll::Error {
                message: j
                    .get("error_description")
                    .and_then(Value::as_str)
                    .or(other)
                    .unwrap_or("unexpected answer")
                    .to_string(),
            },
        })
    }

    async fn user(&self, token: &str) -> Result<GithubUser> {
        let res = self.get(token, "/user").await?;
        if !res.status().is_success() {
            return Err(Self::fail(res.status().as_u16(), "reading the account"));
        }
        let v: Value = res.json().await.map_err(|e| {
            Error::http(
                None,
                format!(
                    "GitHub answered something unreadable: {}",
                    scrub(&e.to_string(), token)
                ),
            )
        })?;
        Ok(GithubUser {
            login: text(v.get("login").unwrap_or(&Value::Null)),
        })
    }

    async fn repos(&self, token: &str, query: Option<&str>) -> Result<Vec<Repo>> {
        let mut out: Vec<Repo> = Vec::new();
        for page in 1..=3 {
            let res = self
                .get(token, &format!("/user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member&page={page}"))
                .await?;
            if !res.status().is_success() {
                return Err(Self::fail(res.status().as_u16(), "listing repositories"));
            }
            let rows: Vec<ApiRepo> = res.json().await.map_err(|e| {
                Error::http(
                    None,
                    format!(
                        "GitHub answered something unreadable: {}",
                        scrub(&e.to_string(), token)
                    ),
                )
            })?;
            let n = rows.len();
            out.extend(rows.into_iter().map(Repo::from));
            if n < 100 {
                break;
            }
        }
        let q = query
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty());
        Ok(match q {
            Some(q) => out
                .into_iter()
                .filter(|r| {
                    r.full_name.to_lowercase().contains(&q)
                        || r.description
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&q)
                })
                .collect(),
            None => out,
        })
    }

    async fn repo(&self, token: &str, full_name: &str) -> Result<Repo> {
        let res = self.get(token, &format!("/repos/{full_name}")).await?;
        if !res.status().is_success() {
            return Err(Self::fail(
                res.status().as_u16(),
                &format!("reading {full_name}"),
            ));
        }
        let r: ApiRepo = res.json().await.map_err(|e| {
            Error::http(
                None,
                format!(
                    "GitHub answered something unreadable: {}",
                    scrub(&e.to_string(), token)
                ),
            )
        })?;
        Ok(r.into())
    }

    async fn file(
        &self,
        token: &str,
        full_name: &str,
        git_ref: &str,
        path: &str,
    ) -> Result<Option<String>> {
        let encoded = path
            .split('/')
            .map(encode_component)
            .collect::<Vec<_>>()
            .join("/");
        let res = self
            .get(
                token,
                &format!(
                    "/repos/{full_name}/contents/{encoded}?ref={}",
                    encode_component(git_ref)
                ),
            )
            .await?;
        if res.status().as_u16() == 404 {
            return Ok(None);
        }
        if !res.status().is_success() {
            return Err(Self::fail(
                res.status().as_u16(),
                &format!("reading {path}"),
            ));
        }
        let j: Value = res.json().await.map_err(|e| {
            Error::http(
                None,
                format!(
                    "GitHub answered something unreadable: {}",
                    scrub(&e.to_string(), token)
                ),
            )
        })?;
        let Some(content) = j.get("content").and_then(Value::as_str) else {
            return Ok(None);
        };
        if j.get("encoding").and_then(Value::as_str) == Some("base64") {
            // GitHub wraps base64 at 60 columns
            let compact: String = content.chars().filter(|c| !c.is_whitespace()).collect();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(compact)
                .map_err(|e| {
                    Error::http(
                        None,
                        format!("GitHub sent undecodable content for {path}: {e}"),
                    )
                })?;
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        } else {
            Ok(Some(content.to_string()))
        }
    }

    async fn branches(&self, token: &str, full_name: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for page in 1..=3 {
            let res = self
                .get(
                    token,
                    &format!("/repos/{full_name}/branches?per_page=100&page={page}"),
                )
                .await?;
            if !res.status().is_success() {
                return Err(Self::fail(
                    res.status().as_u16(),
                    &format!("listing branches of {full_name}"),
                ));
            }
            let rows: Vec<Value> = res.json().await.map_err(|e| {
                Error::http(
                    None,
                    format!(
                        "GitHub answered something unreadable: {}",
                        scrub(&e.to_string(), token)
                    ),
                )
            })?;
            let n = rows.len();
            out.extend(
                rows.iter()
                    .filter_map(|b| b.get("name").and_then(Value::as_str).map(String::from)),
            );
            if n < 100 {
                break;
            }
        }
        Ok(out)
    }
}

/// A repo and its `ratchet.yaml` on the default branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInspection {
    pub repo: Repo,
    pub config_yaml: Option<String>,
}

/// What the New optimization flow shows for a GitHub repo: the repo plus `ratchet.yaml` read from its default branch.
pub async fn inspect_github_repo(
    api: &dyn GithubApi,
    token: &str,
    full_name: &str,
) -> Result<RepoInspection> {
    let repo = api.repo(token, full_name).await?;
    let config_yaml = api
        .file(token, &repo.full_name, &repo.default_branch, "ratchet.yaml")
        .await?;
    Ok(RepoInspection { repo, config_yaml })
}

// ------------------------------------------------------------------ the mock

/// Scriptable state of [`MockGithubApi`] (the TS `FakeGithub`).
pub struct MockGithubState {
    pub client_id_configured: bool,
    pub repo_list: Vec<Repo>,
    /// `owner/name@ref:path` -> text
    pub files: HashMap<String, String>,
    pub branches: HashMap<String, Vec<String>>,
    pub polls: VecDeque<DevicePoll>,
    pub login: String,
    pub user_fails: bool,
    pub tokens_seen: Vec<String>,
    pub started: u32,
}

/// An in-memory `GithubApi` for simulation and tests: no network.
pub struct MockGithubApi {
    pub state: Mutex<MockGithubState>,
}

impl Default for MockGithubApi {
    fn default() -> Self {
        MockGithubApi::new()
    }
}

impl MockGithubApi {
    pub fn new() -> Self {
        let repo =
            |name: &str, branch: &str, private: bool, description: Option<&str>, pushed: &str| {
                Repo {
                    full_name: name.into(),
                    default_branch: branch.into(),
                    private,
                    description: description.map(String::from),
                    pushed_at: Some(pushed.into()),
                }
            };
        MockGithubApi {
            state: Mutex::new(MockGithubState {
                client_id_configured: true,
                repo_list: vec![
                    repo(
                        "acme/api",
                        "main",
                        true,
                        Some("checkout service"),
                        "2026-01-02T00:00:00Z",
                    ),
                    repo("acme/web", "trunk", false, None, "2026-01-01T00:00:00Z"),
                ],
                files: HashMap::new(),
                branches: HashMap::new(),
                polls: VecDeque::new(),
                login: "octocat".into(),
                user_fails: false,
                tokens_seen: Vec::new(),
                started: 0,
            }),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut MockGithubState) -> R) -> R {
        f(&mut self.state.lock().expect("mock state"))
    }
}

#[async_trait]
impl GithubApi for MockGithubApi {
    fn client_id_configured(&self) -> bool {
        self.with(|s| s.client_id_configured)
    }

    async fn device_start(&self) -> Result<DeviceCode> {
        self.with(|s| s.started += 1);
        Ok(DeviceCode {
            device_code: "dc".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 900,
            interval: 5,
        })
    }

    async fn device_poll(&self, _device_code: &str) -> Result<DevicePoll> {
        Ok(self
            .with(|s| s.polls.pop_front())
            .unwrap_or(DevicePoll::Pending))
    }

    async fn user(&self, token: &str) -> Result<GithubUser> {
        self.with(|s| {
            s.tokens_seen.push(token.to_string());
            if s.user_fails {
                Err(Error::http(
                    Some(401),
                    "GitHub rejected the sign-in (401): sign in again in Settings.",
                ))
            } else {
                Ok(GithubUser {
                    login: s.login.clone(),
                })
            }
        })
    }

    async fn repos(&self, token: &str, query: Option<&str>) -> Result<Vec<Repo>> {
        Ok(self.with(|s| {
            s.tokens_seen.push(token.to_string());
            let q = query.map(str::to_lowercase).filter(|q| !q.is_empty());
            match q {
                Some(q) => s
                    .repo_list
                    .iter()
                    .filter(|r| r.full_name.contains(&q))
                    .cloned()
                    .collect(),
                None => s.repo_list.clone(),
            }
        }))
    }

    async fn repo(&self, token: &str, full_name: &str) -> Result<Repo> {
        self.with(|s| {
            s.tokens_seen.push(token.to_string());
            s.repo_list
                .iter()
                .find(|r| r.full_name == full_name)
                .cloned()
                .ok_or_else(|| {
                    Error::http(
                        Some(404),
                        format!("{full_name}: not found, or the account cannot see it (404)."),
                    )
                })
        })
    }

    async fn file(
        &self,
        _token: &str,
        full_name: &str,
        git_ref: &str,
        path: &str,
    ) -> Result<Option<String>> {
        Ok(self.with(|s| {
            s.files
                .get(&format!("{full_name}@{git_ref}:{path}"))
                .cloned()
        }))
    }

    async fn branches(&self, _token: &str, full_name: &str) -> Result<Vec<String>> {
        self.with(|s| {
            if let Some(b) = s.branches.get(full_name) {
                return Ok(b.clone());
            }
            s.repo_list
                .iter()
                .find(|r| r.full_name == full_name)
                .map(|r| vec![r.default_branch.clone()])
                .ok_or_else(|| Error::http(Some(404), format!("listing branches of {full_name}: not found, or the account cannot see it (404).")))
        })
    }
}
