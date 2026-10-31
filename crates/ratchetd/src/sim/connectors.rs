//! Connectors of the simulation (PROVISIONAL API): saved GitHub / AWS / agent accounts and the per-repo selection.

use std::collections::HashMap;

use ratchet_core::enums::{AgentKind, AgentsWhere};

use super::Inner;
use crate::api::*;
use crate::service::{ServiceError, ServiceResult};

#[derive(Debug, Default)]
pub struct ConnectorsState {
    pub items: Vec<ConnectorVm>,
    pub selections: HashMap<String, ConnectorSelectionInput>,
    next: u32,
}

fn base(id: &str, kind: ConnectorKind, name: &str, detail: &str) -> ConnectorVm {
    ConnectorVm {
        id: id.into(),
        kind,
        name: name.into(),
        detail: detail.into(),
        status: ConnectorStatus::Connected,
        status_text: "Connected".into(),
        is_default: false,
        cost_note: None,
        github: None,
        aws: None,
        agent: None,
    }
}

impl ConnectorsState {
    /// The accounts of `design/v02/2-connectors.png`.
    pub fn demo() -> Self {
        let mut gh_personal = base(
            "conn_gh_personal",
            ConnectorKind::Github,
            "srikanth235",
            "Personal \u{00b7} 42 repos",
        );
        gh_personal.is_default = true;
        gh_personal.github = Some(GithubConnectorInfo {
            login: "srikanth235".into(),
            account_kind: GithubAccountKind::Personal,
            repo_count: Some(42),
        });
        let mut gh_org = base(
            "conn_gh_acme",
            ConnectorKind::Github,
            "acme-org",
            "GitHub App \u{00b7} 18 repos",
        );
        gh_org.github = Some(GithubConnectorInfo {
            login: "acme-org".into(),
            account_kind: GithubAccountKind::App,
            repo_count: Some(18),
        });

        let mut aws_prod = base(
            "conn_aws_prod",
            ConnectorKind::Aws,
            "acme-prod",
            "SSO \u{00b7} us-east-1 \u{00b7} cap $500 / month",
        );
        aws_prod.is_default = true;
        aws_prod.aws = Some(AwsConnectorInfo {
            auth: AwsConnectorAuth::Sso,
            profile: Some("acme-prod".into()),
            region: "us-east-1".into(),
            account_id: Some("111122223333".into()),
            cap_usd_per_month: Some(500.0),
            remaining_usd_this_month: Some(412.0),
        });
        aws_prod.cost_note = Some("$412 left this month".into());
        let mut aws_dev = base(
            "conn_aws_dev",
            ConnectorKind::Aws,
            "personal-dev",
            "~/.aws profile \u{00b7} us-west-2 \u{00b7} cap $100 / month",
        );
        aws_dev.aws = Some(AwsConnectorInfo {
            auth: AwsConnectorAuth::Profile,
            profile: Some("personal-dev".into()),
            region: "us-west-2".into(),
            account_id: Some("444455556666".into()),
            cap_usd_per_month: Some(100.0),
            remaining_usd_this_month: Some(96.0),
        });
        aws_dev.cost_note = Some("$96 left this month".into());
        let mut aws_stg = base(
            "conn_aws_staging",
            ConnectorKind::Aws,
            "acme-staging",
            "SSO \u{00b7} eu-west-1",
        );
        aws_stg.status = ConnectorStatus::Expired;
        aws_stg.status_text = "Expired".into();
        aws_stg.cost_note = Some("Expired".into());
        aws_stg.aws = Some(AwsConnectorInfo {
            auth: AwsConnectorAuth::Sso,
            profile: Some("acme-staging".into()),
            region: "eu-west-1".into(),
            account_id: Some("777788889999".into()),
            cap_usd_per_month: None,
            remaining_usd_this_month: None,
        });

        let mut cc_login = base(
            "conn_agent_cc_login",
            ConnectorKind::Agent,
            "Claude Code \u{00b7} your login",
            "Found on this Mac \u{00b7} v2.1.314",
        );
        cc_login.is_default = true;
        cc_login.status_text = "Signed in".into();
        cc_login.cost_note = Some("Free while this Mac is awake".into());
        cc_login.agent = Some(AgentConnectorInfo {
            agent: AgentKind::ClaudeCode,
            credential: AgentCredential::Login,
            version: Some("2.1.314".into()),
            key_hint: None,
            runs_in: vec![AgentsWhere::Local],
        });
        let mut cc_key = base(
            "conn_agent_cc_key",
            ConnectorKind::Agent,
            "Claude Code \u{00b7} team key",
            "API key \u{00b7}\u{00b7}\u{00b7}\u{00b7}4f2a \u{00b7} also runs in the cloud",
        );
        cc_key.status_text = "Valid".into();
        cc_key.cost_note = Some("Billed to the key".into());
        cc_key.agent = Some(AgentConnectorInfo {
            agent: AgentKind::ClaudeCode,
            credential: AgentCredential::ApiKey,
            version: None,
            key_hint: Some("\u{00b7}\u{00b7}\u{00b7}\u{00b7}4f2a".into()),
            runs_in: vec![AgentsWhere::Cloud, AgentsWhere::Local],
        });
        let mut codex = base(
            "conn_agent_codex_login",
            ConnectorKind::Agent,
            "Codex \u{00b7} your login",
            "Found on this Mac \u{00b7} v0.166 \u{00b7} run codex login",
        );
        codex.status = ConnectorStatus::SignedOut;
        codex.status_text = "Signed out".into();
        codex.cost_note = Some("Signed out".into());
        codex.agent = Some(AgentConnectorInfo {
            agent: AgentKind::Codex,
            credential: AgentCredential::Login,
            version: Some("0.166.0".into()),
            key_hint: None,
            runs_in: vec![AgentsWhere::Local],
        });

        Self {
            items: vec![
                gh_personal,
                gh_org,
                aws_prod,
                aws_dev,
                aws_stg,
                cc_login,
                cc_key,
                codex,
            ],
            selections: HashMap::new(),
            next: 0,
        }
    }

