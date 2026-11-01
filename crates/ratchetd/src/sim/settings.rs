//! Settings, GitHub sign-in, repos, AWS profiles and agent CLIs of the simulation (all in memory; no secret is
//! stored, only whether one is set).

use std::sync::Arc;
use std::time::Duration;

use ratchet_core::enums::AgentKind;
use ratchet_core::enums::AgentsWhere;
use ratchet_core::list_templates;
use ratchet_core::ports::{ProposeTemplateInput, ProposeTemplateResult};
use ratchet_core::templates::{TemplateError, build_template_proposal};

use super::{DEFAULT_CONFIG_YAML, Inner, repo_slug};
use crate::api::validate::{validate_live_settings, validate_profile_name, validate_secret_format};
use crate::api::*;
use crate::service::{ServiceError, ServiceResult};

const SIGN_IN_FIRST: &str = "Sign in with GitHub first (Settings).";

#[derive(Debug, Clone, Default)]
pub struct GithubSim {
    pub signed_in: bool,
    pub login: Option<String>,
    pub source: Option<GithubSource>,
    pub flow: Option<GithubFlowVm>,
}

#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    pub aws_profile: String,
    pub live: LiveSettings,
    pub secrets: SecretsPresent,
    pub github: GithubSim,
}

impl SettingsState {
    pub fn demo() -> Self {
        Self {
            aws_profile: "acme-prod".into(),
            live: LiveSettings {
                aws_profile: "acme-prod".into(),
                aws_auth: AwsAuthKind::Sso,
                ..LiveSettings::default()
            },
            secrets: SecretsPresent {
                anthropic_api_key: true,
                openai_api_key: false,
                github_token: true,
            },
            github: GithubSim {
                signed_in: true,
                login: Some("srikanth235".into()),
                source: Some(GithubSource::Device),
                flow: None,
            },
        }
    }
}

struct DemoRepo {
    full_name: &'static str,
    private: bool,
    description: &'static str,
    pushed_at: &'static str,
}

const REPOS: [DemoRepo; 6] = [
    DemoRepo {
        full_name: "acme-org/shop-api",
        private: true,
        description: "Checkout and orders API",
        pushed_at: "2026-10-30T16:02:11Z",
    },
    DemoRepo {
        full_name: "acme-org/search-svc",
        private: true,
        description: "Search relevance service",
        pushed_at: "2026-10-29T09:41:00Z",
    },
    DemoRepo {
        full_name: "acme-org/demand-model",
        private: true,
        description: "Demand forecasting",
        pushed_at: "2026-10-27T12:15:30Z",
    },
    DemoRepo {
        full_name: "acme-org/web-app",
        private: true,
        description: "Storefront",
        pushed_at: "2026-10-25T08:00:00Z",
    },
    DemoRepo {
        full_name: "srikanth235/acme-cli",
        private: false,
        description: "Command line for Acme",
        pushed_at: "2026-10-20T19:20:45Z",
    },
    DemoRepo {
        full_name: "acme/search-api",
        private: false,
        description: "The search API of the demo",
        pushed_at: "2026-10-18T10:00:00Z",
    },
];

impl Inner {
    // ---- settings ----------------------------------------------------------------------------------------

    pub(crate) fn settings_vm(&self) -> SettingsVm {
        let st = self.st();
        let s = &st.settings;
        SettingsVm {
            aws_profile: s.aws_profile.clone(),
            aws_region: if s.live.aws_region.is_empty() {
                DEFAULT_AWS_REGION.into()
            } else {
                s.live.aws_region.clone()
            },
            secrets: s.secrets.clone(),
            advanced: AdvancedSettingsVm {
                values: LiveSettings {
                    aws_profile: s.aws_profile.clone(),
                    ..s.live.clone()
                },
                defaults: AdvancedDefaults {
                    tofu_binary: "tofu".into(),
                    module_dir: "infra/aws".into(),
                },
            },
            encryption_available: true,
            engine: self.engine(),
        }
    }

