//! In-memory `SandboxProvider` for simulation; behaves like `TofuSandboxProvider` for the engine. (TS `sandbox/mock.ts`)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::tofu::{RuntimeRoleLister, TeardownBlockedError};
use crate::domain::{
    AwsIdentityRef, PlanAction, PlanChange, SandboxInput, SandboxLimits, SandboxNetwork,
    SandboxOutputs, SandboxPlan,
};
use crate::error::Result;
use crate::ports::SandboxProvider;
use crate::util::assert_valid_sandbox_id;

/// The headline resources of infra/aws (docs/V0_ARCHITECTURE.md §7), with real addresses.
pub const SANDBOX_RESOURCES: &[(&str, &str)] = &[
    ("aws_s3_bucket.artifacts", "aws_s3_bucket"),
    ("aws_s3_bucket.sealed", "aws_s3_bucket"),
    (
        "aws_s3_bucket_policy.sealed_deny_agents",
        "aws_s3_bucket_policy",
    ),
    ("aws_iam_policy.agent_boundary", "aws_iam_policy"),
    ("aws_iam_role.bench_runner", "aws_iam_role"),
    (
        "aws_iam_instance_profile.bench_runner",
        "aws_iam_instance_profile",
    ),
    ("aws_iam_role.confirm_runner", "aws_iam_role"),
    (
        "aws_iam_instance_profile.confirm_runner",
        "aws_iam_instance_profile",
    ),
    ("aws_iam_role.deps_runner", "aws_iam_role"),
    (
        "aws_iam_instance_profile.deps_runner",
        "aws_iam_instance_profile",
    ),
    ("aws_vpc.sandbox", "aws_vpc"),
    ("aws_subnet.bench", "aws_subnet"),
    ("aws_subnet.agent", "aws_subnet"),
    ("aws_vpc_endpoint.s3", "aws_vpc_endpoint"),
    ("aws_vpc_endpoint.interface[\"ssm\"]", "aws_vpc_endpoint"),
    (
        "aws_vpc_endpoint.interface[\"ssmmessages\"]",
        "aws_vpc_endpoint",
    ),
    (
        "aws_vpc_endpoint.interface[\"ec2messages\"]",
        "aws_vpc_endpoint",
    ),
    ("aws_vpc_endpoint.interface[\"logs\"]", "aws_vpc_endpoint"),
    ("aws_security_group.agent_runner", "aws_security_group"),
    ("aws_security_group.bench", "aws_security_group"),
    ("aws_security_group.endpoints", "aws_security_group"),
];

#[derive(Default)]
pub struct MockSandboxProviderOptions {
    pub account_id: Option<String>,
    /// Called on destroy, mirroring `TofuSandboxProvider`'s teardown guard.
    pub role_lister: Option<Arc<dyn RuntimeRoleLister>>,
    /// Sandboxes that are already applied (e.g. restored from the ledger after an app restart), by sandbox id.
    pub initial: HashMap<String, SandboxOutputs>,
}

pub struct MockSandboxProvider {
    opts: MockSandboxProviderOptions,
    applied: Mutex<HashMap<String, SandboxOutputs>>,
    calls: Mutex<Vec<String>>,
}

impl Default for MockSandboxProvider {
    fn default() -> Self {
        Self::new(MockSandboxProviderOptions::default())
    }
}

impl MockSandboxProvider {
    pub fn new(mut opts: MockSandboxProviderOptions) -> Self {
        let applied = std::mem::take(&mut opts.initial);
        Self {
            opts,
            applied: Mutex::new(applied),
            calls: Mutex::default(),
        }
    }

    /// `plan:<id>`, `apply:<id>`, `destroy:<id>` in call order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls poisoned").clone()
    }

    fn record(&self, op: &str, id: &str) {
        self.calls
            .lock()
            .expect("calls poisoned")
            .push(format!("{op}:{id}"));
    }
}

