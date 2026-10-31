//! `AwsCloudCompute`: per-agent IAM identities, SSM secrets, EC2 runners and bench machines, SSM remote commands.
//! (TS `compute/aws.ts`.) Every id is validated before it reaches an API call, a tag, an IAM name or a policy.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use aws_sdk_ec2::types as ec2t;
use aws_sdk_iam::types as iamt;
use aws_sdk_ssm::types as ssmt;
use base64::Engine as _;
use regex::Regex;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::user_data::{RunnerNetworkOptions, bench_user_data, deps_user_data, runner_user_data};
use crate::civil;
use crate::error::{Error, Result, sdk};
use crate::identity::CallerIdentity;
use crate::util::{
    DEFAULT_AGENT_ROLE_PATH, TAG_AGENT, TAG_OPT, TAG_PURPOSE, TAG_ROLE, TAG_SANDBOX, arn_name,
    assert_valid_id, assert_valid_instance_id, bounded_name, json_str, sh_quote,
};
use ratchet_core::enums::InstanceRole;
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{
    AgentIdentity, BenchLaunchPurpose, BenchLaunchSpec, Clock, CloudCompute, InstanceInfo,
    InstanceState, PortResult, RemoteCommand, RemoteCommandResult, RunnerLaunchSpec,
    SandboxOutputs, SecretRef,
};

pub const KILL_SWITCH_POLICY_NAME: &str = "ratchet-kill-switch";
pub const AGENT_POLICY_NAME: &str = "ratchet-agent-permissions";

#[derive(Debug, Clone, Default)]
pub struct AwsComputeConfig {
    pub sandbox_id: String,
    pub region: String,
    /// `agent_boundary_ref` output.
    pub agent_boundary_arn: String,
    /// `agent_role_path` output, e.g. `/ratchet/agent/`.
    pub agent_role_path: String,
    /// Agent subnet (`network_ref.subnet_id`).
    pub subnet_id: String,
    /// Private bench subnet with no route to the internet (`network_ref.bench_subnet_id`). Required to launch bench
    /// instances: there is deliberately no fallback to `subnet_id`, which may be public.
    pub bench_subnet_id: Option<String>,
    pub agent_runner_security_group_id: String,
    pub bench_security_group_id: String,
    /// Bench instance profile for `baseline` and `bench` phases: ARN or name (`eval_identity_ref`). NO sealed access.
    pub bench_instance_profile: String,
    /// Instance profile for `confirm` runs only (`confirm_identity_ref`): the only identity that can read the sealed
    /// bucket. Required to launch a confirm instance; must differ from `bench_instance_profile`.
    pub confirm_instance_profile: Option<String>,
    /// Instance profile for `deps` jobs only (`deps_identity_ref`): writes `bench-deps/` and nothing else, no sealed
    /// access. Required to launch a dependency-snapshot job (absent in sandboxes applied before snapshots existed:
    /// re-apply). Must differ from the bench and confirm profiles.
    pub deps_instance_profile: Option<String>,
    /// `network_ref.egress_proxy_url`; when set, agent runners get HTTPS_PROXY. `None`: no proxy.
    pub egress_proxy_url: Option<String>,
    pub artifacts_bucket: String,
    /// Local self-destruct for runners/bench (`shutdown` + terminate-on-shutdown). Default 2.
    pub max_instance_lifetime_hours: Option<f64>,
}

impl AwsComputeConfig {
    /// TS `awsComputeConfigFromOutputs`.
    pub fn from_outputs(o: &SandboxOutputs, sandbox_id: &str) -> Self {
        Self {
            sandbox_id: sandbox_id.to_string(),
            region: o.region.clone(),
            agent_boundary_arn: o.agent_boundary_ref.clone(),
            agent_role_path: o.agent_role_path.clone(),
            subnet_id: o.network.subnet_id.clone(),
            bench_subnet_id: Some(o.network.bench_subnet_id.clone()),
            agent_runner_security_group_id: o.network.agent_runner_security_group_id.clone(),
            bench_security_group_id: o.network.bench_security_group_id.clone(),
            bench_instance_profile: o.eval_identity.instance_profile_arn.clone(),
            confirm_instance_profile: Some(o.confirm_identity.instance_profile_arn.clone()),
            deps_instance_profile: o
                .deps_identity
                .as_ref()
                .map(|d| d.instance_profile_arn.clone()),
            egress_proxy_url: o.network.egress_proxy_url.clone(),
            artifacts_bucket: o.artifacts_bucket.clone(),
            max_instance_lifetime_hours: Some(o.limits.max_instance_lifetime_hours),
        }
    }
}

/// The four SDK clients the compute layer uses.
#[derive(Clone)]
pub struct AwsClients {
    pub ec2: aws_sdk_ec2::Client,
    pub iam: aws_sdk_iam::Client,
    pub ssm: aws_sdk_ssm::Client,
    pub sts: aws_sdk_sts::Client,
}

impl AwsClients {
    pub fn from_sdk_config(cfg: &aws_config::SdkConfig) -> Self {
        Self {
            ec2: aws_sdk_ec2::Client::new(cfg),
            iam: aws_sdk_iam::Client::new(cfg),
            ssm: aws_sdk_ssm::Client::new(cfg),
            sts: aws_sdk_sts::Client::new(cfg),
        }
    }

