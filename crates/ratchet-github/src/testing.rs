//! Test support: an in-process fake GitHub server and a git runner that records (and rewrites) what it is asked to do.
//!
//! The fake speaks the slice of GitHub Ratchet uses and is a port of `startFakeGithub` from
//! `packages/e2e/harness/fakes.mjs` (device flow, `/user`, `/user/repos`, `/repos/{o}/{r}`, contents), plus branches and
//! pull requests. Point the code at it with `RATCHET_GITHUB_API_URL` / `RATCHET_GITHUB_WEB_URL` (see
//! [`FakeGithubServer::env`]) or by passing [`FakeGithubServer::base_url`] as `api_url` / `web_url`.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::error::Result;
use crate::git::{GitRunOptions, GitRunner};

pub const DEVICE_CODE: &str = "WDJB-MJHT";
pub const FAKE_LOGIN: &str = "e2e-octocat";
pub const FAKE_TOKEN: &str = "gho_E2EFAKETOKEN0123456789abcdefABCDEF";
pub const FAKE_CLIENT_ID: &str = "e2e-client-id";

/// A repository as `/user/repos` returns it (`full_name`, `default_branch`, `private`, `description`, `pushed_at`).
pub fn repo(
    full_name: &str,
    default_branch: &str,
    description: Option<&str>,
    private: bool,
) -> Value {
    json!({
        "full_name": full_name,
        "default_branch": default_branch,
        "private": private,
        "description": description,
        "pushed_at": "2026-10-01T00:00:00Z",
    })
}

/// Same, with the `permissions` object `GET /repos/{o}/{r}` carries for the authenticated user.
pub fn repo_with_permissions(
    full_name: &str,
    default_branch: &str,
    private: bool,
    push: bool,
) -> Value {
    let mut r = repo(full_name, default_branch, None, private);
    r["permissions"] = json!({ "pull": true, "push": push, "admin": false });
    r
}

#[derive(Debug, Clone, PartialEq)]
pub struct FakePull {
    pub number: u64,
    pub owner: String,
    pub repo: String,
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
}

#[derive(Default)]
pub struct FakeGithubConfig {
    /// What `/user/repos` lists (100 per page).
    pub repos: Vec<Value>,
    /// `"owner/name@ref:path"` -> text.
    pub files: HashMap<String, String>,
    /// `"owner/name"` -> branch names (default: just the repo's default branch).
    pub branches: HashMap<String, Vec<String>>,
}

#[derive(Default)]
struct Shared {
    config: FakeGithubConfig,
    approved: bool,
    polls: usize,
    requests: Vec<String>,
    /// Authorization headers seen on authenticated routes.
    authorizations: Vec<String>,
    /// Form bodies of the device-flow POSTs.
    forms: Vec<HashMap<String, String>>,
    poll_script: VecDeque<Value>,
    pulls: Vec<FakePull>,
    next_pull: u64,
    fail_pull_create: VecDeque<(u16, String)>,
    fail_any: VecDeque<(u16, String)>,
    base_url: String,
}

pub struct FakeGithubServer {
    pub base_url: String,
    shared: Arc<Mutex<Shared>>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl FakeGithubServer {
    pub async fn start(config: FakeGithubConfig) -> FakeGithubServer {
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind fake github");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
        let shared = Arc::new(Mutex::new(Shared {
            config,
            base_url: base_url.clone(),
            next_pull: 1,
            ..Default::default()
        }));
        let app = Router::new().fallback(handle).with_state(shared.clone());
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        FakeGithubServer {
            base_url,
            shared,
            shutdown: Some(tx),
            task: Some(task),
        }
    }

    /// The environment that points the app at this server.
    pub fn env(&self) -> Vec<(String, String)> {
        vec![
            ("RATCHET_GITHUB_API_URL".into(), self.base_url.clone()),
            ("RATCHET_GITHUB_WEB_URL".into(), self.base_url.clone()),
            ("RATCHET_GITHUB_CLIENT_ID".into(), FAKE_CLIENT_ID.into()),
        ]
    }

    /// The user "approves" the device code: the next poll returns a token.
    pub fn approve(&self) {
        self.with(|s| s.approved = true);
    }

    /// Answer the next device-flow polls with these bodies (before falling back to approved/pending).
    pub fn script_poll(&self, answer: Value) {
        self.with(|s| s.poll_script.push_back(answer));
    }

    pub fn polls(&self) -> usize {
        self.with(|s| s.polls)
    }

    /// `"METHOD /path"` of every request, in order.
    pub fn requests(&self) -> Vec<String> {
        self.with(|s| s.requests.clone())
    }

    pub fn authorizations(&self) -> Vec<String> {
        self.with(|s| s.authorizations.clone())
    }

    pub fn forms(&self) -> Vec<HashMap<String, String>> {
        self.with(|s| s.forms.clone())
    }

    pub fn pulls(&self) -> Vec<FakePull> {
        self.with(|s| s.pulls.clone())
    }

    /// Make the next `POST …/pulls` fail with this status and message.
    pub fn fail_next_pull_create(&self, status: u16, message: &str) {
        self.with(|s| s.fail_pull_create.push_back((status, message.to_string())));
    }

    /// Make the next authenticated request (any route) fail with this status and message.
    pub fn fail_next_request(&self, status: u16, message: &str) {
        self.with(|s| s.fail_any.push_back((status, message.to_string())));
    }

    pub fn set_file(&self, key: &str, text: &str) {
        self.with(|s| {
            s.config.files.insert(key.to_string(), text.to_string());
        });
    }

    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        f(&mut self.shared.lock().expect("fake github state"))
    }