fn identity(acct: &str, name: &str) -> AwsIdentityRef {
    AwsIdentityRef {
        role_arn: format!("arn:aws:iam::{acct}:role/ratchet/bench/{name}"),
        role_name: name.to_string(),
        instance_profile_arn: format!("arn:aws:iam::{acct}:instance-profile/ratchet/bench/{name}"),
        instance_profile_name: name.to_string(),
    }
}

#[async_trait]
impl SandboxProvider for MockSandboxProvider {
    async fn plan(&self, input: &SandboxInput) -> Result<SandboxPlan> {
        self.record("plan", &input.sandbox_id);
        assert_valid_sandbox_id(&input.sandbox_id)?;
        let exists = self
            .applied
            .lock()
            .expect("applied poisoned")
            .contains_key(&input.sandbox_id);
        let action = if exists {
            PlanAction::NoOp
        } else {
            PlanAction::Create
        };
        let changes: Vec<PlanChange> = SANDBOX_RESOURCES
            .iter()
            .map(|(address, ty)| PlanChange {
                action,
                address: (*address).to_string(),
                resource_type: (*ty).to_string(),
            })
            .collect();
        let n = if exists { 0 } else { changes.len() };
        Ok(SandboxPlan {
            changes,
            summary: format!("Plan: {n} to add, 0 to change, 0 to destroy."),
        })
    }

    async fn apply(&self, input: &SandboxInput) -> Result<SandboxOutputs> {
        self.record("apply", &input.sandbox_id);
        assert_valid_sandbox_id(&input.sandbox_id)?;
        if let Some(existing) = self
            .applied
            .lock()
            .expect("applied poisoned")
            .get(&input.sandbox_id)
        {
            return Ok(existing.clone());
        }
        let acct = self.opts.account_id.as_deref().unwrap_or("000000000000");
        let p = format!("ratchet-{}", input.sandbox_id);
        let out = SandboxOutputs {
            artifacts_bucket: format!("{p}-artifacts"),
            sealed_bucket: format!("{p}-sealed"),
            agent_boundary_ref: format!("arn:aws:iam::{acct}:policy/ratchet/{p}-agent-boundary"),
            eval_identity_ref: format!(
                "arn:aws:iam::{acct}:instance-profile/ratchet/bench/{p}-bench-runner"
            ),
            agent_role_path: "/ratchet/agent/".to_string(),
            network: SandboxNetwork {
                subnet_id: "subnet-0mockagent".to_string(),
                bench_subnet_id: "subnet-0mockbench".to_string(),
                agent_runner_security_group_id: "sg-0mockrunner".to_string(),
                bench_security_group_id: "sg-0mockbench".to_string(),
                egress_proxy_url: None,
            },
            region: input.region.clone(),
            vpc_id: "vpc-0mock0000".to_string(),
            eval_identity: identity(acct, &format!("{p}-bench-runner")),
            confirm_identity: identity(acct, &format!("{p}-confirm-runner")),
            deps_identity: Some(identity(acct, &format!("{p}-deps-runner"))),
            sealed_confirm_prefix: "holdout/".to_string(),
            limits: SandboxLimits {
                max_instances_per_agent: input.max_instances_per_agent,
                max_instance_lifetime_hours: input.max_instance_lifetime_hours,
            },
        };
        self.applied
            .lock()
            .expect("applied poisoned")
            .insert(input.sandbox_id.clone(), out.clone());
        Ok(out)
    }

    async fn destroy(&self, input: &SandboxInput) -> Result<()> {
        self.record("destroy", &input.sandbox_id);
        let roles = match &self.opts.role_lister {
            Some(l) => l.list_runtime_agent_roles(input).await?,
            None => vec![],
        };
        if !roles.is_empty() {
            return Err(TeardownBlockedError { roles }.into());
        }
        self.applied
            .lock()
            .expect("applied poisoned")
            .remove(&input.sandbox_id);
        Ok(())
    }

    async fn outputs(&self, sandbox_id: &str) -> Result<Option<SandboxOutputs>> {
        Ok(self
            .applied
            .lock()
            .expect("applied poisoned")
            .get(sandbox_id)
            .cloned())
    }
}