    /// Clients for `region` using the default credential chain, or the named profile in `~/.aws/config` (static, SSO or
    /// assume-role) when given.
    pub async fn connect(region: &str, profile: Option<&str>) -> Self {
        Self::from_sdk_config(&load_sdk_config(region, profile).await)
    }
}

pub(crate) async fn load_sdk_config(region: &str, profile: Option<&str>) -> aws_config::SdkConfig {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()));
    if let Some(p) = profile {
        loader = loader.profile_name(p);
    }
    loader.load().await
}

#[derive(Clone)]
pub struct AwsCloudComputeOptions {
    pub clock: Arc<dyn Clock>,
    /// SSM poll interval for `run_command`. Default 2000.
    pub poll_interval_ms: u64,
    /// Retries for IAM eventual consistency on RunInstances. Default 6.
    pub iam_propagation_retries: u32,
}

impl Default for AwsCloudComputeOptions {
    fn default() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            poll_interval_ms: 2000,
            iam_propagation_retries: 6,
        }
    }
}

const TERMINAL_SSM: [&str; 8] = [
    "Success",
    "Failed",
    "Cancelled",
    "TimedOut",
    "DeliveryTimedOut",
    "ExecutionTimedOut",
    "Undeliverable",
    "Terminated",
];

static ENV_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("static regex"));
static SECRET_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]+$").expect("static regex"));
static TRANSITION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\(([0-9]{4})-([0-9]{2})-([0-9]{2}) ([0-9]{2}):([0-9]{2}):([0-9]{2}) GMT\)")
        .expect("static regex")
});

pub struct AwsCloudCompute {
    pub clients: AwsClients,
    cfg: AwsComputeConfig,
    opts: AwsCloudComputeOptions,
    account: OnceCell<(String, String)>,
}

fn build_err(e: impl std::fmt::Display) -> Error {
    Error::other(format!("could not build AWS request: {e}"))
}

impl AwsCloudCompute {
    pub fn new(
        cfg: AwsComputeConfig,
        clients: AwsClients,
        opts: AwsCloudComputeOptions,
    ) -> Result<Self> {
        if !cfg.agent_role_path.starts_with('/') || !cfg.agent_role_path.ends_with('/') {
            return Err(Error::invalid(format!(
                "agentRolePath must start and end with \"/\": {}",
                cfg.agent_role_path
            )));
        }
        if cfg
            .confirm_instance_profile
            .as_deref()
            .is_some_and(|c| !c.is_empty() && c == cfg.bench_instance_profile)
        {
            return Err(Error::invalid(
                "confirmInstanceProfile must differ from benchInstanceProfile: baseline/bench phases must not hold sealed access",
            ));
        }
        if let Some(d) = cfg
            .deps_instance_profile
            .as_deref()
            .filter(|d| !d.is_empty())
            && (d == cfg.bench_instance_profile
                || cfg.confirm_instance_profile.as_deref() == Some(d))
        {
            return Err(Error::invalid(
                "depsInstanceProfile must differ from the bench and confirm profiles: the snapshot builder has egress and writes bench-deps/, the bench runs candidate code",
            ));
        }
        Ok(Self {
            clients,
            cfg,
            opts,
            account: OnceCell::new(),
        })
    }

    pub fn config(&self) -> &AwsComputeConfig {
        &self.cfg
    }

    /// Verify credentials; optionally that they belong to the expected (dedicated) account.
    pub async fn preflight(&self, expected_account_id: Option<&str>) -> Result<CallerIdentity> {
        let r = self
            .clients
            .sts
            .get_caller_identity()
            .send()
            .await
            .map_err(sdk)?;
        let account_id = r.account().unwrap_or("").to_string();
        if let Some(expected) = expected_account_id.filter(|e| !e.is_empty())
            && account_id != expected
        {
            return Err(Error::other(format!(
                "credentials are for account {account_id}, expected sandbox account {expected}"
            )));
        }
        Ok(CallerIdentity {
            account_id,
            arn: r.arn().unwrap_or("").to_string(),
        })
    }

    /// `(account id, partition)`, looked up once.
    async fn acct(&self) -> Result<&(String, String)> {
        self.account
            .get_or_try_init(|| async {
                let r = self
                    .clients
                    .sts
                    .get_caller_identity()
                    .send()
                    .await
                    .map_err(sdk)?;
                let arn = r.arn().unwrap_or("");
                let partition = arn
                    .split(':')
                    .nth(1)
                    .filter(|p| !p.is_empty())
                    .unwrap_or("aws")
                    .to_string();
                Ok::<_, Error>((r.account().unwrap_or("").to_string(), partition))
            })
            .await
    }

    fn lifetime_hours(&self) -> f64 {
        self.cfg.max_instance_lifetime_hours.unwrap_or(2.0)
    }