    pub(crate) fn set_aws_profile(&self, profile: &str) -> ServiceResult<SettingsVm> {
        if let Some(problem) = validate_profile_name(profile) {
            return Err(ServiceError::InvalidFields {
                message: problem.clone(),
                fields: vec![FieldError::new("profile", problem)],
            });
        }
        let profile = profile.trim().to_owned();
        {
            let mut st = self.st();
            let kind_sso = aws_profile_list()
                .iter()
                .any(|p| p.name == profile && p.kind == AwsProfileKind::Sso);
            st.settings.aws_profile.clone_from(&profile);
            st.settings.live.aws_profile.clone_from(&profile);
            if !profile.is_empty() {
                st.settings.live.aws_auth = if kind_sso {
                    AwsAuthKind::Sso
                } else {
                    AwsAuthKind::Profile
                };
            }
        }
        Ok(self.settings_vm())
    }

    pub(crate) fn save_advanced(&self, input: LiveSettingsInput) -> ServiceResult<SettingsVm> {
        let errors = validate_live_settings(&input);
        if !errors.is_empty() {
            return Err(ServiceError::InvalidFields {
                message: format!("{}: {}", errors[0].path, errors[0].message),
                fields: errors,
            });
        }
        {
            let mut st = self.st();
            let trim = |v: Option<String>| v.map(|s| s.trim().to_owned());
            let s = &mut st.settings;
            if let Some(v) = trim(input.aws_profile) {
                s.aws_profile.clone_from(&v);
                s.live.aws_profile = v;
            }
            if let Some(v) = trim(input.aws_region) {
                s.live.aws_region = v;
            }
            if let Some(v) = input.aws_auth {
                s.live.aws_auth = v;
            }
            if let Some(v) = trim(input.runner_image_ref) {
                s.live.runner_image_ref = v;
            }
            if let Some(v) = trim(input.bench_image_ref) {
                s.live.bench_image_ref = v;
            }
            if let Some(v) = trim(input.tofu_binary) {
                s.live.tofu_binary = v;
            }
            if let Some(v) = trim(input.module_dir) {
                s.live.module_dir = v;
            }
        }
        Ok(self.settings_vm())
    }

    pub(crate) fn set_secret(&self, name: SecretName, value: &str) -> ServiceResult<SettingsVm> {
        if let Some(problem) = validate_secret_format(name, value) {
            return Err(ServiceError::InvalidFields {
                message: problem.clone(),
                fields: vec![FieldError::new("value", problem)],
            });
        }
        {
            let mut st = self.st();
            st.settings.secrets.set(name, true);
            if name == SecretName::GithubToken {
                st.settings.github = GithubSim {
                    signed_in: true,
                    login: Some("token-user".into()),
                    source: Some(GithubSource::Token),
                    flow: None,
                };
            }
        }
        if name == SecretName::GithubToken {
            self.emit(ApiEvent::Github { signed_in: true });
        }
        Ok(self.settings_vm())
    }

    pub(crate) fn clear_secret(&self, name: SecretName) -> ServiceResult<SettingsVm> {
        {
            let mut st = self.st();
            st.settings.secrets.set(name, false);
            if name == SecretName::GithubToken {
                st.settings.github = GithubSim::default();
            }
        }
        if name == SecretName::GithubToken {
            self.emit(ApiEvent::Github { signed_in: false });
        }
        Ok(self.settings_vm())
    }

    pub(crate) fn test_connection(&self, target: ConnectionTarget) -> ConnectionResultVm {
        let st = self.st();
        let s = &st.settings;
        let (ok, detail) = match target {
            ConnectionTarget::Github => match (&s.github.signed_in, &s.github.login) {
                (true, Some(l)) => (true, format!("signed in as {l}")),
                (true, None) => (true, "signed in".into()),
                _ => (false, "Sign in with GitHub first.".into()),
            },
            ConnectionTarget::Aws => {
                if s.aws_profile.is_empty() {
                    (false, "Choose an AWS profile first.".into())
                } else {
                    let region = if s.live.aws_region.is_empty() {
                        DEFAULT_AWS_REGION
                    } else {
                        &s.live.aws_region
                    };
                    (
                        true,
                        format!(
                            "simulated: reached STS as account 000000000000 with profile {} in {region}",
                            s.aws_profile
                        ),
                    )
                }
            }
            ConnectionTarget::Anthropic => {
                secret_probe(s.secrets.anthropic_api_key, "an Anthropic")
            }
            ConnectionTarget::Openai => secret_probe(s.secrets.openai_api_key, "an OpenAI"),
        };
        ConnectionResultVm { target, ok, detail }
    }

    // ---- GitHub ------------------------------------------------------------------------------------------

