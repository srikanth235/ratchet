//! Connectors of the simulation: the real [`ConnectorStore`] over an in-memory secret store, scripted probes and a pretend
//! GitHub, seeded with the accounts of `design/v02/2-connectors.png`. Everything the Connectors screen does works (add,
//! rename, default, remove, test, the device flow, the choice remembered per repo); nothing leaves the process.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ratchet_agents::detect::{HarnessAuth, HarnessInfo, HarnessKind};
use ratchet_connectors::secrets::{MemorySecretStore, SecretStore};
use ratchet_connectors::{
    AddAgentConnectorInput, AddAwsConnectorInput, AwsAuthStyle, AwsProfile, AwsProfileList,
    ConnectorAgent, ConnectorDeps, ConnectorProbe, ConnectorStore, HarnessSource, UpsertGithub,
};
use ratchet_core::ports::Clock;
use ratchet_github::{
    DeviceCode, DevicePoll, Error as GithubError, GithubApi, GithubUser, Repo,
    Result as GithubResult,
};

use super::SimOptions;
use crate::api::ApiEvent;
use crate::connectors_hub::{ConnectorsHub, HubOptions};

pub const SIM_LOGIN: &str = "srikanth235";
const SIM_TOKEN: &str = "gho_SIMULATEDDEMOTOKEN000000001";
const SIM_ORG_TOKEN: &str = "ghp_SIMULATEDORGTOKEN000000000000000000";

/// A pretend GitHub: the device flow is approved after `approve_after` (never when `None`) or when
/// [`SimGithub::approve`] is called; the signed-in account is `srikanth235`.
pub struct SimGithub {
    started: std::sync::Mutex<Option<Instant>>,
    approve_after: Option<Duration>,
    approved: AtomicBool,
}

impl SimGithub {
    pub fn new(approve_after: Option<Duration>) -> Self {
        Self {
            started: std::sync::Mutex::new(None),
            approve_after,
            approved: AtomicBool::new(false),
        }
    }