    /// What the agent needs; the boundary caps it (effective = intersection).
    fn agent_identity_policy(
        &self,
        partition: &str,
        account: &str,
        opt: &str,
        agent: &str,
    ) -> Value {
        let region = &self.cfg.region;
        let bucket = &self.cfg.artifacts_bucket;
        json!({
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Sid": "Ec2",
                    "Effect": "Allow",
                    "Action": ["ec2:RunInstances", "ec2:CreateTags", "ec2:StartInstances", "ec2:StopInstances", "ec2:TerminateInstances", "ec2:Describe*"],
                    "Resource": "*",
                },
                {
                    "Sid": "ArtifactObjects",
                    "Effect": "Allow",
                    "Action": ["s3:GetObject", "s3:PutObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"],
                    "Resource": format!("arn:{partition}:s3:::{bucket}/artifacts/{opt}/{agent}/*"),
                },
                {
                    "Sid": "ArtifactList",
                    "Effect": "Allow",
                    "Action": "s3:ListBucket",
                    "Resource": format!("arn:{partition}:s3:::{bucket}"),
                    "Condition": { "StringLike": { "s3:prefix": format!("artifacts/{opt}/{agent}/*") } },
                },
                {
                    "Sid": "OwnSecrets",
                    "Effect": "Allow",
                    "Action": ["ssm:GetParameter", "ssm:GetParameters"],
                    "Resource": format!("arn:{partition}:ssm:{region}:{account}:parameter/ratchet/{opt}/{agent}/*"),
                },
                {
                    "Sid": "SsmManagedInstance",
                    "Effect": "Allow",
                    "Action": [
                        "ssm:UpdateInstanceInformation",
                        "ssmmessages:CreateControlChannel",
                        "ssmmessages:CreateDataChannel",
                        "ssmmessages:OpenControlChannel",
                        "ssmmessages:OpenDataChannel",
                        "ec2messages:AcknowledgeMessage",
                        "ec2messages:DeleteMessage",
                        "ec2messages:FailMessage",
                        "ec2messages:GetEndpoint",
                        "ec2messages:GetMessages",
                        "ec2messages:SendReply",
                    ],
                    "Resource": "*",
                },
            ],
        })
    }

    fn base_run_request(
        &self,
        a: &RunParams,
    ) -> Result<aws_sdk_ec2::operation::run_instances::builders::RunInstancesFluentBuilder> {
        let tag = |k: &str, v: &str| ec2t::Tag::builder().key(k).value(v).build();
        let instance_tags: Vec<ec2t::Tag> = a.tags.iter().map(|(k, v)| tag(k, v)).collect();
        let volume_tags: Vec<ec2t::Tag> = a
            .tags
            .iter()
            .filter(|(k, _)| k != "Name")
            .map(|(k, v)| tag(k, v))
            .collect();
        let profile = match &a.profile {
            Profile::Arn(arn) => ec2t::IamInstanceProfileSpecification::builder()
                .arn(arn)
                .build(),
            Profile::Name(name) => ec2t::IamInstanceProfileSpecification::builder()
                .name(name)
                .build(),
        };
        Ok(self
            .clients
            .ec2
            .run_instances()
            .image_id(&a.image_ref)
            .instance_type(ec2t::InstanceType::from(a.instance_type.as_str()))
            .min_count(1)
            .max_count(1)
            .subnet_id(&a.subnet_id)
            .security_group_ids(&a.security_group_id)
            .iam_instance_profile(profile)
            .metadata_options(
                ec2t::InstanceMetadataOptionsRequest::builder()
                    .http_tokens(ec2t::HttpTokensState::Required)
                    .http_endpoint(ec2t::InstanceMetadataEndpointState::Enabled)
                    .http_put_response_hop_limit(1)
                    .build(),
            )
            .instance_initiated_shutdown_behavior(ec2t::ShutdownBehavior::Terminate)
            .user_data(base64::engine::general_purpose::STANDARD.encode(a.user_data.as_bytes()))
            .tag_specifications(
                ec2t::TagSpecification::builder()
                    .resource_type(ec2t::ResourceType::Instance)
                    .set_tags(Some(instance_tags))
                    .build(),
            )
            .tag_specifications(
                ec2t::TagSpecification::builder()
                    .resource_type(ec2t::ResourceType::Volume)
                    .set_tags(Some(volume_tags))
                    .build(),
            ))
    }

    async fn run_instance(
        &self,
        params: &RunParams,
        role: InstanceRole,
        optimization_id: &str,
        agent_id: Option<&str>,
        instance_type: &str,
    ) -> Result<InstanceInfo> {
        let retries = self.opts.iam_propagation_retries;
        let mut attempt: u32 = 0;
        loop {
            match self.base_run_request(params)?.send().await {
                Ok(r) => {
                    let inst = r.instances().first();
                    let Some(id) = inst.and_then(|i| i.instance_id()) else {
                        return Err(Error::other("RunInstances returned no instance"));
                    };
                    let launched_at = inst
                        .and_then(|i| i.launch_time())
                        .and_then(|t| t.to_millis().ok())
                        .unwrap_or_else(|| self.opts.clock.now());
                    return Ok(InstanceInfo {
                        id: id.to_string(),
                        role,
                        optimization_id: optimization_id.into(),
                        agent_id: agent_id.map(AgentId::from),
                        instance_type: instance_type.to_string(),
                        state: InstanceState::Pending,
                        launched_at,
                        terminated_at: None,
                    });
                }
                Err(e) => {
                    let e = sdk(e);
                    // A just-created instance profile is not visible to EC2 for a few seconds.
                    let propagating = e.code() == "InvalidParameterValue"
                        && matches!(&e, Error::Aws { message, .. } if message.to_lowercase().contains("instance profile"));
                    if attempt < retries && propagating {
                        let _ = self
                            .opts
                            .clock
                            .sleep((2000u64 << attempt.min(20)).min(15_000), None)
                            .await;
                        attempt += 1;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }
}

struct RunParams {
    image_ref: String,
    instance_type: String,
    subnet_id: String,
    security_group_id: String,
    profile: Profile,
    user_data: String,
    tags: Vec<(String, String)>,
}

enum Profile {
    Arn(String),
    Name(String),
}

fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The typed implementations behind the [`CloudCompute`] port: ids are plain strings and failures are this
/// crate's [`Error`], whose AWS error codes drive the idempotency and retry rules.
impl AwsCloudCompute {
    // ---- identity -----------------------------------------------------------

    async fn create_agent_identity_impl(
        &self,
        optimization_id: &str,
        agent_id: &str,
    ) -> Result<AgentIdentity> {
        assert_valid_id("optimization", optimization_id)?;
        assert_valid_id("agent", agent_id)?;
        let (account_id, partition) = self.acct().await?;
        let name = agent_role_name(optimization_id, agent_id)?;
        let iam_tags = || -> Result<Vec<iamt::Tag>> {
            [
                (TAG_OPT, optimization_id),
                (TAG_AGENT, agent_id),
                (TAG_SANDBOX, self.cfg.sandbox_id.as_str()),
            ]
            .into_iter()
            .map(|(k, v)| {
                iamt::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .map_err(build_err)
            })
            .collect()
        };
        let iam = &self.clients.iam;
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "ec2.amazonaws.com" },
                "Action": "sts:AssumeRole",
                "Condition": { "StringEquals": { "aws:SourceAccount": account_id } },
            }],
        });

        let role = match iam
            .create_role()
            .role_name(&name)
            .path(&self.cfg.agent_role_path)
            .assume_role_policy_document(trust.to_string())
            .permissions_boundary(&self.cfg.agent_boundary_arn)
            .set_tags(Some(iam_tags()?))
            .max_session_duration(3600)
            .description(format!(
                "Ratchet agent {agent_id} of optimization {optimization_id}"
            ))
            .send()
            .await
        {
            Ok(r) => r.role().map(|r| r.arn().to_string()),
            Err(e) => tolerate("EntityAlreadyExists", sdk(e))?,
        };
        let role_arn = role.unwrap_or_else(|| {
            format!(
                "arn:{partition}:iam::{account_id}:role{}{name}",
                self.cfg.agent_role_path
            )
        });

        iam.put_role_policy()
            .role_name(&name)
            .policy_name(AGENT_POLICY_NAME)
            .policy_document(
                self.agent_identity_policy(partition, account_id, optimization_id, agent_id)
                    .to_string(),
            )
            .send()
            .await
            .map_err(sdk)?;

        let profile = match iam
            .create_instance_profile()
            .instance_profile_name(&name)
            .path(&self.cfg.agent_role_path)
            .set_tags(Some(iam_tags()?))
            .send()
            .await
        {
            Ok(r) => r.instance_profile().map(|p| p.arn().to_string()),
            Err(e) => tolerate("EntityAlreadyExists", sdk(e))?,
        };
        if let Err(e) = iam
            .add_role_to_instance_profile()
            .instance_profile_name(&name)
            .role_name(&name)
            .send()
            .await
        {
            tolerate::<()>("LimitExceeded", sdk(e))?;
        }
        let profile_arn = profile.unwrap_or_else(|| {
            format!(
                "arn:{partition}:iam::{account_id}:instance-profile{}{name}",
                self.cfg.agent_role_path
            )
        });

        Ok(AgentIdentity {
            optimization_id: optimization_id.into(),
            agent_id: agent_id.into(),
            role_ref: role_arn,
            instance_profile_ref: profile_arn,
        })
    }

    async fn revoke_agent_identity_impl(&self, identity: &AgentIdentity) -> Result<()> {
        let doc = json!({
            "Version": "2012-10-17",
            "Statement": [{ "Sid": "KillSwitchDenyAll", "Effect": "Deny", "Action": "*", "Resource": "*" }],
        });
        self.clients
            .iam
            .put_role_policy()
            .role_name(arn_name(&identity.role_ref))
            .policy_name(KILL_SWITCH_POLICY_NAME)
            .policy_document(doc.to_string())
            .send()
            .await
            .map_err(sdk)?;
        Ok(())
    }

    /// Order matters: the role must leave its instance profile before either can be deleted, and a role cannot be
    /// deleted while inline/managed policies remain.
    async fn delete_agent_identity_impl(&self, identity: &AgentIdentity) -> Result<()> {
        let iam = &self.clients.iam;
        let role = arn_name(&identity.role_ref);
        let profile = arn_name(&identity.instance_profile_ref);
        let ignore_missing = |r: Result<()>| -> Result<()> {
            match r {
                Err(e) if e.code_starts_with("NoSuchEntity") => Ok(()),
                other => other,
            }
        };

        ignore_missing(
            iam.remove_role_from_instance_profile()
                .instance_profile_name(profile)
                .role_name(role)
                .send()
                .await
                .map(|_| ())
                .map_err(sdk),
        )?;
        ignore_missing(
            iam.delete_instance_profile()
                .instance_profile_name(profile)
                .send()
                .await
                .map(|_| ())
                .map_err(sdk),
        )?;

        let inline = match iam.list_role_policies().role_name(role).send().await {
            Ok(r) => r.policy_names().to_vec(),
            Err(e) => {
                let e = sdk(e);
                if e.code_starts_with("NoSuchEntity") {
                    vec![]
                } else {
                    return Err(e);
                }
            }
        };
        for name in inline {
            ignore_missing(
                iam.delete_role_policy()
                    .role_name(role)
                    .policy_name(name)
                    .send()
                    .await
                    .map(|_| ())
                    .map_err(sdk),
            )?;
        }
        let attached = match iam
            .list_attached_role_policies()
            .role_name(role)
            .send()
            .await
        {
            Ok(r) => r
                .attached_policies()
                .iter()
                .filter_map(|p| p.policy_arn().map(str::to_string))
                .collect::<Vec<_>>(),
            Err(e) => {
                let e = sdk(e);
                if e.code_starts_with("NoSuchEntity") {
                    vec![]
                } else {
                    return Err(e);
                }
            }
        };
        for arn in attached {
            ignore_missing(
                iam.detach_role_policy()
                    .role_name(role)
                    .policy_arn(arn)
                    .send()
                    .await
                    .map(|_| ())
                    .map_err(sdk),
            )?;
        }
        ignore_missing(
            iam.delete_role()
                .role_name(role)
                .send()
                .await
                .map(|_| ())
                .map_err(sdk),
        )?;
        Ok(())
    }

    // ---- secrets ------------------------------------------------------------

    async fn put_secret_impl(
        &self,
        optimization_id: &str,
        agent_id: &str,
        name: &str,
        value: &str,
    ) -> Result<SecretRef> {
        assert_valid_id("optimization", optimization_id)?;
        assert_valid_id("agent", agent_id)?;
        if !SECRET_NAME_RE.is_match(name) || name.contains("..") {
            return Err(Error::invalid(format!(
                "invalid secret name {}",
                json_str(name)
            )));
        }
        let reference = format!("{}{name}", secret_prefix(optimization_id, agent_id)?);
        let ssm = &self.clients.ssm;
        // PutParameter rejects Tags together with Overwrite, so tag separately.
        ssm.put_parameter()
            .name(&reference)
            .value(value)
            .r#type(ssmt::ParameterType::SecureString)
            .overwrite(true)
            .send()
            .await
            .map_err(sdk)?;
        ssm.add_tags_to_resource()
            .resource_type(ssmt::ResourceTypeForTagging::Parameter)
            .resource_id(&reference)
            .tags(
                ssmt::Tag::builder()
                    .key(TAG_OPT)
                    .value(optimization_id)
                    .build()
                    .map_err(build_err)?,
            )
            .tags(
                ssmt::Tag::builder()
                    .key(TAG_AGENT)
                    .value(agent_id)
                    .build()
                    .map_err(build_err)?,
            )
            .send()
            .await
            .map_err(sdk)?;
        Ok(SecretRef {
            name: name.to_string(),
            reference,
        })
    }

    async fn delete_secrets_impl(&self, optimization_id: &str, agent_id: &str) -> Result<()> {
        assert_valid_id("optimization", optimization_id)?;
        assert_valid_id("agent", agent_id)?;
        let ssm = &self.clients.ssm;
        let prefix = secret_prefix(optimization_id, agent_id)?;
        let mut names: Vec<String> = vec![];
        let mut token: Option<String> = None;
        loop {
            let r = ssm
                .get_parameters_by_path()
                .path(&prefix)
                .recursive(true)
                .with_decryption(false)
                .set_next_token(token.clone())
                .send()
                .await
                .map_err(sdk)?;
            names.extend(
                r.parameters()
                    .iter()
                    .filter_map(|p| p.name().map(str::to_string)),
            );
            token = r.next_token().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        for chunk in names.chunks(10) {
            ssm.delete_parameters()
                .set_names(Some(chunk.to_vec()))
                .send()
                .await
                .map_err(sdk)?;
        }
        Ok(())
    }

    // ---- compute ------------------------------------------------------------

    async fn launch_runner_impl(&self, spec: &RunnerLaunchSpec) -> Result<InstanceInfo> {
        assert_valid_id("optimization", spec.optimization_id.as_str())?;
        assert_valid_id("agent", spec.agent_id.as_str())?;
        let net =
            RunnerNetworkOptions::new(self.cfg.region.clone(), self.cfg.egress_proxy_url.clone());
        let user_data = runner_user_data(spec, self.lifetime_hours(), Some(&net))?;
        let params = RunParams {
            image_ref: spec.image_ref.clone(),
            instance_type: spec.instance_type.clone(),
            subnet_id: self.cfg.subnet_id.clone(),
            security_group_id: self.cfg.agent_runner_security_group_id.clone(),
            profile: Profile::Arn(spec.identity.instance_profile_ref.clone()),
            user_data,
            tags: tags(&[
                (TAG_OPT, spec.optimization_id.as_str()),
                (TAG_AGENT, spec.agent_id.as_str()),
                (TAG_ROLE, "agent_runner"),
                (TAG_SANDBOX, &self.cfg.sandbox_id),
                (
                    "Name",
                    &format!("ratchet-runner-{}-{}", spec.optimization_id, spec.agent_id),
                ),
            ]),
        };
        self.run_instance(
            &params,
            InstanceRole::AgentRunner,
            spec.optimization_id.as_str(),
            Some(spec.agent_id.as_str()),
            &spec.instance_type,
        )
        .await
    }

    /// Bench instances sit in the private bench subnet. The instance profile depends on the phase: only `confirm` gets
    /// the identity that can read the sealed bucket; `baseline` and `bench` run candidate code with no sealed access at
    /// all. A missing profile fails closed.
    ///
    /// `deps` (dependency snapshot) is the one purpose that runs elsewhere: in the AGENT subnet with the agent runner
    /// security group (egress, through the allow-list proxy when it is enabled), under its own profile that can write
    /// `bench-deps/` and nothing else. It never executes candidate code, and there is no fallback from `deps` to a bench
    /// subnet or profile or the other way round.
    async fn launch_bench_instance_impl(&self, spec: &BenchLaunchSpec) -> Result<InstanceInfo> {
        assert_valid_id("optimization", spec.optimization_id.as_str())?;
        let p = bench_profile_for(&self.cfg, spec.purpose)?;
        let deps = spec.purpose == BenchLaunchPurpose::Deps;
        let bench_subnet = match (&self.cfg.bench_subnet_id, deps) {
            (_, true) => None,
            (Some(s), false) if !s.is_empty() => Some(s.clone()),
            _ => {
                return Err(Error::other(
                    "benchSubnetId is not configured (network_ref.bench_subnet_id): refusing to launch a bench instance in a subnet that may have internet access",
                ));
            }
        };
        let user_data = if deps {
            let net = RunnerNetworkOptions::new(
                self.cfg.region.clone(),
                self.cfg.egress_proxy_url.clone(),
            );
            deps_user_data(spec, self.lifetime_hours(), Some(&net))?
        } else {
            bench_user_data(spec, self.lifetime_hours())?
        };
        let params = RunParams {
            image_ref: spec.image_ref.clone(),
            instance_type: spec.instance_type.clone(),
            subnet_id: bench_subnet.unwrap_or_else(|| self.cfg.subnet_id.clone()),
            security_group_id: if deps {
                self.cfg.agent_runner_security_group_id.clone()
            } else {
                self.cfg.bench_security_group_id.clone()
            },
            profile: if p.starts_with("arn:") {
                Profile::Arn(p.to_string())
            } else {
                Profile::Name(p.to_string())
            },
            user_data,
            tags: tags(&[
                (TAG_OPT, spec.optimization_id.as_str()),
                (TAG_ROLE, "bench"),
                (TAG_PURPOSE, spec.purpose.as_str()),
                (TAG_SANDBOX, &self.cfg.sandbox_id),
                (
                    "Name",
                    &format!(
                        "ratchet-{}-{}-{}",
                        if deps { "deps" } else { "bench" },
                        spec.optimization_id,
                        spec.purpose.as_str()
                    ),
                ),
            ]),
        };
        self.run_instance(
            &params,
            InstanceRole::Bench,
            spec.optimization_id.as_str(),
            None,
            &spec.instance_type,
        )
        .await
    }

    async fn terminate_impl(&self, instance_ids: &[String]) -> Result<()> {
        if instance_ids.is_empty() {
            return Ok(());
        }
        for id in instance_ids {
            assert_valid_instance_id(id)?;
        }
        match self
            .clients
            .ec2
            .terminate_instances()
            .set_instance_ids(Some(instance_ids.to_vec()))
            .send()
            .await
        {
            Ok(_) => Ok(()),
            // already gone: idempotent
            Err(e) => {
                let e = sdk(e);
                if e.code() == "InvalidInstanceID.NotFound" {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn list_tagged_impl(&self, optimization_id: Option<&str>) -> Result<Vec<InstanceInfo>> {
        // `*` and `?` are wildcards in EC2 filter values, so an unvalidated id could list other optimizations.
        if let Some(id) = optimization_id {
            assert_valid_id("optimization", id)?;
        }
        let filter = match optimization_id {
            Some(id) => ec2t::Filter::builder()
                .name(format!("tag:{TAG_OPT}"))
                .values(id)
                .build(),
            None => ec2t::Filter::builder()
                .name("tag-key")
                .values(TAG_OPT)
                .build(),
        };
        let mut out = vec![];
        let mut token: Option<String> = None;
        loop {
            let r = self
                .clients
                .ec2
                .describe_instances()
                .filters(filter.clone())
                .set_next_token(token.clone())
                .send()
                .await
                .map_err(sdk)?;
            for res in r.reservations() {
                for i in res.instances() {
                    if let Some(info) = to_instance_info(i) {
                        out.push(info);
                    }
                }
            }
            token = r.next_token().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    // ---- remote commands ----------------------------------------------------

    async fn run_command_impl(
        &self,
        instance_id: &str,
        command: &RemoteCommand,
    ) -> Result<RemoteCommandResult> {
        assert_valid_instance_id(instance_id)?;
        let timeout = command.timeout_seconds.unwrap_or(600);
        let mut lines: Vec<String> = vec![];
        for (k, v) in command.env.iter().flatten() {
            if !ENV_KEY_RE.is_match(k) {
                return Err(Error::invalid(format!(
                    "invalid env var name {}",
                    json_str(k)
                )));
            }
            lines.push(format!("export {k}={}", sh_quote(v)));
        }
        lines.push(command.script.clone());
        let script = lines.join("\n");
        let ssm = &self.clients.ssm;
        let sent = ssm
            .send_command()
            .instance_ids(instance_id)
            .document_name("AWS-RunShellScript")
            .parameters("commands", vec![script])
            .parameters("executionTimeout", vec![timeout.to_string()])
            .timeout_seconds(timeout.max(30) as i32)
            .comment("ratchet")
            .send()
            .await
            .map_err(sdk)?;
        let Some(command_id) = sent
            .command()
            .and_then(|c| c.command_id())
            .map(str::to_string)
        else {
            return Err(Error::other("SendCommand returned no CommandId"));
        };

        let clock = &self.opts.clock;
        let deadline = clock.now() + (timeout as i64 + 60) * 1000;
        loop {
            let _ = clock.sleep(self.opts.poll_interval_ms, None).await;
            let inv = match ssm
                .get_command_invocation()
                .command_id(&command_id)
                .instance_id(instance_id)
                .send()
                .await
            {
                Ok(inv) => inv,
                Err(e) => {
                    let e = sdk(e);
                    if e.code() != "InvocationDoesNotExist" {
                        return Err(e); // not registered yet is the only error worth waiting out
                    }
                    if clock.now() > deadline {
                        return Err(Error::other(format!(
                            "SSM command {command_id} never registered on {instance_id}"
                        )));
                    }
                    continue;
                }
            };
            let status = inv.status().map_or("Pending", |s| s.as_str()).to_string();
            if TERMINAL_SSM.contains(&status.as_str()) {
                let timed_out = status.contains("TimedOut");
                let success = status == "Success";
                // The SDK reports an absent ResponseCode as 0; the TS code treated absent as 1 for a failed command.
                let code = inv.response_code();
                let exit_code = if timed_out {
                    124
                } else if code == 0 && !success {
                    1
                } else {
                    code
                };
                let stderr = inv.standard_error_content().unwrap_or("");
                return Ok(RemoteCommandResult {
                    exit_code,
                    stdout: inv.standard_output_content().unwrap_or("").to_string(),
                    stderr: format!(
                        "{stderr}{}",
                        if success {
                            String::new()
                        } else {
                            format!(
                                "{}[ssm status: {status}]",
                                if stderr.is_empty() { "" } else { "\n" }
                            )
                        }
                    ),
                });
            }
            if clock.now() > deadline {
                return Err(Error::other(format!(
                    "SSM command {command_id} on {instance_id} still {status} after {}s",
                    timeout + 60
                )));
            }
        }
    }
}

#[async_trait]
impl CloudCompute for AwsCloudCompute {
    async fn create_agent_identity(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<AgentIdentity> {
        Ok(self
            .create_agent_identity_impl(optimization_id.as_str(), agent_id.as_str())
            .await?)
    }

    async fn revoke_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        Ok(self.revoke_agent_identity_impl(identity).await?)
    }

    async fn delete_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        Ok(self.delete_agent_identity_impl(identity).await?)
    }

    async fn put_secret(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
        name: &str,
        value: &str,
    ) -> PortResult<SecretRef> {
        Ok(self
            .put_secret_impl(optimization_id.as_str(), agent_id.as_str(), name, value)
            .await?)
    }

    async fn delete_secrets(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<()> {
        Ok(self
            .delete_secrets_impl(optimization_id.as_str(), agent_id.as_str())
            .await?)
    }

    async fn launch_runner(&self, spec: &RunnerLaunchSpec) -> PortResult<InstanceInfo> {
        Ok(self.launch_runner_impl(spec).await?)
    }

    async fn launch_bench_instance(&self, spec: &BenchLaunchSpec) -> PortResult<InstanceInfo> {
        Ok(self.launch_bench_instance_impl(spec).await?)
    }

    async fn terminate(&self, instance_ids: &[String]) -> PortResult<()> {
        Ok(self.terminate_impl(instance_ids).await?)
    }

    async fn list_tagged(
        &self,
        optimization_id: Option<&OptimizationId>,
    ) -> PortResult<Vec<InstanceInfo>> {
        Ok(self
            .list_tagged_impl(optimization_id.map(OptimizationId::as_str))
            .await?)
    }

    async fn run_command(
        &self,
        instance_id: &str,
        command: &RemoteCommand,
    ) -> PortResult<RemoteCommandResult> {
        Ok(self.run_command_impl(instance_id, command).await?)
    }
}

// ---- helpers ----------------------------------------------------------------

/// `Ok(None)` when `e` is the tolerated error `code` (prefix match), else the error.
fn tolerate<T>(code: &str, e: Error) -> Result<Option<T>> {
    if e.code_starts_with(code) {
        Ok(None)
    } else {
        Err(e)
    }
}

pub fn agent_role_name(opt: &str, agent: &str) -> Result<String> {
    assert_valid_id("optimization", opt)?;
    assert_valid_id("agent", agent)?;
    Ok(bounded_name(&format!("ratchet-agent-{opt}-{agent}"), 64))
}

pub fn secret_prefix(opt: &str, agent: &str) -> Result<String> {
    assert_valid_id("optimization", opt)?;
    assert_valid_id("agent", agent)?;
    Ok(format!("/ratchet/{opt}/{agent}/"))
}

/// Picks the instance profile for a bench phase; the sealed-capable one only for `confirm`, the `bench-deps/` writer
/// only for `deps`. Anything not configured fails closed.
pub fn bench_profile_for(cfg: &AwsComputeConfig, purpose: BenchLaunchPurpose) -> Result<&str> {
    match purpose {
        BenchLaunchPurpose::Baseline | BenchLaunchPurpose::Bench => Ok(&cfg.bench_instance_profile),
        BenchLaunchPurpose::Confirm => cfg.confirm_instance_profile.as_deref().filter(|p| !p.is_empty()).ok_or_else(|| {
            Error::other("confirmInstanceProfile is not configured (confirm_identity_ref): refusing to run a confirm job under the bench identity")
        }),
        BenchLaunchPurpose::Deps => cfg.deps_instance_profile.as_deref().filter(|p| !p.is_empty()).ok_or_else(|| {
            Error::other("depsInstanceProfile is not configured (deps_identity_ref): re-apply the sandbox to create the dependency-snapshot identity; refusing to build a snapshot under another identity")
        }),
    }
}

fn instance_state(name: &str) -> InstanceState {
    match name {
        "running" => InstanceState::Running,
        "shutting-down" | "stopping" => InstanceState::Stopping,
        "stopped" => InstanceState::Stopped,
        "terminated" => InstanceState::Terminated,
        _ => InstanceState::Pending,
    }
}

/// An EC2 instance as an [`InstanceInfo`]; `None` for instances without a `ratchet:opt` tag.
pub fn to_instance_info(i: &ec2t::Instance) -> Option<InstanceInfo> {
    let tag = |key: &str| {
        i.tags()
            .iter()
            .find(|t| t.key() == Some(key))
            .and_then(|t| t.value())
            .map(str::to_string)
    };
    let opt = tag(TAG_OPT)?;
    let id = i.instance_id()?;
    let agent = tag(TAG_AGENT);
    // Agent-launched experiments carry only opt+agent tags (the boundary forces just those).
    let role = match tag(TAG_ROLE).as_deref() {
        Some("agent_runner") => InstanceRole::AgentRunner,
        Some("bench") => InstanceRole::Bench,
        Some("experiment") => InstanceRole::Experiment,
        _ if agent.is_some() => InstanceRole::Experiment,
        _ => InstanceRole::Bench,
    };
    let state = i
        .state()
        .and_then(|s| s.name())
        .map_or(InstanceState::Pending, |n| instance_state(n.as_str()));
    let mut info = InstanceInfo {
        id: id.to_string(),
        role,
        optimization_id: opt.into(),
        agent_id: agent.filter(|a| !a.is_empty()).map(AgentId::from),
        instance_type: i
            .instance_type()
            .map_or("unknown", |t| t.as_str())
            .to_string(),
        state,
        launched_at: i
            .launch_time()
            .and_then(|t| t.to_millis().ok())
            .unwrap_or(0),
        terminated_at: None,
    };
    if matches!(state, InstanceState::Terminated | InstanceState::Stopped) {
        // StateTransitionReason: "User initiated (2026-01-02 03:04:05 GMT)"
        if let Some(c) = TRANSITION_RE.captures(i.state_transition_reason().unwrap_or("")) {
            let n = |k: usize| c[k].parse::<i64>().unwrap_or(0);
            info.terminated_at = Some(civil::epoch_ms(n(1), n(2), n(3), n(4), n(5), n(6)));
        }
    }
    Some(info)
}

/// Runtime agent roles still present under the agent path (teardown guard for `TofuSandboxProvider`).
pub async fn list_agent_roles(
    iam: &aws_sdk_iam::Client,
    agent_role_path: &str,
) -> Result<Vec<String>> {
    let mut names = vec![];
    let mut marker: Option<String> = None;
    loop {
        let r = iam
            .list_roles()
            .path_prefix(agent_role_path)
            .set_marker(marker.clone())
            .send()
            .await
            .map_err(sdk)?;
        names.extend(r.roles().iter().map(|role| role.role_name().to_string()));
        marker = if r.is_truncated() {
            r.marker().map(str::to_string)
        } else {
            None
        };
        if marker.is_none() {
            break;
        }
    }
    Ok(names)
}

/// The default role path, `/ratchet/agent/`.
pub fn default_agent_role_path() -> &'static str {
    DEFAULT_AGENT_ROLE_PATH
}