    pub(crate) fn github_status(&self) -> GithubStatusVm {
        let st = self.st();
        let g = &st.settings.github;
        GithubStatusVm {
            signed_in: g.signed_in,
            login: g.login.clone(),
            source: g.source,
            client_id_configured: true,
            flow: g.flow.clone(),
        }
    }

    pub(crate) fn github_sign_in(self: &Arc<Self>) -> GithubStatusVm {
        {
            let mut st = self.st();
            if st.settings.github.signed_in {
                drop(st);
                return self.github_status();
            }
            st.settings.github.flow = Some(GithubFlowVm {
                state: GithubFlowState::Pending,
                user_code: "WDJB-MJHT".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_at: self.clock.now() + 15 * 60_000,
                message: None,
            });
        }
        if self.opts.auto_run && self.opts.github_flow_ms > 0 {
            let me = Arc::clone(self);
            let ms = self.opts.github_flow_ms;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                me.complete_github_flow();
            });
        }
        self.github_status()
    }

    pub(crate) fn complete_github_flow(&self) {
        let done = {
            let mut st = self.st();
            let g = &mut st.settings.github;
            if g.flow.is_some() && !g.signed_in {
                *g = GithubSim {
                    signed_in: true,
                    login: Some("srikanth235".into()),
                    source: Some(GithubSource::Device),
                    flow: None,
                };
                st.settings.secrets.github_token = true;
                true
            } else {
                false
            }
        };
        if done {
            self.emit(ApiEvent::Github { signed_in: true });
        }
    }

    pub(crate) fn github_sign_out(&self) -> GithubStatusVm {
        {
            let mut st = self.st();
            st.settings.github = GithubSim::default();
            st.settings.secrets.github_token = false;
        }
        self.emit(ApiEvent::Github { signed_in: false });
        self.github_status()
    }

    fn need_github(&self) -> ServiceResult<()> {
        if self.st().settings.github.signed_in {
            Ok(())
        } else {
            Err(ServiceError::NotReady(SIGN_IN_FIRST.into()))
        }
    }

    pub(crate) fn list_repos(&self, query: Option<&str>) -> ServiceResult<Vec<RepoVm>> {
        self.need_github()?;
        let q = query.unwrap_or("").trim().to_lowercase();
        Ok(REPOS
            .iter()
            .filter(|r| q.is_empty() || r.full_name.to_lowercase().contains(&q))
            .map(|r| RepoVm {
                full_name: r.full_name.into(),
                default_branch: "main".into(),
                private: r.private,
                description: Some(r.description.into()),
                pushed_at: Some(r.pushed_at.into()),
            })
            .collect())
    }

    /// The ratchet.yaml of a repo the simulation knows.
    pub(crate) fn known_repo_yaml(&self, slug: &str) -> Option<String> {
        match slug {
            "acme/search-api" => Some(DEFAULT_CONFIG_YAML.to_owned()),
            "acme-org/demand-model" => {
                ratchet_core::render_template("forecast-backtest", &Default::default())
                    .ok()
                    .map(|r| r.files[0].content.clone())
            }
            _ => None,
        }
    }

    pub(crate) async fn inspect_repo(&self, source: RepoSource) -> ServiceResult<RepoInspectVm> {
        match &source {
            RepoSource::Github { repo } => {
                self.need_github()?;
                let slug = repo_slug(repo).ok_or_else(|| ServiceError::InvalidFields {
                    message: "use owner/name".into(),
                    fields: vec![FieldError::new("source.repo", "use owner/name")],
                })?;
                if !REPOS.iter().any(|r| r.full_name == slug) {
                    return Err(ServiceError::not_found("repository", &slug));
                }
                let yaml = self.known_repo_yaml(&slug);
                Ok(self.inspect_result(source, Some(slug), "main".into(), yaml, None))
            }
            RepoSource::Local { path } => {
                let dir = std::path::Path::new(path);
                if !dir.is_dir() {
                    return Ok(self.inspect_result(
                        source.clone(),
                        None,
                        "main".into(),
                        None,
                        Some(format!("{path} is not a folder")),
                    ));
                }
                let yaml = tokio::fs::read_to_string(dir.join("ratchet.yaml"))
                    .await
                    .ok();
                let problem =
                    (!dir.join(".git").exists()).then(|| format!("{path} is not a git repository"));
                Ok(self.inspect_result(source.clone(), None, "main".into(), yaml, problem))
            }
        }
    }

    /// Parse a repo's ratchet.yaml for step 2.
    fn inspect_result(
        &self,
        source: RepoSource,
        repo: Option<String>,
        base_ref: String,
        config_yaml: Option<String>,
        problem: Option<String>,
    ) -> RepoInspectVm {
        let (config, errors) = match &config_yaml {
            Some(y) => {
                let v = crate::views::validate_config_text(y);
                (v.summary, v.errors)
            }
            None => (None, Vec::new()),
        };
        let templates = if config_yaml.is_none() && problem.is_none() {
            list_templates(ratchet_core::templates::TemplateFilter {
                kind: Some(ratchet_core::enums::TemplateKind::Template),
                category: None,
            })
        } else {
            Vec::new()
        };
        RepoInspectVm {
            source,
            repo,
            base_ref,
            config_yaml,
            config,
            errors,
            templates,
            problem,
        }
    }

    // ---- AWS and agent CLIs ------------------------------------------------------------------------------

    pub(crate) fn aws_profiles(&self) -> AwsProfilesVm {
        let st = self.st();
        AwsProfilesVm {
            profiles: aws_profile_list(),
            sso_sessions: vec!["acme".into()],
            selected: st.settings.aws_profile.clone(),
            region: if st.settings.live.aws_region.is_empty() {
                DEFAULT_AWS_REGION.into()
            } else {
                st.settings.live.aws_region.clone()
            },
            default_region: DEFAULT_AWS_REGION.into(),
            files_found: true,
        }
    }

    pub(crate) fn harnesses(&self) -> Vec<HarnessVm> {
        vec![
            HarnessVm {
                kind: HarnessKind::ClaudeCode,
                installed: true,
                version: Some("2.1.314".into()),
                path: Some("/usr/local/bin/claude".into()),
                auth: HarnessAuth::SignedIn,
                hint: "signed in".into(),
            },
            HarnessVm {
                kind: HarnessKind::Codex,
                installed: true,
                version: Some("0.166.0".into()),
                path: Some("/usr/local/bin/codex".into()),
                auth: HarnessAuth::SignedOut,
                hint: "run `codex login` in a terminal".into(),
            },
        ]
    }

    // ---- preflight ---------------------------------------------------------------------------------------

    pub(crate) fn preflight(&self, input: PreflightInput) -> Vec<PreflightItem> {
        let item = |id: &str, label: &str, state: PreflightState, detail: &str| PreflightItem {
            id: id.into(),
            label: label.into(),
            state,
            detail: detail.into(),
        };
        if input.source == PreflightSource::Demo {
            return vec![item(
                "demo",
                "Demo run",
                PreflightState::Ok,
                "simulation: scripted agents, mock AWS, nothing is installed or billed",
            )];
        }
        let (gh, aws_profile, region, secrets) = {
            let st = self.st();
            let s = &st.settings;
            (
                s.github.clone(),
                s.aws_profile.clone(),
                if s.live.aws_region.is_empty() {
                    DEFAULT_AWS_REGION.to_owned()
                } else {
                    s.live.aws_region.clone()
                },
                s.secrets.clone(),
            )
        };
        let mut items = Vec::new();
        items.push(if gh.signed_in {
            item(
                "github",
                "GitHub",
                PreflightState::Ok,
                &gh.login
                    .map_or("signed in".into(), |l| format!("signed in as {l}")),
            )
        } else {
            item(
                "github",
                "Sign in with GitHub",
                PreflightState::Fail,
                "Ratchet pushes candidate branches and opens the pull request as you",
            )
        });
        items.push(if aws_profile.is_empty() {
            item(
                "aws",
                "Connect AWS",
                PreflightState::Fail,
                "the evaluator runs in a sandbox in your AWS account",
            )
        } else {
            item(
                "aws",
                "AWS",
                PreflightState::Ok,
                &format!("{aws_profile} \u{00b7} {region}"),
            )
        });
        let named = |k: AgentKind| match k {
            AgentKind::Codex => "Codex",
            _ => "Claude Code",
        };
        if input.agents_where == AgentsWhere::Local {
            let found = self.harnesses();
            let kinds: Vec<AgentKind> = input
                .agents
                .iter()
                .copied()
                .filter(|a| matches!(a, AgentKind::ClaudeCode | AgentKind::Codex))
                .collect();
            if kinds.is_empty() {
                items.push(item(
                    "agents",
                    "Choose an agent",
                    PreflightState::Fail,
                    "no agent is selected",
                ));
            }
            for k in kinds {
                let want = if k == AgentKind::Codex {
                    HarnessKind::Codex
                } else {
                    HarnessKind::ClaudeCode
                };
                let label = named(k);
                match found.iter().find(|h| h.kind == want) {
                    Some(h) if h.installed && h.auth != HarnessAuth::SignedOut => items.push(item(
                        &format!("agent-{k}"),
                        label,
                        PreflightState::Ok,
                        &format!(
                            "{} \u{00b7} {}",
                            h.version.as_deref().unwrap_or("installed"),
                            if h.auth == HarnessAuth::ApiKey {
                                "API key"
                            } else {
                                "signed in"
                            }
                        ),
                    )),
                    Some(h) => items.push(item(
                        &format!("agent-{k}"),
                        &format!("{label} is not ready"),
                        PreflightState::Fail,
                        &h.hint,
                    )),
                    None => items.push(item(
                        &format!("agent-{k}"),
                        &format!("{label} is not ready"),
                        PreflightState::Fail,
                        &format!("install {label} on this machine"),
                    )),
                }
            }
        } else {
            for a in &input.agents {
                let (present, vendor) = match a {
                    AgentKind::ClaudeCode => (secrets.anthropic_api_key, "Anthropic"),
                    AgentKind::Codex => (secrets.openai_api_key, "OpenAI"),
                    AgentKind::Grok | AgentKind::Mock => continue,
                };
                items.push(if present {
                    item(
                        &format!("key-{a}"),
                        &format!("{vendor} API key"),
                        PreflightState::Ok,
                        "stored in the keychain",
                    )
                } else {
                    item(
                        &format!("key-{a}"),
                        &format!("{vendor} API key"),
                        PreflightState::Fail,
                        "cloud runners need an API key (Settings > Agents)",
                    )
                });
            }
        }
        items
    }

    /// What blocks a real run (the simulation only checks GitHub).
    pub(crate) fn missing_for_live(&self, _input: &CreateOptimizationInput) -> Vec<String> {
        if self.st().settings.github.signed_in {
            Vec::new()
        } else {
            vec!["sign in with GitHub".into()]
        }
    }

    // ---- templates ---------------------------------------------------------------------------------------

    pub(crate) fn propose_template(
        &self,
        input: ProposeTemplateInput,
    ) -> ServiceResult<ProposeTemplateResult> {
        self.need_github()?;
        let slug = repo_slug(&input.repo_url).ok_or_else(|| ServiceError::InvalidFields {
            message: format!("{} is not a GitHub repository (owner/name)", input.repo_url),
            fields: vec![FieldError::new("repoUrl", "use owner/name")],
        })?;
        let p =
            build_template_proposal(&input.template_id, &input.render_options()).map_err(|e| {
                match e {
                    TemplateError::Unknown(_) => ServiceError::NotFound(e.to_string()),
                    TemplateError::Demo { .. } => ServiceError::Invalid(e.to_string()),
                }
            })?;
        let n = 200.0 + self.rng().next_f64() * 700.0;
        let sha = self.rng().hex(40);
        Ok(ProposeTemplateResult {
            pr_url: format!("https://github.com/{slug}/pull/{}", n as u32),
            branch: p.branch,
            sha,
            template_id: p.template_id,
            files: p.files.into_iter().map(|f| f.path).collect(),
        })
    }
}

fn secret_probe(present: bool, what: &str) -> (bool, String) {
    if present {
        (true, "simulated: the key is accepted".into())
    } else {
        (false, format!("Store {what} API key first."))
    }
}

fn aws_profile_list() -> Vec<AwsProfileVm> {
    vec![
        AwsProfileVm {
            name: "acme-prod".into(),
            kind: AwsProfileKind::Sso,
            source: AwsProfileSource::Config,
            sso_session: Some("acme".into()),
        },
        AwsProfileVm {
            name: "acme-staging".into(),
            kind: AwsProfileKind::Sso,
            source: AwsProfileSource::Config,
            sso_session: Some("acme".into()),
        },
        AwsProfileVm {
            name: "personal-dev".into(),
            kind: AwsProfileKind::Keys,
            source: AwsProfileSource::Credentials,
            sso_session: None,
        },
    ]
}
