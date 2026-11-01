//! "Test connection" for the Settings screen: small, read-only calls that prove each live setting works before any money is
//! spent. Everything is injectable so tests never touch the network; none of the results contains a secret.
//! Port of `packages/runtime/src/live-checks.ts`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;
use ratchet_github::{CheckGitHubAccessOptions, GitHubAccess};
use ratchet_sandbox::sandbox::{ProcessRunner, ProcessSpec, TokioProcessRunner};
use ratchet_sandbox::{CallerIdentity, CheckAwsCredentialsOptions};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionTarget {
    Aws,
    Github,
    Anthropic,
    Openai,
    Xai,
    Tofu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionResult {
    pub target: ConnectionTarget,
    pub ok: bool,
    /// One line for the UI. Never contains a key or token.
    pub detail: String,
}

/// `fetch` of the model-API probes: a GET that yields the status code (or a transport error message).
#[async_trait]
pub trait HttpProbe: Send + Sync {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
        timeout: Duration,
    ) -> Result<u16, String>;
}

struct ReqwestProbe;

#[async_trait]
impl HttpProbe for ReqwestProbe {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
        timeout: Duration,
    ) -> Result<u16, String> {
        let mut req = reqwest::Client::new().get(url).timeout(timeout);
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        req.send()
            .await
            .map(|r| r.status().as_u16())
            .map_err(|e| format!("{e:#}"))
    }
}

pub type CheckAws = Arc<
    dyn Fn(CheckAwsCredentialsOptions) -> BoxFuture<'static, Result<CallerIdentity, String>>
        + Send
        + Sync,
>;
pub type CheckGitHub = Arc<
    dyn Fn(CheckGitHubAccessOptions) -> BoxFuture<'static, Result<GitHubAccess, String>>
        + Send
        + Sync,
>;
/// Resolves to `true` when the path exists.
pub type PathExists = Arc<dyn Fn(&Path) -> BoxFuture<'static, bool> + Send + Sync>;

#[derive(Clone, Default)]
pub struct ConnectionTesterDeps {
    pub fetch: Option<Arc<dyn HttpProbe>>,
    pub process_runner: Option<Arc<dyn ProcessRunner>>,
    pub check_aws: Option<CheckAws>,
    pub check_github: Option<CheckGitHub>,
    pub exists: Option<PathExists>,
    /// Abort a model API probe after this long. Default 15 s.
    pub timeout: Option<Duration>,
}

#[derive(Clone)]
pub struct ConnectionTester {
    fetch: Arc<dyn HttpProbe>,
    runner: Arc<dyn ProcessRunner>,
    check_aws: CheckAws,
    check_github: CheckGitHub,
    exists: PathExists,
    timeout: Duration,
}

pub fn create_connection_tester(deps: ConnectionTesterDeps) -> ConnectionTester {
    ConnectionTester {
        fetch: deps.fetch.unwrap_or_else(|| Arc::new(ReqwestProbe)),
        runner: deps
            .process_runner
            .unwrap_or_else(|| Arc::new(TokioProcessRunner)),
        check_aws: deps.check_aws.unwrap_or_else(|| {
            Arc::new(|o| {
                Box::pin(async move {
                    ratchet_sandbox::check_aws_credentials(o)
                        .await
                        .map_err(|e| e.to_string())
                })
            })
        }),
        check_github: deps.check_github.unwrap_or_else(|| {
            Arc::new(|o| {
                Box::pin(async move {
                    ratchet_github::check_github_access(o)
                        .await
                        .map_err(|e| e.to_string())
                })
            })
        }),
        exists: deps.exists.unwrap_or_else(|| {
            Arc::new(|p| {
                let p = p.to_path_buf();
                Box::pin(async move { tokio::fs::metadata(&p).await.is_ok() })
            })
        }),
        timeout: deps.timeout.unwrap_or(Duration::from_secs(15)),
    }
}

fn redact(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|s| s.len() >= 4)
        .fold(text.to_owned(), |t, s| t.replace(*s, "***"))
}

fn first_line(msg: &str) -> &str {
    msg.split('\n').next().unwrap_or("failed")
}

async fn wrap(
    target: ConnectionTarget,
    secrets: &[&str],
    fut: impl std::future::Future<Output = Result<String, String>>,
) -> ConnectionResult {
    match fut.await {
        Ok(detail) => ConnectionResult {
            target,
            ok: true,
            detail: redact(&detail, secrets),
        },
        Err(e) => ConnectionResult {
            target,
            ok: false,
            detail: redact(first_line(&e), secrets),
        },
    }
}