    pub async fn close(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }
}

impl Drop for FakeGithubServer {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// A client for tests: no proxy, short timeout.
pub fn test_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("test client")
}

fn json_response(status: u16, body: Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn not_found() -> Response {
    json_response(404, json!({ "message": "Not Found" }))
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

async fn handle(
    State(shared): State<Arc<Mutex<Shared>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = shared.lock().expect("fake github state");
    let path = uri.path().to_string();
    s.requests.push(format!("{method} {path}"));
    let query: HashMap<String, String> = uri
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1")
        .to_string();

    if method == Method::POST && path == "/login/device/code" {
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(&body).into_owned().collect();
        s.forms.push(form);
        return json_response(
            200,
            json!({ "device_code": "dc-e2e", "user_code": DEVICE_CODE, "verification_uri": format!("http://{host}/login/device"), "expires_in": 900, "interval": 1 }),
        );
    }
    if method == Method::POST && path == "/login/oauth/access_token" {
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(&body).into_owned().collect();
        s.forms.push(form);
        s.polls += 1;
        if let Some(answer) = s.poll_script.pop_front() {
            return json_response(200, answer);
        }
        return json_response(
            200,
            if s.approved {
                json!({ "access_token": FAKE_TOKEN, "token_type": "bearer", "scope": "repo" })
            } else {
                json!({ "error": "authorization_pending" })
            },
        );
    }

    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(String::from);
    let Some(token) = bearer else {
        return json_response(401, json!({ "message": "Bad credentials" }));
    };
    s.authorizations.push(token);
    if let Some((status, message)) = s.fail_any.pop_front() {
        return json_response(status, json!({ "message": message }));
    }

    if path == "/user" {
        return json_response(200, json!({ "login": FAKE_LOGIN }));
    }
    if path == "/user/repos" {
        let page: usize = query.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
        let rows: Vec<Value> = s
            .config
            .repos
            .iter()
            .skip((page.max(1) - 1) * 100)
            .take(100)
            .cloned()
            .collect();
        return json_response(200, Value::Array(rows));
    }

    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if parts.len() >= 3 && parts[0] == "repos" {
        let (owner, name) = (parts[1].to_string(), parts[2].to_string());
        let full = format!("{owner}/{name}");
        match (method.as_str(), &parts[3..]) {
            ("GET", []) => {
                return match s
                    .config
                    .repos
                    .iter()
                    .find(|r| r["full_name"] == full.as_str())
                {
                    Some(r) => json_response(200, r.clone()),
                    None => not_found(),
                };
            }
            ("GET", ["contents", ..]) => {
                let file_path = percent_decode(&parts[4..].join("/"));
                let key = format!(
                    "{full}@{}:{file_path}",
                    query.get("ref").cloned().unwrap_or_default()
                );
                return match s.config.files.get(&key) {
                    None => not_found(),
                    Some(text) => json_response(
                        200,
                        json!({ "encoding": "base64", "content": base64::engine::general_purpose::STANDARD.encode(text) }),
                    ),
                };
            }
            ("GET", ["branches"]) => {
                let names: Option<Vec<String>> =
                    s.config.branches.get(&full).cloned().or_else(|| {
                        s.config
                            .repos
                            .iter()
                            .find(|r| r["full_name"] == full.as_str())
                            .map(|r| {
                                vec![r["default_branch"].as_str().unwrap_or("main").to_string()]
                            })
                    });
                return match names {
                    None => not_found(),
                    Some(names) => json_response(
                        200,
                        Value::Array(names.into_iter().map(|n| json!({ "name": n })).collect()),
                    ),
                };
            }
            ("POST", ["pulls"]) => {
                if let Some((status, message)) = s.fail_pull_create.pop_front() {
                    return json_response(status, json!({ "message": message }));
                }
                let input: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let field = |k: &str| input[k].as_str().unwrap_or("").to_string();
                let (head, base) = (field("head"), field("base"));
                if s.pulls
                    .iter()
                    .any(|p| p.owner == owner && p.repo == name && p.head == head && p.base == base)
                {
                    return json_response(
                        422,
                        json!({ "message": "Validation Failed", "errors": [{ "message": format!("A pull request already exists for {owner}:{head}.") }] }),
                    );
                }
                let number = s.next_pull;
                s.next_pull += 1;
                s.pulls.push(FakePull {
                    number,
                    owner: owner.clone(),
                    repo: name.clone(),
                    head,
                    base,
                    title: field("title"),
                    body: field("body"),
                });
                let url = format!("{}/{owner}/{name}/pull/{number}", s.base_url);
                return json_response(201, json!({ "html_url": url, "number": number }));
            }
            ("GET", ["pulls"]) => {
                let want_head = query.get("head").cloned().unwrap_or_default();
                let want_base = query.get("base").cloned();
                let rows: Vec<Value> = s
                    .pulls
                    .iter()
                    .filter(|p| p.owner == owner && p.repo == name && format!("{}:{}", p.owner, p.head) == want_head)
                    .filter(|p| want_base.as_deref().is_none_or(|b| b == p.base))
                    .map(|p| json!({ "html_url": format!("{}/{owner}/{name}/pull/{}", s.base_url, p.number), "number": p.number }))
                    .collect();
                return json_response(200, Value::Array(rows));
            }
            ("PATCH", ["pulls", number]) => {
                let n: u64 = number.parse().unwrap_or(0);
                let input: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let base_url = s.base_url.clone();
                return match s
                    .pulls
                    .iter_mut()
                    .find(|p| p.number == n && p.owner == owner && p.repo == name)
                {
                    None => not_found(),
                    Some(p) => {
                        if let Some(t) = input["title"].as_str() {
                            p.title = t.to_string();
                        }
                        if let Some(b) = input["body"].as_str() {
                            p.body = b.to_string();
                        }
                        json_response(
                            200,
                            json!({ "html_url": format!("{base_url}/{owner}/{name}/pull/{n}"), "number": n }),
                        )
                    }
                };
            }
            _ => {}
        }
    }
    not_found()
}

// ------------------------------------------------------------------ git runner

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    /// The arguments as the caller passed them (before any rewrite).
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// A runner that records every call and rewrites argument substrings before running them on `inner`. The GitHub host
/// tests use it to swap `https://github.com/acme/search.git` for a local bare repo while checking exactly what the host
/// asked for (argv, environment).
pub struct RecordingRunner {
    inner: Arc<dyn GitRunner>,
    rewrites: Vec<(String, String)>,
    calls: Mutex<Vec<RecordedCall>>,
}

impl RecordingRunner {
    pub fn new(inner: Arc<dyn GitRunner>, rewrites: Vec<(String, String)>) -> Self {
        RecordingRunner {
            inner,
            rewrites,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls").clone()
    }
}

#[async_trait]
impl GitRunner for RecordingRunner {
    async fn run(&self, args: &[String], opts: &GitRunOptions) -> Result<String> {
        self.calls.lock().expect("calls").push(RecordedCall {
            args: args.to_vec(),
            env: opts.env.iter().cloned().collect(),
        });
        let rewritten: Vec<String> = args
            .iter()
            .map(|a| {
                self.rewrites
                    .iter()
                    .fold(a.clone(), |acc, (from, to)| acc.replace(from.as_str(), to))
            })
            .collect();
        self.inner.run(&rewritten, opts).await
    }
}
