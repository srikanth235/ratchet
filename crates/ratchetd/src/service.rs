//! `AppService`: everything the daemon's routes need, as one trait.
//!
//! The routes are thin: they decode the request, call one method here and encode the answer. The simulation
//! (`crate::sim::SimService`) implements the trait over the ledger, `ratchet_core::ports::mock` and seeded demo
//! data; `ratchet-engine` will implement it for real. Reads that only need the ledger can reuse
//! [`crate::views::LedgerViews`].
//!
//! The *provisional* groups (connectors, overview) and the environment drafts have default bodies that say the engine does
//! not support them yet, so an engine can ship without them.

use async_trait::async_trait;
use ratchet_core::enums::TemplateKind;
use ratchet_core::ports::{CostReconcileReport, ProposeTemplateInput, ProposeTemplateResult};
use ratchet_core::templates::{TemplateFilter, TemplateInfo};
use ratchet_core::{Calibration, EffectiveProtected};
use tokio::sync::broadcast;

use crate::api::*;

/// Why a service call failed. Each variant maps to one HTTP status and [`ErrorCode`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ServiceError {
    /// 404
    #[error("{0}")]
    NotFound(String),
    /// 400, one message
    #[error("{0}")]
    Invalid(String),
    /// 400, per-field problems
    #[error("{message}")]
    InvalidFields {
        message: String,
        fields: Vec<FieldError>,
    },
    /// 409: the resource is in a state that does not allow this
    #[error("{0}")]
    Conflict(String),
    /// 409: something to set up first
    #[error("{0}")]
    NotReady(String),
    /// 501: this engine does not do that
    #[error("{0}")]
    Unsupported(String),
    /// 502: GitHub, AWS or a model API failed
    #[error("{0}")]
    Upstream(String),
    /// 500
    #[error("{0}")]
    Internal(String),
}

pub type ServiceResult<T> = Result<T, ServiceError>;

impl ServiceError {
    pub fn not_found(what: &str, id: &str) -> Self {
        Self::NotFound(format!("{what} {id} not found"))
    }

    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Invalid(_) | Self::InvalidFields { .. } => ErrorCode::InvalidRequest,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::NotReady(_) => ErrorCode::NotReady,
            Self::Unsupported(_) => ErrorCode::Unsupported,
            Self::Upstream(_) => ErrorCode::Upstream,
            Self::Internal(_) => ErrorCode::Internal,
        }
    }

    pub fn into_api_error(self) -> ApiError {
        let code = self.code();
        let (message, fields) = match self {
            Self::InvalidFields { message, fields } => (message, fields),
            other => (other.to_string(), Vec::new()),
        };
        ApiError {
            code,
            message,
            fields,
        }
    }
}

impl From<ratchet_core::LedgerError> for ServiceError {
    fn from(e: ratchet_core::LedgerError) -> Self {
        use ratchet_core::LedgerErrorCode as C;
        match e.code {
            C::NotFound => Self::NotFound(e.message),
            C::Invalid | C::NoBaseline | C::RatchetNotImproved => Self::Invalid(e.message),
            C::Database => Self::Internal(e.message),
        }
    }
}

impl From<ratchet_core::ports::PortError> for ServiceError {
    fn from(e: ratchet_core::ports::PortError) -> Self {
        use ratchet_core::ports::PortError as P;
        match e {
            P::Unsupported(what) => {
                Self::Unsupported(format!("this engine does not support {what}"))
            }
            other => Self::Upstream(other.to_string()),
        }
    }
}

fn unsupported<T>(what: &str) -> ServiceResult<T> {
    Err(ServiceError::Unsupported(format!(
        "{what} is not available in this engine yet (provisional API)"
    )))
}

