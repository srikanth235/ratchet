//! The connectors group of [`crate::service::AppService`], written once for both services: the simulation and `RealService`
//! hold a [`ConnectorsHub`] over a [`ConnectorStore`] (the simulation over an in-memory one with scripted probes). Port of the
//! connectors section of `packages/desktop/src/main/service.ts`.

use std::sync::{Arc, Mutex, PoisonError};

use ratchet_connectors::{
    AddAgentConnectorInput, AddAwsConnectorInput, AddGithubConnectorInput, AwsProfileLister,
    ConnectorError, ConnectorStore, ConnectorTestVm, RepoConnections, RepoConnectionsVm,
    StartGithubSignInInput, UpdateConnectorInput, UpsertGithub,
};
use ratchet_github::GithubApi;
use ratchet_github::signin::{
    AccountStore, FlowState, GithubFlow, GithubSignIn, OpenExternalFn, SignInDeps,
};

use crate::api::{ApiEvent, ConnectorsVm, GithubFlowState, GithubFlowVm};
use crate::service::{ServiceError, ServiceResult};

/// GitHub failures, as the API reports them: a 404 is not found, a bad argument is invalid, the rest is upstream.
pub fn github_error(e: ratchet_github::Error) -> ServiceError {
    match &e {
        ratchet_github::Error::Http {
            status: Some(404), ..
        } => ServiceError::NotFound(e.to_string()),
        ratchet_github::Error::Invalid(m) => ServiceError::Invalid(m.clone()),
        _ => ServiceError::Upstream(e.to_string()),
    }
}

impl From<ConnectorError> for ServiceError {
    fn from(e: ConnectorError) -> Self {
        match e {
            ConnectorError::NotFound(m) => Self::NotFound(m),
            ConnectorError::Invalid(m) => Self::Invalid(m),
            ConnectorError::Keychain(m) => Self::Conflict(m),
        }
    }
}

/// The device flow in progress, as the wire shows it.
pub fn flow_vm(f: GithubFlow) -> GithubFlowVm {
    GithubFlowVm {
        state: match f.state {
            FlowState::Pending => GithubFlowState::Pending,
            FlowState::Expired => GithubFlowState::Expired,
            FlowState::Denied => GithubFlowState::Denied,
            FlowState::Error => GithubFlowState::Error,
        },
        user_code: f.user_code,
        verification_uri: f.verification_uri,
        expires_at: f.expires_at,
        message: f.message,
    }
}

/// Where a device-flow sign-in lands: the connector store (a new account, the account with the same login, or the connector
/// being signed in again).
struct HubAccounts {
    store: Arc<ConnectorStore>,
    /// The connector the last sign-in stored (for the `connector` event).
    last: Arc<Mutex<Option<String>>>,
}

impl AccountStore for HubAccounts {
    fn set_token(&self, _: &str) {
        unreachable!("the hub implements `save`")
    }

    fn set_account(&self, _: &str, _: &str) {
        unreachable!("the hub implements `save`")
    }

    fn save(&self, token: &str, login: &str, connector_id: Option<&str>) {
        let input = UpsertGithub {
            connector_id: connector_id.map(str::to_owned),
            ..UpsertGithub::new(login, token, true)
        };
        match self.store.upsert_github(input) {
            Ok(rec) => {
                *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(rec.id);
            }
            Err(e) => tracing::warn!("cannot store the GitHub account: {e}"),
        }
    }
}

pub type Invalidate = Arc<dyn Fn() + Send + Sync>;
pub type Emit = Arc<dyn Fn(ApiEvent) + Send + Sync>;

pub struct HubOptions {
    pub store: Arc<ConnectorStore>,
    pub github: Option<Arc<dyn GithubApi>>,
    /// The profiles in `~/.aws`: adding a connector only offers profiles that exist.
    pub aws_profiles: AwsProfileLister,
    pub open_external: Option<OpenExternalFn>,
    pub clock: Arc<dyn ratchet_core::ports::Clock>,
    /// Runtimes built from the old choice are rebuilt on next use.
    pub invalidate: Invalidate,
    pub emit: Emit,
}

pub struct ConnectorsHub {
    store: Arc<ConnectorStore>,
    github: Option<Arc<dyn GithubApi>>,
    signin: Option<GithubSignIn>,
    aws_profiles: AwsProfileLister,
    invalidate: Invalidate,
    emit: Emit,
}

impl ConnectorsHub {
    pub fn new(o: HubOptions) -> Self {
        let last: Arc<Mutex<Option<String>>> = Arc::default();
        let signin = o.github.as_ref().map(|api| {
            let mut deps = SignInDeps::new(
                api.clone(),
                Arc::new(HubAccounts {
                    store: o.store.clone(),
                    last: last.clone(),
                }),
            );
            deps.open_external = o.open_external.clone();
            deps.clock = o.clock.clone();
            let (invalidate, emit, store) = (o.invalidate.clone(), o.emit.clone(), o.store.clone());
            deps.on_signed_in = Some(Arc::new(move || {
                invalidate();
                emit(ApiEvent::Github { signed_in: true });
                let id = last.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some(id) = id {
                    emit(ApiEvent::Connector {
                        status: store.vm_of(&id).map(|c| c.status()),
                        connector_id: id,
                    });
                }
            }));
            GithubSignIn::new(deps)
        });
        Self {
            store: o.store,
            github: o.github,
            signin,
            aws_profiles: o.aws_profiles,
            invalidate: o.invalidate,
            emit: o.emit,
        }
    }

    pub fn store(&self) -> &Arc<ConnectorStore> {
        &self.store
    }