    fn next_id(&mut self) -> String {
        self.next += 1;
        format!("conn_new_{}", self.next)
    }
}

fn status_text(s: ConnectorStatus, kind: ConnectorKind) -> &'static str {
    match (s, kind) {
        (ConnectorStatus::Connected, ConnectorKind::Agent) => "Signed in",
        (ConnectorStatus::Connected, _) => "Connected",
        (ConnectorStatus::Expired, _) => "Expired",
        (ConnectorStatus::SignedOut, _) => "Signed out",
        (ConnectorStatus::Invalid, _) => "Invalid",
        (ConnectorStatus::Unknown, _) => "Not tested",
    }
}

fn invalid(path: &str, message: &str) -> ServiceError {
    ServiceError::InvalidFields {
        message: message.into(),
        fields: vec![FieldError::new(path, message)],
    }
}

fn aws_detail(a: &AwsConnectorInfo) -> String {
    let auth = match a.auth {
        AwsConnectorAuth::Sso => "SSO",
        AwsConnectorAuth::Profile => "~/.aws profile",
        AwsConnectorAuth::Role => "CloudFormation role",
    };
    match a.cap_usd_per_month {
        Some(c) => format!("{auth} \u{00b7} {} \u{00b7} cap ${c:.0} / month", a.region),
        None => format!("{auth} \u{00b7} {}", a.region),
    }
}

impl Inner {
    pub(crate) fn list_connectors(&self) -> Vec<ConnectorVm> {
        self.st().connectors.items.clone()
    }

