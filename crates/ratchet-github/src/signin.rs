//! "Sign in with GitHub": the OAuth device flow (ported from `github-signin.ts`).
//!
//! `start` returns the code to show and keeps polling in the background; on success the token goes to the keychain-backed
//! [`AccountStore`] (never to the UI) and the account login is kept.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::api::GithubApi;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowState {
    Pending,
    Expired,
    Denied,
    Error,
}

/// The sign-in in progress. Same JSON as the TS `GithubFlowVM`.
// TODO(core): a view-model type; moves to the API crate with the other VMs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubFlow {
    pub state: FlowState,
    /// Shown to the user, who types it at `verification_uri`.
    pub user_code: String,
    pub verification_uri: String,
    /// Milliseconds since the epoch.
    pub expires_at: i64,
    pub message: Option<String>,
}

/// Where a successful sign-in lands (the Keychain-backed settings in the app).
pub trait AccountStore: Send + Sync {
    fn set_token(&self, token: &str);
    /// `source` is `"device"` for this flow.
    fn set_account(&self, login: &str, source: &str);
}

pub type SleepFn =
    Arc<dyn Fn(u64, CancellationToken) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A real timer that cancellation interrupts.
pub fn real_sleep() -> SleepFn {
    Arc::new(|ms, cancel| {
        Box::pin(async move {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(ms)) => {}
                _ = cancel.cancelled() => {}
            }
        })
    })
}

/// Called with the verification URL; must not fail (a panic is swallowed: the code is shown in the app anyway).
pub type OpenExternalFn = Arc<dyn Fn(&str) + Send + Sync>;

pub struct SignInDeps {
    pub api: Arc<dyn GithubApi>,
    pub store: Arc<dyn AccountStore>,
    /// Open the verification page in the system browser.
    pub open_external: Option<OpenExternalFn>,
    /// Injectable for tests (default: a real timer that `cancel` can interrupt).
    pub sleep: Option<SleepFn>,
    /// Milliseconds since the epoch (default: the system clock).
    pub now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
    /// Called after the token is stored (the service rebuilds what depends on it).
    pub on_signed_in: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl SignInDeps {
    pub fn new(api: Arc<dyn GithubApi>, store: Arc<dyn AccountStore>) -> Self {
        SignInDeps {
            api,
            store,
            open_external: None,
            sleep: None,
            now: None,
            on_signed_in: None,
        }
    }

    fn now(&self) -> i64 {
        match &self.now {
            Some(f) => f(),
            None => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        }
    }
}

#[derive(Default)]
struct Inner {
    flow: Option<GithubFlow>,
    /// Identifies the current flow (the TS code compared object identity).
    generation: u64,
    cancel: Option<CancellationToken>,
}

pub struct GithubSignIn {
    deps: Arc<SignInDeps>,
    inner: Arc<Mutex<Inner>>,
}

impl GithubSignIn {
    pub fn new(deps: SignInDeps) -> Self {
        GithubSignIn {
            deps: Arc::new(deps),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn current(&self) -> Option<GithubFlow> {
        self.inner.lock().expect("sign-in state").flow.clone()
    }

    /// Request a device code, show it (and open the browser), and poll in the background until the user approves.
    pub async fn start(&self) -> Result<GithubFlow> {
        self.cancel();
        let code = self.deps.api.device_start().await?;
        let flow = GithubFlow {
            state: FlowState::Pending,
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
            expires_at: self.deps.now() + (code.expires_in as i64) * 1000,
            message: None,
        };
        let cancel = CancellationToken::new();
        let generation = {
            let mut inner = self.inner.lock().expect("sign-in state");
            inner.generation += 1;
            inner.flow = Some(flow.clone());
            inner.cancel = Some(cancel.clone());
            inner.generation
        };
        if let Some(open) = &self.deps.open_external {
            // the code is shown in the app anyway
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                open(&code.verification_uri)
            }));
        }
        let task = Poller {
            deps: self.deps.clone(),
            inner: self.inner.clone(),
            generation,
            cancel,
            expires_at: flow.expires_at,
        };
        tokio::spawn(task.run(code.device_code, code.interval));
        Ok(flow)
    }

    pub fn cancel(&self) {
        let mut inner = self.inner.lock().expect("sign-in state");
        if let Some(c) = inner.cancel.take() {
            c.cancel();
        }
        inner.flow = None;
    }
}

struct Poller {
    deps: Arc<SignInDeps>,
    inner: Arc<Mutex<Inner>>,
    generation: u64,
    cancel: CancellationToken,
    expires_at: i64,
}

impl Poller {
    fn is_current(&self) -> bool {
        let inner = self.inner.lock().expect("sign-in state");
        inner.generation == self.generation && inner.flow.is_some()
    }

    fn end(&self, state: FlowState, message: &str) {
        let mut inner = self.inner.lock().expect("sign-in state");
        if inner.generation == self.generation
            && let Some(flow) = inner.flow.as_mut()
        {
            flow.state = state;
            flow.message = Some(message.to_string());
        }
    }

    async fn run(self, device_code: String, start_interval: u64) {
        use crate::api::DevicePoll;
        let sleep = self.deps.sleep.clone().unwrap_or_else(real_sleep);
        let mut interval = start_interval.max(1);
        while !self.cancel.is_cancelled() {
            sleep(interval * 1000, self.cancel.clone()).await;
            if self.cancel.is_cancelled() || !self.is_current() {
                return;
            }
            if self.deps.now() >= self.expires_at {
                return self.end(FlowState::Expired, "The code expired. Start again.");
            }
            let poll = match self.deps.api.device_poll(&device_code).await {
                Ok(p) => p,
                Err(e) => return self.end(FlowState::Error, &e.to_string()),
            };
            match poll {
                DevicePoll::Pending => continue,
                DevicePoll::SlowDown { interval: asked } => {
                    interval = (interval + 5).max(asked);
                    continue;
                }
                DevicePoll::Expired => {
                    return self.end(FlowState::Expired, "The code expired. Start again.");
                }
                DevicePoll::Denied => {
                    return self.end(FlowState::Denied, "Access was denied on GitHub.");
                }
                DevicePoll::Error { message } => return self.end(FlowState::Error, &message),
                DevicePoll::Ok { token } => {
                    match self.deps.api.user(&token).await {
                        Ok(user) => {
                            self.deps.store.set_token(&token);
                            self.deps.store.set_account(&user.login, "device");
                        }
                        Err(e) => return self.end(FlowState::Error, &e.to_string()),
                    }
                    {
                        let mut inner = self.inner.lock().expect("sign-in state");
                        if inner.generation == self.generation {
                            inner.flow = None;
                        }
                    }
                    if let Some(done) = &self.deps.on_signed_in {
                        done();
                    }
                    return;
                }
            }
        }
    }
}