    pub fn github(&self) -> Option<&Arc<dyn GithubApi>> {
        self.github.as_ref()
    }

    /// The device flow (`None` when GitHub is not available in this build).
    pub fn signin(&self) -> Option<&GithubSignIn> {
        self.signin.as_ref()
    }

    pub fn need_signin(&self) -> ServiceResult<&GithubSignIn> {
        self.signin.as_ref().ok_or_else(|| {
            ServiceError::Unsupported("GitHub is not available in this build.".into())
        })
    }

    /// Stop the sign-in poll.
    pub fn shutdown(&self) {
        if let Some(s) = &self.signin {
            s.cancel();
        }
    }

    fn changed(&self, id: Option<&str>) {
        (self.invalidate)();
        if let Some(id) = id {
            (self.emit)(ApiEvent::Connector {
                connector_id: id.to_owned(),
                status: self.store.vm_of(id).map(|c| c.status()),
            });
        }
    }

    // ------------------------------------------------------------------ the group

    /// Everything the Connectors screen shows.
    pub async fn view(&self) -> ConnectorsVm {
        ConnectorsVm {
            connectors: self.store.list().await,
            github_flow: self
                .signin
                .as_ref()
                .and_then(GithubSignIn::current)
                .map(flow_vm),
            encryption_available: self.store.encryption_available(),
            client_id_configured: self
                .github
                .as_ref()
                .is_some_and(|g| g.client_id_configured()),
        }
    }

    pub async fn refresh(&self) -> ServiceResult<ConnectorsVm> {
        self.store.refresh().await;
        Ok(self.view().await)
    }

    pub async fn add_github(&self, input: AddGithubConnectorInput) -> ServiceResult<ConnectorsVm> {
        let rec = self
            .store
            .add_github_token(&input.token, input.name, false)
            .await?;
        self.changed(Some(&rec.id));
        Ok(self.view().await)
    }

    pub async fn start_sign_in(
        &self,
        input: StartGithubSignInInput,
    ) -> ServiceResult<ConnectorsVm> {
        if let Some(id) = &input.connector_id
            && self.store.vm_of(id).is_none()
        {
            return Err(ConnectorError::NotFound("That connector no longer exists.".into()).into());
        }
        let signin = self.need_signin()?;
        signin
            .start_for(input.connector_id)
            .await
            .map_err(github_error)?;
        Ok(self.view().await)
    }

    pub async fn cancel_sign_in(&self) -> ServiceResult<ConnectorsVm> {
        if let Some(s) = &self.signin {
            s.cancel();
        }
        Ok(self.view().await)
    }

    pub async fn add_aws(&self, input: AddAwsConnectorInput) -> ServiceResult<ConnectorsVm> {
        let known = (self.aws_profiles)();
        let wanted = input.profile.trim();
        if known.files_found && !known.profiles.iter().any(|p| p.name == wanted) {
            return Err(ServiceError::Invalid(format!(
                "{} is not in ~/.aws/config or ~/.aws/credentials.",
                input.profile
            )));
        }
        let rec = self.store.add_aws(input, false)?;
        // the screen refreshes the status; an offline probe must not hold the add
        let store = self.store.clone();
        let id = rec.id.clone();
        tokio::spawn(async move {
            let _ = store.test(&id).await;
        });
        self.changed(Some(&rec.id));
        Ok(self.view().await)
    }

    pub async fn add_agent(&self, input: AddAgentConnectorInput) -> ServiceResult<ConnectorsVm> {
        let is_key = matches!(input, AddAgentConnectorInput::ApiKey { .. });
        let rec = self.store.add_agent(input, false).await?;
        if is_key {
            let store = self.store.clone();
            let id = rec.id.clone();
            tokio::spawn(async move {
                let _ = store.test(&id).await;
            });
        }
        self.changed(Some(&rec.id));
        Ok(self.view().await)
    }

    pub async fn update(
        &self,
        id: &str,
        patch: UpdateConnectorInput,
    ) -> ServiceResult<ConnectorsVm> {
        self.store.update(id, patch)?;
        self.changed(Some(id));
        Ok(self.view().await)
    }

    pub async fn remove(&self, id: &str) -> ServiceResult<ConnectorsVm> {
        self.store.remove(id)?;
        self.changed(None);
        (self.emit)(ApiEvent::Connector {
            connector_id: id.to_owned(),
            status: None,
        });
        Ok(self.view().await)
    }

    pub async fn set_default(&self, id: &str) -> ServiceResult<ConnectorsVm> {
        self.store.set_default(id)?;
        self.changed(Some(id));
        Ok(self.view().await)
    }

    pub async fn test(&self, id: &str) -> ServiceResult<ConnectorTestVm> {
        let r = self.store.test(id).await?;
        let connector = self.store.vm_of(id);
        (self.emit)(ApiEvent::Connector {
            connector_id: id.to_owned(),
            status: connector.as_ref().map(|c| c.status()),
        });
        Ok(ConnectorTestVm {
            ok: r.ok,
            detail: r.detail,
            connector,
        })
    }

    pub async fn repo_connections(&self, repo: &str) -> ServiceResult<RepoConnectionsVm> {
        self.store.list().await; // detected CLI logins are connectors too
        Ok(self.store.repo_connections(repo))
    }

    pub async fn save_repo_connections(
        &self,
        repo: &str,
        selection: RepoConnections,
    ) -> ServiceResult<RepoConnectionsVm> {
        let r = self.store.save_repo_connections(repo, selection)?;
        (self.invalidate)();
        Ok(r)
    }
}