    pub(crate) fn get_connector(&self, id: &str) -> ServiceResult<ConnectorVm> {
        self.st()
            .connectors
            .items
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| ServiceError::not_found("connector", id))
    }

    pub(crate) fn create_connector(
        &self,
        input: CreateConnectorInput,
    ) -> ServiceResult<ConnectorVm> {
        let mut st = self.st();
        let id = st.connectors.next_id();
        let first_of_kind =
            |st: &super::State, k: ConnectorKind| !st.connectors.items.iter().any(|c| c.kind == k);
        let vm = match input {
            CreateConnectorInput::Github { name, token } => {
                if let Some(t) = &token
                    && let Some(p) =
                        crate::api::validate::validate_secret_format(SecretName::GithubToken, t)
                {
                    return Err(invalid("token", &p));
                }
                let login = name.unwrap_or_else(|| "new-account".into());
                let mut c = base(
                    &id,
                    ConnectorKind::Github,
                    &login,
                    "Personal \u{00b7} 0 repos",
                );
                c.github = Some(GithubConnectorInfo {
                    login,
                    account_kind: GithubAccountKind::Personal,
                    repo_count: None,
                });
                if token.is_none() {
                    c.status = ConnectorStatus::Unknown;
                    c.status_text = "Not tested".into();
                }
                c
            }
            CreateConnectorInput::Aws {
                name,
                auth,
                profile,
                region,
                cap_usd_per_month,
            } => {
                if name.trim().is_empty() {
                    return Err(invalid("name", "name is required"));
                }
                if let Some(cap) = cap_usd_per_month
                    && !(cap.is_finite() && cap > 0.0)
                {
                    return Err(invalid("capUsdPerMonth", "must be a positive number"));
                }
                let region = region.unwrap_or_else(|| DEFAULT_AWS_REGION.into());
                if !crate::api::validate::region_ok(&region) {
                    return Err(invalid(
                        "region",
                        "Use a region id such as us-east-1 or eu-west-2",
                    ));
                }
                let info = AwsConnectorInfo {
                    auth,
                    profile,
                    region,
                    account_id: None,
                    cap_usd_per_month,
                    remaining_usd_this_month: cap_usd_per_month,
                };
                let mut c = base(&id, ConnectorKind::Aws, name.trim(), &aws_detail(&info));
                c.cost_note = info
                    .remaining_usd_this_month
                    .map(|r| format!("${r:.0} left this month"));
                c.aws = Some(info);
                c
            }
            CreateConnectorInput::Agent {
                name,
                agent,
                credential,
                api_key,
                runs_in,
            } => {
                if name.trim().is_empty() {
                    return Err(invalid("name", "name is required"));
                }
                let key_hint = match (&credential, &api_key) {
                    (AgentCredential::ApiKey, None) => {
                        return Err(invalid("apiKey", "an API key is required"));
                    }
                    (AgentCredential::ApiKey, Some(k)) => {
                        let secret = if agent == AgentKind::Codex {
                            SecretName::OpenaiApiKey
                        } else {
                            SecretName::AnthropicApiKey
                        };
                        if let Some(p) = crate::api::validate::validate_secret_format(secret, k) {
                            return Err(invalid("apiKey", &p));
                        }
                        Some(format!(
                            "\u{00b7}\u{00b7}\u{00b7}\u{00b7}{}",
                            &k[k.len().saturating_sub(4)..]
                        ))
                    }
                    _ => None,
                };
                let runs_in = runs_in.unwrap_or_else(|| {
                    if credential == AgentCredential::ApiKey {
                        vec![AgentsWhere::Cloud, AgentsWhere::Local]
                    } else {
                        vec![AgentsWhere::Local]
                    }
                });
                let mut c = base(
                    &id,
                    ConnectorKind::Agent,
                    name.trim(),
                    &key_hint
                        .as_ref()
                        .map_or("Found on this Mac".to_owned(), |h| format!("API key {h}")),
                );
                c.status_text = if key_hint.is_some() {
                    "Valid".into()
                } else {
                    "Signed in".into()
                };
                c.cost_note = Some(if key_hint.is_some() {
                    "Billed to the key".into()
                } else {
                    "Free while this Mac is awake".into()
                });
                c.agent = Some(AgentConnectorInfo {
                    agent,
                    credential,
                    version: None,
                    key_hint,
                    runs_in,
                });
                c
            }
        };
        let mut vm = vm;
        vm.is_default = first_of_kind(&st, vm.kind);
        st.connectors.items.push(vm.clone());
        drop(st);
        self.emit(ApiEvent::Connector {
            connector_id: vm.id.clone(),
            status: Some(vm.status),
        });
        Ok(vm)
    }

    pub(crate) fn update_connector(
        &self,
        id: &str,
        input: UpdateConnectorInput,
    ) -> ServiceResult<ConnectorVm> {
        let mut st = self.st();
        let items = &mut st.connectors.items;
        let idx = items
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| ServiceError::not_found("connector", id))?;
        if let Some(name) = &input.name {
            if name.trim().is_empty() {
                return Err(invalid("name", "name must not be empty"));
            }
            items[idx].name = name.trim().to_owned();
        }
        if let Some(cap) = input.cap_usd_per_month {
            if !(cap.is_finite() && cap > 0.0) {
                return Err(invalid("capUsdPerMonth", "must be a positive number"));
            }
            let Some(aws) = items[idx].aws.as_mut() else {
                return Err(invalid("capUsdPerMonth", "only AWS connectors have a cap"));
            };
            aws.cap_usd_per_month = Some(cap);
            aws.remaining_usd_this_month = Some(cap);
            items[idx].detail = aws_detail(aws);
            items[idx].cost_note = Some(format!("${cap:.0} left this month"));
        }
        if let Some(region) = &input.region {
            if !crate::api::validate::region_ok(region) {
                return Err(invalid(
                    "region",
                    "Use a region id such as us-east-1 or eu-west-2",
                ));
            }
            let Some(aws) = items[idx].aws.as_mut() else {
                return Err(invalid("region", "only AWS connectors have a region"));
            };
            aws.region.clone_from(region);
            items[idx].detail = aws_detail(aws);
        }
        if let Some(key) = &input.api_key {
            let Some(agent) = items[idx].agent.as_mut() else {
                return Err(invalid("apiKey", "only agent connectors have an API key"));
            };
            let secret = if agent.agent == AgentKind::Codex {
                SecretName::OpenaiApiKey
            } else {
                SecretName::AnthropicApiKey
            };
            if let Some(p) = crate::api::validate::validate_secret_format(secret, key) {
                return Err(invalid("apiKey", &p));
            }
            agent.credential = AgentCredential::ApiKey;
            agent.key_hint = Some(format!(
                "\u{00b7}\u{00b7}\u{00b7}\u{00b7}{}",
                &key[key.len().saturating_sub(4)..]
            ));
        }
        if input.is_default == Some(true) {
            let kind = items[idx].kind;
            for c in items.iter_mut().filter(|c| c.kind == kind) {
                c.is_default = false;
            }
            items[idx].is_default = true;
        } else if input.is_default == Some(false) {
            items[idx].is_default = false;
        }
        Ok(items[idx].clone())
    }

    pub(crate) fn delete_connector(&self, id: &str) -> ServiceResult<()> {
        let mut st = self.st();
        let items = &mut st.connectors.items;
        let idx = items
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| ServiceError::not_found("connector", id))?;
        let removed = items.remove(idx);
        if removed.is_default
            && let Some(next) = items.iter_mut().find(|c| c.kind == removed.kind)
        {
            next.is_default = true;
        }
        for sel in st.connectors.selections.values_mut() {
            if sel.github_connector_id.as_deref() == Some(id) {
                sel.github_connector_id = None;
            }
            if sel.aws_connector_id.as_deref() == Some(id) {
                sel.aws_connector_id = None;
            }
            sel.agents.retain(|a| a.connector_id != id);
        }
        drop(st);
        self.emit(ApiEvent::Connector {
            connector_id: id.to_owned(),
            status: None,
        });
        Ok(())
    }

    /// "Test" / "Sign in": an expired or signed-out connector comes back as connected.
    pub(crate) fn test_connector(&self, id: &str) -> ServiceResult<ConnectorTestVm> {
        let mut st = self.st();
        let c = st
            .connectors
            .items
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| ServiceError::not_found("connector", id))?;
        let was = c.status;
        c.status = ConnectorStatus::Connected;
        c.status_text = status_text(c.status, c.kind).into();
        if matches!(c.kind, ConnectorKind::Aws) {
            c.cost_note = c
                .aws
                .as_ref()
                .and_then(|a| a.remaining_usd_this_month)
                .map(|r| format!("${r:.0} left this month"))
                .or(Some("Connected".into()));
        } else if c.kind == ConnectorKind::Agent {
            c.cost_note = Some(match c.agent.as_ref().map(|a| a.credential) {
                Some(AgentCredential::ApiKey) => "Billed to the key".into(),
                _ => "Free while this Mac is awake".into(),
            });
        }
        let vm = c.clone();
        drop(st);
        if was != ConnectorStatus::Connected {
            self.emit(ApiEvent::Connector {
                connector_id: id.to_owned(),
                status: Some(vm.status),
            });
        }
        Ok(ConnectorTestVm {
            ok: true,
            detail: if was == ConnectorStatus::Connected {
                "simulated: still connected".into()
            } else {
                "simulated: signed in again".into()
            },
            connector: vm,
        })
    }

    fn default_selection(items: &[ConnectorVm]) -> ConnectorSelectionInput {
        let default_of = |k: ConnectorKind| {
            items
                .iter()
                .find(|c| c.kind == k && c.is_default && c.status == ConnectorStatus::Connected)
                .map(|c| c.id.clone())
        };
        ConnectorSelectionInput {
            github_connector_id: default_of(ConnectorKind::Github),
            aws_connector_id: default_of(ConnectorKind::Aws),
            agents: items
                .iter()
                .filter(|c| {
                    c.kind == ConnectorKind::Agent
                        && c.is_default
                        && c.status == ConnectorStatus::Connected
                })
                .map(|c| AgentSelection {
                    connector_id: c.id.clone(),
                    runs_in: c
                        .agent
                        .as_ref()
                        .and_then(|a| a.runs_in.first().copied())
                        .unwrap_or(AgentsWhere::Local),
                })
                .collect(),
        }
    }

    pub(crate) fn connector_selection(&self, repo: &str) -> ServiceResult<ConnectorSelectionVm> {
        if repo.trim().is_empty() {
            return Err(invalid("repo", "repo is required"));
        }
        let st = self.st();
        Ok(match st.connectors.selections.get(repo) {
            Some(s) => ConnectorSelectionVm {
                repo: repo.into(),
                selection: s.clone(),
                remembered: true,
            },
            None => ConnectorSelectionVm {
                repo: repo.into(),
                selection: Inner::default_selection(&st.connectors.items),
                remembered: false,
            },
        })
    }

    pub(crate) fn save_connector_selection(
        &self,
        input: SaveConnectorSelectionInput,
    ) -> ServiceResult<ConnectorSelectionVm> {
        if input.repo.trim().is_empty() {
            return Err(invalid("repo", "repo is required"));
        }
        let mut st = self.st();
        let find = |id: &str, kind: ConnectorKind| {
            st.connectors
                .items
                .iter()
                .find(|c| c.id == id && c.kind == kind)
        };
        let mut fields = Vec::new();
        if let Some(id) = &input.selection.github_connector_id
            && find(id, ConnectorKind::Github).is_none()
        {
            fields.push(FieldError::new(
                "selection.githubConnectorId",
                "not a GitHub connector",
            ));
        }
        if let Some(id) = &input.selection.aws_connector_id {
            match find(id, ConnectorKind::Aws) {
                None => fields.push(FieldError::new(
                    "selection.awsConnectorId",
                    "not an AWS connector",
                )),
                Some(c) if c.status == ConnectorStatus::Expired => {
                    fields.push(FieldError::new(
                        "selection.awsConnectorId",
                        "this AWS connector has expired; sign in again",
                    ));
                }
                Some(_) => {}
            }
        }
        for (i, a) in input.selection.agents.iter().enumerate() {
            match find(&a.connector_id, ConnectorKind::Agent) {
                None => fields.push(FieldError::new(
                    format!("selection.agents[{i}].connectorId"),
                    "not an agent connector",
                )),
                Some(c)
                    if !c
                        .agent
                        .as_ref()
                        .is_some_and(|x| x.runs_in.contains(&a.runs_in)) =>
                {
                    fields.push(FieldError::new(
                        format!("selection.agents[{i}].runsIn"),
                        "this connector cannot run there",
                    ));
                }
                Some(_) => {}
            }
        }
        if !fields.is_empty() {
            return Err(ServiceError::InvalidFields {
                message: "the connector selection is invalid".into(),
                fields,
            });
        }
        st.connectors
            .selections
            .insert(input.repo.clone(), input.selection.clone());
        Ok(ConnectorSelectionVm {
            repo: input.repo,
            selection: input.selection,
            remembered: true,
        })
    }
}