    /// The user approved the code in the browser.
    pub fn approve(&self) {
        self.approved.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl GithubApi for SimGithub {
    fn client_id_configured(&self) -> bool {
        true
    }

    async fn device_start(&self) -> GithubResult<DeviceCode> {
        *self.started.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        self.approved.store(false, Ordering::SeqCst);
        Ok(DeviceCode {
            device_code: "sim-device-code".into(),
            user_code: "WDJB-MJHT".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 900,
            interval: 1,
        })
    }

    async fn device_poll(&self, _device_code: &str) -> GithubResult<DevicePoll> {
        let started = *self.started.lock().unwrap_or_else(|e| e.into_inner());
        let due = match (started, self.approve_after) {
            (Some(at), Some(after)) => at.elapsed() >= after,
            _ => false,
        };
        if due || self.approved.load(Ordering::SeqCst) {
            Ok(DevicePoll::Ok {
                token: SIM_TOKEN.into(),
            })
        } else {
            Ok(DevicePoll::Pending)
        }
    }

    async fn user(&self, token: &str) -> GithubResult<GithubUser> {
        // the seeded organisation token belongs to the organisation, every other token to the demo user
        Ok(GithubUser {
            login: if token == SIM_ORG_TOKEN {
                "acme-org".into()
            } else {
                SIM_LOGIN.into()
            },
        })
    }

    async fn repos(&self, _token: &str, _query: Option<&str>) -> GithubResult<Vec<Repo>> {
        Ok(Vec::new())
    }

    async fn repo(&self, _token: &str, full_name: &str) -> GithubResult<Repo> {
        Err(GithubError::http(
            Some(404),
            format!("{full_name}: not found, or the account cannot see it (404)."),
        ))
    }

    async fn file(&self, _: &str, _: &str, _: &str, _: &str) -> GithubResult<Option<String>> {
        Ok(None)
    }

    async fn branches(&self, _token: &str, _full_name: &str) -> GithubResult<Vec<String>> {
        Ok(vec!["main".into()])
    }
}

/// Probes that answer like the screenshots: the staging SSO session has expired, everything else works.
struct SimProbe;

#[async_trait]
impl ConnectorProbe for SimProbe {
    async fn aws(&self, _region: &str, profile: &str) -> (bool, String) {
        if profile == "acme-staging" {
            (false, "The SSO session has expired".into())
        } else {
            (true, "signed in to account 111122223333".into())
        }
    }

    async fn anthropic(&self, _api_key: &str) -> (bool, String) {
        (true, "Anthropic accepted the key".into())
    }

    async fn openai(&self, _api_key: &str) -> (bool, String) {
        (true, "OpenAI accepted the key".into())
    }

    async fn xai(&self, _api_key: &str) -> (bool, String) {
        (true, "xAI accepted the key".into())
    }
}

/// Claude Code is installed and signed in; Codex is not installed.
struct SimHarnesses;

#[async_trait]
impl HarnessSource for SimHarnesses {
    async fn get(&self) -> Vec<HarnessInfo> {
        vec![
            HarnessInfo {
                kind: HarnessKind::ClaudeCode,
                installed: true,
                version: Some("2.1.314".into()),
                path: Some("/usr/local/bin/claude".into()),
                auth: HarnessAuth::SignedIn,
                hint: "signed in".into(),
            },
            HarnessInfo {
                kind: HarnessKind::Codex,
                installed: false,
                version: None,
                path: None,
                auth: HarnessAuth::SignedOut,
                hint: "Install Codex: npm i -g @openai/codex".into(),
            },
        ]
    }

    async fn refresh(&self) -> Vec<HarnessInfo> {
        self.get().await
    }
}

fn profiles() -> AwsProfileList {
    let p = |name: &str, kind| AwsProfile {
        name: name.into(),
        kind,
    };
    AwsProfileList {
        profiles: vec![
            p("acme-prod", AwsAuthStyle::Sso),
            p("acme-staging", AwsAuthStyle::Sso),
            p("personal-dev", AwsAuthStyle::Keys),
        ],
        files_found: true,
    }
}

/// The hub of the simulation, seeded when `seed` is set. Returns the pretend GitHub too (tests approve its device flow).
pub(super) async fn build_hub(
    opts: &SimOptions,
    clock: Arc<dyn Clock>,
    emit: Arc<dyn Fn(ApiEvent) + Send + Sync>,
    seed: bool,
) -> (ConnectorsHub, Arc<SimGithub>) {
    let approve_after = (opts.auto_run && opts.github_flow_ms > 0)
        .then(|| Duration::from_millis(opts.github_flow_ms));
    let github = Arc::new(SimGithub::new(approve_after));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let lister: ratchet_connectors::AwsProfileLister = Arc::new(profiles);
    let store = Arc::new(ConnectorStore::new(
        None,
        secrets,
        HashMap::new(),
        ConnectorDeps {
            github: Some(github.clone()),
            harnesses: Some(Arc::new(SimHarnesses)),
            aws_profiles: Some(lister.clone()),
            probe: Some(Arc::new(SimProbe)),
            legacy: None,
            clock: Some(clock.clone()),
        },
    ));
    if seed {
        seed_demo(&store).await;
    }
    let hub = ConnectorsHub::new(HubOptions {
        store,
        github: Some(github.clone()),
        aws_profiles: lister,
        open_external: None,
        clock,
        invalidate: Arc::new(|| {}),
        emit,
    });
    (hub, github)
}

/// The accounts of `design/v02/2-connectors.png`.
async fn seed_demo(store: &Arc<ConnectorStore>) {
    let aws = |profile: &str, region: &str, cap: Option<f64>| AddAwsConnectorInput {
        profile: profile.into(),
        region: Some(region.into()),
        monthly_cap_usd: cap,
        name: None,
    };
    let seeded = (|| -> ratchet_connectors::Result<()> {
        store.upsert_github(UpsertGithub::new(SIM_LOGIN, SIM_TOKEN, true))?;
        store.upsert_github(UpsertGithub::new("acme-org", SIM_ORG_TOKEN, false))?;
        store.add_aws(aws("acme-prod", "us-east-1", Some(500.0)), false)?;
        store.add_aws(aws("personal-dev", "us-west-2", Some(100.0)), false)?;
        store.add_aws(aws("acme-staging", "eu-west-1", None), false)?;
        Ok(())
    })();
    if let Err(e) = seeded {
        tracing::warn!("cannot seed the simulated connectors: {e}");
        return;
    }
    let _ = store
        .add_agent(
            AddAgentConnectorInput::ApiKey {
                agent: ConnectorAgent::ClaudeCode,
                api_key: "sk-ant-api03-SIMULATEDTEAMKEY0001".into(),
                name: Some("Claude Code · team key".into()),
            },
            false,
        )
        .await;
    // detect the CLI logins, then let the scripted probes set the statuses (the staging SSO session has expired)
    let _ = store.list().await;
    store.refresh().await;
}