impl ConnectionTester {
    /// GET a model API's "list models" endpoint: 200 = the key works.
    async fn probe_models(
        &self,
        name: &str,
        url: &str,
        headers: &[(&str, String)],
    ) -> Result<String, String> {
        let status = self.fetch.get(url, headers, self.timeout).await?;
        match status {
            200 => Ok(format!("{name} accepted the key")),
            401 | 403 => Err(format!("{name} rejected the key ({status})")),
            429 => Ok(format!("{name} accepted the key (rate limited right now)")),
            _ => Err(format!("{name} answered {status}")),
        }
    }

    pub async fn aws(
        &self,
        region: &str,
        profile: Option<&str>,
        expected_account_id: Option<&str>,
    ) -> ConnectionResult {
        wrap(ConnectionTarget::Aws, &[], async {
            let id = (self.check_aws)(CheckAwsCredentialsOptions {
                region: region.to_owned(),
                profile: profile.filter(|p| !p.is_empty()).map(str::to_owned),
                expected_account_id: expected_account_id
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned),
                client: None,
            })
            .await?;
            Ok(format!(
                "signed in to account {} in {region}{}",
                id.account_id,
                profile
                    .filter(|p| !p.is_empty())
                    .map(|p| format!(" as profile {p}"))
                    .unwrap_or_default()
            ))
        })
        .await
    }

    pub async fn github(&self, repo: &str, token: &str) -> ConnectionResult {
        wrap(ConnectionTarget::Github, &[token], async {
            let a = (self.check_github)(CheckGitHubAccessOptions::new(repo, token)).await?;
            if !a.can_push {
                return Err(format!(
                    "{} is readable but the token cannot push: grant it Contents: read and write and Pull requests: read and write",
                    a.full_name
                ));
            }
            Ok(format!(
                "{} ({}, default branch {}); the token can push",
                a.full_name,
                if a.private { "private" } else { "public" },
                a.default_branch
            ))
        })
        .await
    }

    pub async fn anthropic(&self, api_key: &str) -> ConnectionResult {
        wrap(
            ConnectionTarget::Anthropic,
            &[api_key],
            self.probe_models(
                "Anthropic",
                "https://api.anthropic.com/v1/models?limit=1",
                &[
                    ("x-api-key", api_key.to_owned()),
                    ("anthropic-version", "2023-06-01".to_owned()),
                ],
            ),
        )
        .await
    }

    pub async fn openai(&self, api_key: &str) -> ConnectionResult {
        wrap(
            ConnectionTarget::Openai,
            &[api_key],
            self.probe_models(
                "OpenAI",
                "https://api.openai.com/v1/models",
                &[("authorization", format!("Bearer {api_key}"))],
            ),
        )
        .await
    }

    pub async fn xai(&self, api_key: &str) -> ConnectionResult {
        wrap(
            ConnectionTarget::Xai,
            &[api_key],
            self.probe_models(
                "xAI",
                "https://api.x.ai/v1/models",
                &[("authorization", format!("Bearer {api_key}"))],
            ),
        )
        .await
    }

    pub async fn tofu(&self, binary: &str, module_dir: &Path) -> ConnectionResult {
        wrap(ConnectionTarget::Tofu, &[], async {
            let dir = module_dir.display();
            if !(self.exists)(&module_dir.join("main.tf")).await {
                return Err(format!(
                    "{dir} does not contain an OpenTofu module (main.tf not found)"
                ));
            }
            let res = self
                .runner
                .run(&ProcessSpec {
                    command: binary.to_owned(),
                    args: vec!["version".to_owned()],
                    cwd: module_dir.to_path_buf(),
                    env: [("CHECKPOINT_DISABLE".to_owned(), "1".to_owned())].into(),
                })
                .await
                .map_err(|e| format!("cannot run {binary}: {e}"))?;
            if res.exit_code != 0 {
                return Err(format!(
                    "`{binary} version` exited with status {}",
                    res.exit_code
                ));
            }
            let re = regex::Regex::new(r"OpenTofu v(\d+\.\d+\.\d+\S*)").expect("static regex");
            let Some(v) = re.captures(&res.stdout) else {
                return Err(format!(
                    "{binary} is not OpenTofu (got \"{}\")",
                    res.stdout.trim().split('\n').next().unwrap_or("")
                ));
            };
            Ok(format!("OpenTofu {}, module {dir}", &v[1]))
        })
        .await
    }
}