#[async_trait]
pub trait AppService: Send + Sync + 'static {
    // ---- the engine ---------------------------------------------------------------------------------------
    fn engine(&self) -> EngineInfo;

    /// Where the ledger lives; `None` for an in-memory ledger.
    fn ledger_path(&self) -> Option<String> {
        None
    }

    /// Every event the engine publishes. The server numbers them and keeps a window for `Last-Event-ID` resume.
    fn subscribe(&self) -> broadcast::Receiver<ApiEvent>;

    // ---- optimizations -----------------------------------------------------------------------------------
    async fn list_optimizations(&self, range: KpiRange) -> ServiceResult<OptimizationsSnapshot>;
    async fn create_optimization(
        &self,
        input: CreateOptimizationInput,
    ) -> ServiceResult<CreatedOptimization>;
    async fn get_optimization(&self, id: &str) -> ServiceResult<OptimizationDetail>;
    async fn start_optimization(&self, id: &str) -> ServiceResult<()>;
    async fn stop_optimization(&self, id: &str, reason: Option<String>) -> ServiceResult<()>;
    /// `metric`: one of the series' `options` keys; default the objective.
    async fn ratchet_history(
        &self,
        id: &str,
        metric: Option<String>,
    ) -> ServiceResult<RatchetSeries>;
    async fn winning_change(&self, id: &str) -> ServiceResult<WinningChangeVm>;
    async fn candidates(&self, id: &str) -> ServiceResult<Vec<CandidateVm>>;
    async fn spend(&self, id: &str) -> ServiceResult<SpendVm>;
    /// Compare the ledger's estimated infrastructure spend with billing and record correction rows (idempotent).
    async fn reconcile_costs(&self, id: &str) -> ServiceResult<CostReconcileReport>;
    /// `None` until the baseline has run.
    async fn calibration(&self, id: &str) -> ServiceResult<Option<Calibration>>;
    async fn protected_paths(&self, id: &str) -> ServiceResult<EffectiveProtected>;
    /// The receipt of the teardown that covered this run; `None` while the environment is alive.
    async fn receipt(&self, id: &str) -> ServiceResult<Option<TeardownReceiptVm>>;
    async fn open_pull_request(&self, id: &str) -> ServiceResult<PullRequestVm>;
    async fn report(&self, id: &str) -> ServiceResult<ReportVm>;
    async fn recent_activity(
        &self,
        optimization_id: Option<String>,
        limit: u32,
    ) -> ServiceResult<Vec<ActivityVm>>;

    // ---- agents ------------------------------------------------------------------------------------------
    async fn agent_events(
        &self,
        agent_id: &str,
        cursor: Option<i64>,
        limit: u32,
    ) -> ServiceResult<AgentEventsPage>;
    async fn pause_agent(&self, agent_id: &str) -> ServiceResult<()>;
    async fn resume_agent(&self, agent_id: &str) -> ServiceResult<()>;
    async fn message_agent(&self, agent_id: &str, text: &str) -> ServiceResult<()>;

    // ---- config and preflight ----------------------------------------------------------------------------
    /// Parse ratchet.yaml text (read-only; the repo owns the file).
    async fn validate_config(&self, yaml: &str) -> ServiceResult<ValidateConfigResult> {
        Ok(crate::views::validate_config_text(yaml))
    }
    /// "What's missing" for a run.
    async fn preflight(&self, input: PreflightInput) -> ServiceResult<Vec<PreflightItem>>;

    // ---- settings ----------------------------------------------------------------------------------------
    async fn settings(&self) -> ServiceResult<SettingsVm>;
    async fn set_aws_profile(&self, profile: &str) -> ServiceResult<SettingsVm>;
    /// Validated as a whole: nothing is saved if any field is invalid.
    async fn save_advanced_settings(&self, input: LiveSettingsInput) -> ServiceResult<SettingsVm>;
    async fn set_secret(&self, name: SecretName, value: &str) -> ServiceResult<SettingsVm>;
    async fn clear_secret(&self, name: SecretName) -> ServiceResult<SettingsVm>;
    /// Read-only probe (STS, GitHub, a model API); secrets are read inside the daemon.
    async fn test_connection(&self, target: ConnectionTarget) -> ServiceResult<ConnectionResultVm>;

    // ---- GitHub, repos -----------------------------------------------------------------------------------
    async fn github_status(&self) -> ServiceResult<GithubStatusVm>;
    /// Start the device flow: returns the code to show; the daemon polls and stores the token.
    async fn github_sign_in(&self) -> ServiceResult<GithubStatusVm>;
    async fn github_sign_out(&self) -> ServiceResult<GithubStatusVm>;
    async fn list_repos(&self, query: Option<String>) -> ServiceResult<Vec<RepoVm>>;
    async fn inspect_repo(&self, source: RepoSource) -> ServiceResult<RepoInspectVm>;

    // ---- AWS, agent CLIs ---------------------------------------------------------------------------------
    async fn aws_profiles(&self) -> ServiceResult<AwsProfilesVm>;
    /// `refresh`: probe the machine again ("Re-detect").
    async fn harnesses(&self, refresh: bool) -> ServiceResult<Vec<HarnessVm>>;

    // ---- templates ---------------------------------------------------------------------------------------
    async fn list_templates(&self, kind: Option<TemplateKind>) -> ServiceResult<Vec<TemplateInfo>> {
        Ok(ratchet_core::list_templates(TemplateFilter {
            kind,
            category: None,
        }))
    }
    async fn get_template(&self, id: &str) -> ServiceResult<TemplateInfo> {
        ratchet_core::get_template(id).ok_or_else(|| ServiceError::not_found("template", id))
    }
    /// "Use template" / "Add via pull request": commit the template's files on a branch and open the PR.
    async fn propose_template(
        &self,
        input: ProposeTemplateInput,
    ) -> ServiceResult<ProposeTemplateResult>;

    // ---- environments ------------------------------------------------------------------------------------
    async fn environments(&self) -> ServiceResult<EnvironmentsVm>;
    /// "End sandbox now". Refused (409) while a run is active.
    async fn end_environment(&self, sandbox_id: &str) -> ServiceResult<EnvironmentsVm>;

    // ---- overview (provisional) --------------------------------------------------------------------------
    async fn overview(&self, _range: OverviewRange) -> ServiceResult<OverviewVm> {
        unsupported("overview")
    }

    // ---- connectors (provisional) ------------------------------------------------------------------------
    async fn list_connectors(&self) -> ServiceResult<Vec<ConnectorVm>> {
        unsupported("connectors")
    }
    async fn create_connector(&self, _input: CreateConnectorInput) -> ServiceResult<ConnectorVm> {
        unsupported("connectors")
    }
    async fn get_connector(&self, _id: &str) -> ServiceResult<ConnectorVm> {
        unsupported("connectors")
    }
    async fn update_connector(
        &self,
        _id: &str,
        _input: UpdateConnectorInput,
    ) -> ServiceResult<ConnectorVm> {
        unsupported("connectors")
    }
    async fn delete_connector(&self, _id: &str) -> ServiceResult<()> {
        unsupported("connectors")
    }
    /// "Test" / "Sign in": probe the account and refresh its status.
    async fn test_connector(&self, _id: &str) -> ServiceResult<ConnectorTestVm> {
        unsupported("connectors")
    }
    async fn connector_selection(&self, _repo: &str) -> ServiceResult<ConnectorSelectionVm> {
        unsupported("connectors")
    }
    async fn save_connector_selection(
        &self,
        _input: SaveConnectorSelectionInput,
    ) -> ServiceResult<ConnectorSelectionVm> {
        unsupported("connectors")
    }

    // ---- environment drafts ------------------------------------------------------------------------------
    /// Records the draft (`drafting`), starts the setup agent in the background and returns at once.
    async fn create_environment_draft(
        &self,
        _input: CreateEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        unsupported("environment drafts")
    }
    async fn list_environment_drafts(&self) -> ServiceResult<EnvironmentDraftsVm> {
        unsupported("environment drafts")
    }
    async fn get_environment_draft(&self, _id: &str) -> ServiceResult<EnvironmentDraftVm> {
        unsupported("environment drafts")
    }
    /// Forget a draft that was not approved.
    async fn discard_environment_draft(&self, _id: &str) -> ServiceResult<()> {
        unsupported("environment drafts")
    }
    /// "Ask for a change": the same agent session continues; returns at once with the draft `drafting`.
    async fn revise_environment_draft(
        &self,
        _id: &str,
        _input: ReviseEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        unsupported("environment drafts")
    }
    /// Commits the files to the protected ref (or opens a pull request) and locks the draft: agents change code only,
    /// never the environment.
    async fn approve_environment_draft(
        &self,
        _id: &str,
        _input: ApproveEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        unsupported("environment drafts")
    }
}
