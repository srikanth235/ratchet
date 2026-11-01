//! Port of `packages/aws/src/compute/aws.test.ts` (everything except the user-data tests, which are in
//! `user_data.rs`). The AWS SDK clients are mocked with `aws-smithy-mocks`; real AWS is never called.

use std::sync::Arc;

use crate::common::{Log, meta, rec, rec_op};
use aws_sdk_ec2::operation::describe_instances::{DescribeInstancesInput, DescribeInstancesOutput};
use aws_sdk_ec2::operation::run_instances::{
    RunInstancesError, RunInstancesInput, RunInstancesOutput,
};
use aws_sdk_ec2::operation::terminate_instances::{
    TerminateInstancesError, TerminateInstancesInput, TerminateInstancesOutput,
};
use aws_sdk_ec2::types as ec2t;
use aws_sdk_iam::operation::add_role_to_instance_profile::{
    AddRoleToInstanceProfileError, AddRoleToInstanceProfileInput, AddRoleToInstanceProfileOutput,
};
use aws_sdk_iam::operation::create_instance_profile::{
    CreateInstanceProfileError, CreateInstanceProfileInput, CreateInstanceProfileOutput,
};
use aws_sdk_iam::operation::create_role::{CreateRoleError, CreateRoleInput, CreateRoleOutput};
use aws_sdk_iam::operation::delete_instance_profile::{
    DeleteInstanceProfileError, DeleteInstanceProfileInput, DeleteInstanceProfileOutput,
};
use aws_sdk_iam::operation::delete_role::{DeleteRoleError, DeleteRoleInput, DeleteRoleOutput};
use aws_sdk_iam::operation::delete_role_policy::{DeleteRolePolicyInput, DeleteRolePolicyOutput};
use aws_sdk_iam::operation::detach_role_policy::{DetachRolePolicyInput, DetachRolePolicyOutput};
use aws_sdk_iam::operation::list_attached_role_policies::{
    ListAttachedRolePoliciesError, ListAttachedRolePoliciesInput, ListAttachedRolePoliciesOutput,
};
use aws_sdk_iam::operation::list_role_policies::{
    ListRolePoliciesError, ListRolePoliciesInput, ListRolePoliciesOutput,
};
use aws_sdk_iam::operation::list_roles::{ListRolesInput, ListRolesOutput};
use aws_sdk_iam::operation::put_role_policy::{PutRolePolicyInput, PutRolePolicyOutput};
use aws_sdk_iam::operation::remove_role_from_instance_profile::{
    RemoveRoleFromInstanceProfileError, RemoveRoleFromInstanceProfileInput,
    RemoveRoleFromInstanceProfileOutput,
};
use aws_sdk_iam::types as iamt;
use aws_sdk_ssm::operation::add_tags_to_resource::{
    AddTagsToResourceInput, AddTagsToResourceOutput,
};
use aws_sdk_ssm::operation::delete_parameters::{DeleteParametersInput, DeleteParametersOutput};
use aws_sdk_ssm::operation::get_command_invocation::{
    GetCommandInvocationError, GetCommandInvocationInput, GetCommandInvocationOutput,
};
use aws_sdk_ssm::operation::get_parameters_by_path::{
    GetParametersByPathInput, GetParametersByPathOutput,
};
use aws_sdk_ssm::operation::put_parameter::{PutParameterInput, PutParameterOutput};
use aws_sdk_ssm::operation::send_command::{SendCommandInput, SendCommandOutput};
use aws_sdk_ssm::types as ssmt;
use aws_sdk_sts::operation::get_caller_identity::GetCallerIdentityOutput;
use aws_smithy_mocks::{Rule, RuleMode, mock, mock_client};
use aws_smithy_types::DateTime;
use base64::Engine as _;
use ratchet_core::enums::{AgentKind, InstanceRole};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::SandboxInput;
use ratchet_core::ports::mock::ManualClock;
use ratchet_core::ports::{
    AgentIdentity, BenchLaunchPurpose, BenchLaunchSpec, Clock, InstanceState, PortError,
    PortResult, RemoteCommand, RunnerLaunchSpec, SecretRef,
};
use ratchet_core::ports::{CloudCompute, SandboxProvider};
use ratchet_sandbox::compute::aws::{
    AGENT_POLICY_NAME, AwsClients, AwsCloudCompute, AwsCloudComputeOptions, AwsComputeConfig,
    KILL_SWITCH_POLICY_NAME, agent_role_name, bench_profile_for, list_agent_roles, secret_prefix,
    to_instance_info,
};
use ratchet_sandbox::sandbox::MockSandboxProvider;

const NO_RULES: &[Rule] = &[];

fn cfg() -> AwsComputeConfig {
    AwsComputeConfig {
        sandbox_id: "demo".into(),
        region: "us-east-1".into(),
        agent_boundary_arn: "arn:aws:iam::123456789012:policy/ratchet/ratchet-demo-agent-boundary"
            .into(),
        agent_role_path: "/ratchet/agent/".into(),
        subnet_id: "subnet-agent".into(),
        bench_subnet_id: Some("subnet-bench".into()),
        agent_runner_security_group_id: "sg-runner".into(),
        bench_security_group_id: "sg-bench".into(),
        bench_instance_profile:
            "arn:aws:iam::123456789012:instance-profile/ratchet/bench/ratchet-demo-bench-runner"
                .into(),
        confirm_instance_profile: Some(
            "arn:aws:iam::123456789012:instance-profile/ratchet/bench/ratchet-demo-confirm-runner"
                .into(),
        ),
        deps_instance_profile: Some(
            "arn:aws:iam::123456789012:instance-profile/ratchet/bench/ratchet-demo-deps-runner"
                .into(),
        ),
        egress_proxy_url: None,
        artifacts_bucket: "ratchet-demo-artifacts".into(),
        max_instance_lifetime_hours: Some(2.0),
    }
}

fn sts_client() -> aws_sdk_sts::Client {
    let rule = mock!(aws_sdk_sts::Client::get_caller_identity).then_output(|| {
        GetCallerIdentityOutput::builder()
            .account("123456789012")
            .arn("arn:aws:sts::123456789012:assumed-role/x/y")
            .build()
    });
    mock_client!(aws_sdk_sts, RuleMode::MatchAny, &[&rule])
}

struct Rig {
    compute: AwsCloudCompute,
    clock: Arc<ManualClock>,
}

fn rig_with(cfg: AwsComputeConfig, ec2: &[Rule], iam: &[Rule], ssm: &[Rule]) -> Rig {
    let clock = Arc::new(ManualClock::new(1_000_000));
    let clients = AwsClients {
        ec2: mock_client!(aws_sdk_ec2, RuleMode::MatchAny, ec2),
        iam: mock_client!(aws_sdk_iam, RuleMode::MatchAny, iam),
        ssm: mock_client!(aws_sdk_ssm, RuleMode::MatchAny, ssm),
        sts: sts_client(),
    };
    let opts = AwsCloudComputeOptions {
        clock: clock.clone(),
        poll_interval_ms: 1000,
        iam_propagation_retries: 6,
    };
    Rig {
        compute: AwsCloudCompute::new(cfg, clients, opts).expect("config is valid"),
        clock,
    }
}

fn rig(ec2: &[Rule], iam: &[Rule], ssm: &[Rule]) -> Rig {
    rig_with(cfg(), ec2, iam, ssm)
}

fn try_new(cfg: AwsComputeConfig) -> Result<AwsCloudCompute, ratchet_sandbox::Error> {
    let clients = AwsClients {
        ec2: mock_client!(aws_sdk_ec2, RuleMode::MatchAny, NO_RULES),
        iam: mock_client!(aws_sdk_iam, RuleMode::MatchAny, NO_RULES),
        ssm: mock_client!(aws_sdk_ssm, RuleMode::MatchAny, NO_RULES),
        sts: sts_client(),
    };
    AwsCloudCompute::new(cfg, clients, AwsCloudComputeOptions::default())
}

fn identity() -> AgentIdentity {
    AgentIdentity {
        optimization_id: "opt1".into(),
        agent_id: "a1".into(),
        role_ref: "arn:aws:iam::123456789012:role/ratchet/agent/ratchet-agent-opt1-a1".into(),
        instance_profile_ref:
            "arn:aws:iam::123456789012:instance-profile/ratchet/agent/ratchet-agent-opt1-a1".into(),
    }
}

fn runner_spec() -> RunnerLaunchSpec {
    RunnerLaunchSpec {
        optimization_id: "opt1".into(),
        agent_id: "a1".into(),
        kind: AgentKind::ClaudeCode,
        instance_type: "c7i.large".into(),
        image_ref: "ami-runner".into(),
        identity: identity(),
        secrets: vec![SecretRef {
            name: "ANTHROPIC_API_KEY".into(),
            reference: "/ratchet/opt1/a1/ANTHROPIC_API_KEY".into(),
        }],
        repo_url: "https://github.com/o/r'; rm -rf / #.git".into(),
        branch: "ratchet/opt1/a1".into(),
    }
}

fn bench_spec(purpose: BenchLaunchPurpose) -> BenchLaunchSpec {
    BenchLaunchSpec {
        optimization_id: "opt1".into(),
        instance_type: "c7i.2xlarge".into(),
        image_ref: "ami-bench".into(),
        purpose,
    }
}

// ---- SDK object builders -----------------------------------------------------------------------------

fn iam_role(arn: &str) -> iamt::Role {
    iamt::Role::builder()
        .path("/ratchet/agent/")
        .role_name("n")
        .role_id("AROA")
        .arn(arn)
        .create_date(DateTime::from_secs(0))
        .build()
        .unwrap()
}

fn iam_profile(arn: &str) -> iamt::InstanceProfile {
    iamt::InstanceProfile::builder()
        .path("/ratchet/agent/")
        .instance_profile_name("n")
        .instance_profile_id("AIPA")
        .arn(arn)
        .create_date(DateTime::from_secs(0))
        .set_roles(Some(vec![]))
        .build()
        .unwrap()
}

fn running(id: &str) -> RunInstancesOutput {
    RunInstancesOutput::builder()
        .instances(ec2t::Instance::builder().instance_id(id).build())
        .build()
}

fn decode_user_data(input: &RunInstancesInput) -> String {
    String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(input.user_data().expect("user data"))
            .unwrap(),
    )
    .unwrap()
}

fn tags_of(input: &RunInstancesInput, kind: ec2t::ResourceType) -> Vec<(String, String)> {
    let spec = input
        .tag_specifications()
        .iter()
        .find(|t| t.resource_type() == Some(&kind))
        .expect("tag spec");
    spec.tags()
        .iter()
        .map(|t| (t.key().unwrap().to_string(), t.value().unwrap().to_string()))
        .collect()
}

fn has_tag(tags: &[(String, String)], k: &str, v: &str) -> bool {
    tags.iter().any(|(tk, tv)| tk == k && tv == v)
}

/// Rule that records RunInstances inputs and answers with one instance.
fn run_instances_ok(log: &Log<RunInstancesInput>, id: &'static str) -> Rule {
    mock!(aws_sdk_ec2::Client::run_instances)
        .match_requests(rec(log))
        .then_output(move || running(id))
}

// Ids are substituted into IAM role names, tag values, SSM paths, S3 prefixes and policy variables
// (`${aws:PrincipalTag/ratchet:opt}` inside Resource ARNs), where `*`, `?`, `/` and `..` would widen a pattern.
const INJECTION: &[&str] = &[
    "*",
    "?",
    "/",
    "..",
    "a*",
    "a?b",
    "a/b",
    "../x",
    "opt1/../opt2",
    "opt1/*",
    "Opt",
    "a b",
    "",
    "a_b",
    "a:b",
    "a${b}",
];

// ---- id validation (security requirement) -------------------------------------------------------------

#[tokio::test]
async fn rejects_injection_at_every_id_entry_point() {
    let long = "x".repeat(65);
    let mut cases: Vec<&str> = INJECTION.to_vec();
    cases.push(&long);
    // No rules on any client: a single AWS call would panic the mock.
    let r = rig(NO_RULES, NO_RULES, NO_RULES);
    let c = &r.compute;
    for bad in cases {
        let ctx = |what: &str| format!("{what} with id {bad:?}");
        let must = |res: PortResult<()>, needle: &str, what: &str| {
            let e = res.expect_err(&ctx(what));
            assert!(e.to_string().contains(needle), "{}: {e}", ctx(what));
        };
        must(
            c.create_agent_identity(&OptimizationId::from(bad), &AgentId::from("a1"))
                .await
                .map(|_| ()),
            "invalid optimization id",
            "createAgentIdentity opt",
        );
        must(
            c.create_agent_identity(&OptimizationId::from("opt1"), &AgentId::from(bad))
                .await
                .map(|_| ()),
            "invalid agent id",
            "createAgentIdentity agent",
        );
        must(
            c.put_secret(&OptimizationId::from(bad), &AgentId::from("a1"), "K", "v")
                .await
                .map(|_| ()),
            "invalid optimization id",
            "putSecret opt",
        );
        must(
            c.put_secret(&OptimizationId::from("opt1"), &AgentId::from(bad), "K", "v")
                .await
                .map(|_| ()),
            "invalid agent id",
            "putSecret agent",
        );
        must(
            c.delete_secrets(&OptimizationId::from(bad), &AgentId::from("a1"))
                .await,
            "invalid optimization id",
            "deleteSecrets opt",
        );
        must(
            c.delete_secrets(&OptimizationId::from("opt1"), &AgentId::from(bad))
                .await,
            "invalid agent id",
            "deleteSecrets agent",
        );
        let mut spec = runner_spec();
        spec.optimization_id = bad.into();
        must(
            c.launch_runner(&spec).await.map(|_| ()),
            "invalid optimization id",
            "launchRunner opt",
        );
        let mut spec = runner_spec();
        spec.agent_id = bad.into();
        must(
            c.launch_runner(&spec).await.map(|_| ()),
            "invalid agent id",
            "launchRunner agent",
        );
        for purpose in [
            BenchLaunchPurpose::Bench,
            BenchLaunchPurpose::Confirm,
            BenchLaunchPurpose::Deps,
        ] {
            let mut spec = bench_spec(purpose);
            spec.optimization_id = bad.into();
            must(
                c.launch_bench_instance(&spec).await.map(|_| ()),
                "invalid optimization id",
                "launchBenchInstance",
            );
        }
        must(
            c.list_tagged(Some(&OptimizationId::from(bad)))
                .await
                .map(|_| ()),
            "invalid optimization id",
            "listTagged",
        );
        must(
            agent_role_name(bad, "a1")
                .map(|_| ())
                .map_err(PortError::from),
            "invalid optimization id",
            "agentRoleName opt",
        );
        must(
            agent_role_name("opt1", bad)
                .map(|_| ())
                .map_err(PortError::from),
            "invalid agent id",
            "agentRoleName agent",
        );
        must(
            secret_prefix(bad, "a1")
                .map(|_| ())
                .map_err(PortError::from),
            "invalid optimization id",
            "secretPrefix opt",
        );
        must(
            secret_prefix("opt1", bad)
                .map(|_| ())
                .map_err(PortError::from),
            "invalid agent id",
            "secretPrefix agent",
        );
    }
}

#[tokio::test]
async fn rejects_bad_instance_ids_in_terminate_and_run_command() {
    let r = rig(NO_RULES, NO_RULES, NO_RULES);
    for bad in ["*", "?", "i-*", "i-1/../i-2", "../x", "", "i-", "I-ABC"] {
        let e = r.compute.terminate(&[bad.to_string()]).await.unwrap_err();
        assert!(
            e.to_string().contains("invalid instance id"),
            "{bad:?}: {e}"
        );
        let e = r
            .compute
            .terminate(&["i-0ok".to_string(), bad.to_string()])
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("invalid instance id"),
            "{bad:?}: {e}"
        );
        let e = r
            .compute
            .run_command(bad, &RemoteCommand::new("true"))
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("invalid instance id"),
            "{bad:?}: {e}"
        );
    }
}

#[tokio::test]
async fn rejects_bad_secret_names_including_dotdot_slash_and_wildcards() {
    let r = rig(NO_RULES, NO_RULES, NO_RULES);
    for bad in ["../../x", "..", "a/b", "a*", "a?", "", "a b"] {
        let e = r
            .compute
            .put_secret(
                &OptimizationId::from("opt1"),
                &AgentId::from("a1"),
                bad,
                "v",
            )
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("invalid secret name"),
            "{bad:?}: {e}"
        );
    }
}

#[tokio::test]
async fn accepts_well_formed_ids() {
    let put = mock!(aws_sdk_ssm::Client::put_parameter)
        .then_output(|| PutParameterOutput::builder().build());
    let tag = mock!(aws_sdk_ssm::Client::add_tags_to_resource)
        .then_output(|| AddTagsToResourceOutput::builder().build());
    let r = rig(NO_RULES, NO_RULES, &[put, tag]);
    r.compute
        .put_secret(
            &OptimizationId::from("opt-1"),
            &AgentId::from("claude-code-2"),
            "ANTHROPIC_API_KEY",
            "v",
        )
        .await
        .unwrap();
}

#[test]
fn shortens_very_long_role_names_but_keeps_them_unique_and_at_most_64_chars() {
    let a = agent_role_name(&"o".repeat(60), &"a".repeat(60)).unwrap();
    let b = agent_role_name(&"o".repeat(60), &format!("{}b", "a".repeat(59))).unwrap();
    assert!(a.len() <= 64);
    assert_ne!(a, b);
}

// ---- createAgentIdentity ------------------------------------------------------------------------------

#[tokio::test]
async fn creates_role_under_path_with_boundary_tags_policy_and_profile() {
    let id = identity();
    let (roles, policies, profiles, adds): (
        Log<CreateRoleInput>,
        Log<PutRolePolicyInput>,
        Log<CreateInstanceProfileInput>,
        Log<AddRoleToInstanceProfileInput>,
    ) = (Log::new(), Log::new(), Log::new(), Log::new());
    let role_arn = id.role_ref.clone();
    let profile_arn = id.instance_profile_ref.clone();
    let rules = [
        mock!(aws_sdk_iam::Client::create_role)
            .match_requests(rec(&roles))
            .then_output(move || {
                CreateRoleOutput::builder()
                    .role(iam_role(&role_arn))
                    .build()
            }),
        mock!(aws_sdk_iam::Client::put_role_policy)
            .match_requests(rec(&policies))
            .then_output(|| PutRolePolicyOutput::builder().build()),
        mock!(aws_sdk_iam::Client::create_instance_profile)
            .match_requests(rec(&profiles))
            .then_output(move || {
                CreateInstanceProfileOutput::builder()
                    .instance_profile(iam_profile(&profile_arn))
                    .build()
            }),
        mock!(aws_sdk_iam::Client::add_role_to_instance_profile)
            .match_requests(rec(&adds))
            .then_output(|| AddRoleToInstanceProfileOutput::builder().build()),
    ];
    let r = rig(NO_RULES, &rules, NO_RULES);
    let got = r
        .compute
        .create_agent_identity(&OptimizationId::from("opt1"), &AgentId::from("a1"))
        .await
        .unwrap();
    assert_eq!(got, id);

    let role = roles.first();
    assert_eq!(role.role_name(), Some("ratchet-agent-opt1-a1"));
    assert_eq!(role.path(), Some("/ratchet/agent/"));
    assert_eq!(
        role.permissions_boundary(),
        Some(cfg().agent_boundary_arn.as_str())
    );
    let tags: Vec<(&str, &str)> = role.tags().iter().map(|t| (t.key(), t.value())).collect();
    assert!(tags.contains(&("ratchet:opt", "opt1")));
    assert!(tags.contains(&("ratchet:agent", "a1")));
    assert!(tags.contains(&("ratchet:sandbox", "demo")));
    let trust: serde_json::Value =
        serde_json::from_str(role.assume_role_policy_document().unwrap()).unwrap();
    assert_eq!(
        trust["Statement"][0]["Principal"],
        serde_json::json!({ "Service": "ec2.amazonaws.com" })
    );
    assert_eq!(
        trust["Statement"][0]["Condition"]["StringEquals"]["aws:SourceAccount"],
        "123456789012"
    );

    let pol = policies.first();
    assert_eq!(pol.role_name(), Some("ratchet-agent-opt1-a1"));
    assert_eq!(pol.policy_name(), Some(AGENT_POLICY_NAME));
    let doc: serde_json::Value = serde_json::from_str(pol.policy_document().unwrap()).unwrap();
    let stmt = |sid: &str| {
        doc["Statement"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["Sid"] == sid)
            .unwrap()
            .clone()
    };
    assert_eq!(
        stmt("ArtifactObjects")["Resource"],
        "arn:aws:s3:::ratchet-demo-artifacts/artifacts/opt1/a1/*"
    );
    assert_eq!(
        stmt("OwnSecrets")["Resource"],
        "arn:aws:ssm:us-east-1:123456789012:parameter/ratchet/opt1/a1/*"
    );
    assert!(!doc.to_string().contains("iam:"));

    let prof = profiles.first();
    assert_eq!(prof.instance_profile_name(), Some("ratchet-agent-opt1-a1"));
    assert_eq!(prof.path(), Some("/ratchet/agent/"));
    let add = adds.first();
    assert_eq!(add.instance_profile_name(), Some("ratchet-agent-opt1-a1"));
    assert_eq!(add.role_name(), Some("ratchet-agent-opt1-a1"));
}

#[tokio::test]
async fn create_agent_identity_is_idempotent_when_role_and_profile_already_exist() {
    let rules = [
        mock!(aws_sdk_iam::Client::create_role)
            .then_error(|| CreateRoleError::generic(meta("EntityAlreadyExists", "exists"))),
        mock!(aws_sdk_iam::Client::put_role_policy)
            .then_output(|| PutRolePolicyOutput::builder().build()),
        mock!(aws_sdk_iam::Client::create_instance_profile).then_error(|| {
            CreateInstanceProfileError::generic(meta("EntityAlreadyExists", "exists"))
        }),
        mock!(aws_sdk_iam::Client::add_role_to_instance_profile).then_error(|| {
            AddRoleToInstanceProfileError::generic(meta("LimitExceeded", "already has a role"))
        }),
    ];
    let r = rig(NO_RULES, &rules, NO_RULES);
    assert_eq!(
        r.compute
            .create_agent_identity(&OptimizationId::from("opt1"), &AgentId::from("a1"))
            .await
            .unwrap(),
        identity()
    );
}

#[tokio::test]
async fn create_agent_identity_propagates_unexpected_iam_errors() {
    let rules = [mock!(aws_sdk_iam::Client::create_role)
        .then_error(|| CreateRoleError::generic(meta("AccessDenied", "not allowed")))];
    let r = rig(NO_RULES, &rules, NO_RULES);
    let e = r
        .compute
        .create_agent_identity(&OptimizationId::from("opt1"), &AgentId::from("a1"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("AccessDenied"), "{e}");
}

#[test]
fn rejects_a_malformed_role_path() {
    let mut c = cfg();
    c.agent_role_path = "ratchet/agent".into();
    let e = try_new(c).err().expect("must fail");
    assert!(e.to_string().contains("agentRolePath"), "{e}");
}

// ---- kill switch + delete -----------------------------------------------------------------------------

#[tokio::test]
async fn revoke_attaches_an_inline_deny_all() {
    let log: Log<PutRolePolicyInput> = Log::new();
    let rule = mock!(aws_sdk_iam::Client::put_role_policy)
        .match_requests(rec(&log))
        .then_output(|| PutRolePolicyOutput::builder().build());
    let r = rig(NO_RULES, &[rule], NO_RULES);
    r.compute.revoke_agent_identity(&identity()).await.unwrap();
    let i = log.first();
    assert_eq!(i.role_name(), Some("ratchet-agent-opt1-a1"));
    assert_eq!(i.policy_name(), Some(KILL_SWITCH_POLICY_NAME));
    let doc: serde_json::Value = serde_json::from_str(i.policy_document().unwrap()).unwrap();
    assert_eq!(
        doc["Statement"],
        serde_json::json!([{ "Sid": "KillSwitchDenyAll", "Effect": "Deny", "Action": "*", "Resource": "*" }])
    );
}

#[tokio::test]
async fn deletes_in_dependency_order_profile_role_removal_profile_policies_role() {
    let order: Log<&'static str> = Log::new();
    let removes: Log<RemoveRoleFromInstanceProfileInput> = Log::new();
    let deletes_profile: Log<DeleteInstanceProfileInput> = Log::new();
    let delete_policies: Log<DeleteRolePolicyInput> = Log::new();
    let detaches: Log<DetachRolePolicyInput> = Log::new();
    let delete_roles: Log<DeleteRoleInput> = Log::new();
    let rules = [
        mock!(aws_sdk_iam::Client::remove_role_from_instance_profile)
            .match_requests({
                let (o, l) = (order.clone(), removes.clone());
                move |i: &RemoveRoleFromInstanceProfileInput| {
                    o.push("RemoveRoleFromInstanceProfile");
                    l.push(i.clone());
                    true
                }
            })
            .then_output(|| RemoveRoleFromInstanceProfileOutput::builder().build()),
        mock!(aws_sdk_iam::Client::delete_instance_profile)
            .match_requests({
                let (o, l) = (order.clone(), deletes_profile.clone());
                move |i: &DeleteInstanceProfileInput| {
                    o.push("DeleteInstanceProfile");
                    l.push(i.clone());
                    true
                }
            })
            .then_output(|| DeleteInstanceProfileOutput::builder().build()),
        mock!(aws_sdk_iam::Client::list_role_policies)
            .match_requests(rec_op::<ListRolePoliciesInput>(&order, "ListRolePolicies"))
            .then_output(|| {
                ListRolePoliciesOutput::builder()
                    .policy_names("ratchet-agent-permissions")
                    .policy_names(KILL_SWITCH_POLICY_NAME)
                    .build()
                    .unwrap()
            }),
        mock!(aws_sdk_iam::Client::delete_role_policy)
            .match_requests({
                let (o, l) = (order.clone(), delete_policies.clone());
                move |i: &DeleteRolePolicyInput| {
                    o.push("DeleteRolePolicy");
                    l.push(i.clone());
                    true
                }
            })
            .then_output(|| DeleteRolePolicyOutput::builder().build()),
        mock!(aws_sdk_iam::Client::list_attached_role_policies)
            .match_requests(rec_op::<ListAttachedRolePoliciesInput>(
                &order,
                "ListAttachedRolePolicies",
            ))
            .then_output(|| {
                ListAttachedRolePoliciesOutput::builder()
                    .attached_policies(
                        iamt::AttachedPolicy::builder()
                            .policy_arn("arn:aws:iam::aws:policy/Foo")
                            .build(),
                    )
                    .build()
            }),
        mock!(aws_sdk_iam::Client::detach_role_policy)
            .match_requests({
                let (o, l) = (order.clone(), detaches.clone());
                move |i: &DetachRolePolicyInput| {
                    o.push("DetachRolePolicy");
                    l.push(i.clone());
                    true
                }
            })
            .then_output(|| DetachRolePolicyOutput::builder().build()),
        mock!(aws_sdk_iam::Client::delete_role)
            .match_requests({
                let (o, l) = (order.clone(), delete_roles.clone());
                move |i: &DeleteRoleInput| {
                    o.push("DeleteRole");
                    l.push(i.clone());
                    true
                }
            })
            .then_output(|| DeleteRoleOutput::builder().build()),
    ];
    let r = rig(NO_RULES, &rules, NO_RULES);
    r.compute.delete_agent_identity(&identity()).await.unwrap();
    assert_eq!(
        order.all(),
        [
            "RemoveRoleFromInstanceProfile",
            "DeleteInstanceProfile",
            "ListRolePolicies",
            "DeleteRolePolicy",
            "DeleteRolePolicy",
            "ListAttachedRolePolicies",
            "DetachRolePolicy",
            "DeleteRole",
        ]
    );
    let rm = removes.first();
    assert_eq!(
        (rm.instance_profile_name(), rm.role_name()),
        (Some("ratchet-agent-opt1-a1"), Some("ratchet-agent-opt1-a1"))
    );
    assert_eq!(
        deletes_profile.first().instance_profile_name(),
        Some("ratchet-agent-opt1-a1")
    );
    let names: Vec<_> = delete_policies
        .all()
        .iter()
        .map(|i| i.policy_name().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        ["ratchet-agent-permissions", KILL_SWITCH_POLICY_NAME]
    );
    assert_eq!(
        detaches.first().policy_arn(),
        Some("arn:aws:iam::aws:policy/Foo")
    );
    assert_eq!(
        delete_roles.first().role_name(),
        Some("ratchet-agent-opt1-a1")
    );
}

fn no_such_entity_rules() -> Vec<Rule> {
    let nse = || meta("NoSuchEntity", "gone");
    vec![
        mock!(aws_sdk_iam::Client::remove_role_from_instance_profile)
            .then_error(move || RemoveRoleFromInstanceProfileError::generic(nse())),
        mock!(aws_sdk_iam::Client::delete_instance_profile)
            .then_error(move || DeleteInstanceProfileError::generic(nse())),
        mock!(aws_sdk_iam::Client::list_role_policies)
            .then_error(move || ListRolePoliciesError::generic(nse())),
        mock!(aws_sdk_iam::Client::list_attached_role_policies)
            .then_error(move || ListAttachedRolePoliciesError::generic(nse())),
        mock!(aws_sdk_iam::Client::delete_role).then_error(move || DeleteRoleError::generic(nse())),
    ]
}

#[tokio::test]
async fn delete_is_idempotent_when_everything_is_already_gone() {
    let r = rig(NO_RULES, &no_such_entity_rules(), NO_RULES);
    r.compute.delete_agent_identity(&identity()).await.unwrap();
}

#[tokio::test]
async fn delete_does_not_swallow_other_errors() {
    let mut rules = vec![
        mock!(aws_sdk_iam::Client::remove_role_from_instance_profile)
            .then_error(|| RemoveRoleFromInstanceProfileError::generic(meta("AccessDenied", "no"))),
    ];
    rules.extend(no_such_entity_rules().into_iter().skip(1));
    let r = rig(NO_RULES, &rules, NO_RULES);
    let e = r
        .compute
        .delete_agent_identity(&identity())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("AccessDenied"), "{e}");
}

#[tokio::test]
async fn list_agent_roles_pages_through_the_agent_path() {
    let calls: Log<ListRolesInput> = Log::new();
    let role = |n: &str| {
        iamt::Role::builder()
            .path("/ratchet/agent/")
            .role_name(n)
            .role_id("AROA")
            .arn("arn")
            .create_date(DateTime::from_secs(0))
            .build()
            .unwrap()
    };
    let rule = mock!(aws_sdk_iam::Client::list_roles)
        .match_requests(rec(&calls))
        .sequence()
        .output(move || {
            ListRolesOutput::builder()
                .roles(role("r1"))
                .is_truncated(true)
                .marker("m")
                .build()
                .unwrap()
        })
        .output(move || {
            ListRolesOutput::builder()
                .roles(
                    iamt::Role::builder()
                        .path("/p/")
                        .role_name("r2")
                        .role_id("AROA")
                        .arn("arn")
                        .create_date(DateTime::from_secs(0))
                        .build()
                        .unwrap(),
                )
                .is_truncated(false)
                .build()
                .unwrap()
        })
        .build();
    let client = mock_client!(aws_sdk_iam, RuleMode::MatchAny, &[&rule]);
    assert_eq!(
        list_agent_roles(&client, "/ratchet/agent/").await.unwrap(),
        ["r1", "r2"]
    );
    let calls = calls.all();
    assert_eq!(calls[0].path_prefix(), Some("/ratchet/agent/"));
    assert_eq!(calls[1].marker(), Some("m"));
}

// ---- secrets ------------------------------------------------------------------------------------------

#[tokio::test]
async fn puts_a_secure_string_under_the_agent_path_and_tags_it() {
    let (puts, tags): (Log<PutParameterInput>, Log<AddTagsToResourceInput>) =
        (Log::new(), Log::new());
    let rules = [
        mock!(aws_sdk_ssm::Client::put_parameter)
            .match_requests(rec(&puts))
            .then_output(|| PutParameterOutput::builder().build()),
        mock!(aws_sdk_ssm::Client::add_tags_to_resource)
            .match_requests(rec(&tags))
            .then_output(|| AddTagsToResourceOutput::builder().build()),
    ];
    let r = rig(NO_RULES, NO_RULES, &rules);
    let got = r
        .compute
        .put_secret(
            &OptimizationId::from("opt1"),
            &AgentId::from("a1"),
            "ANTHROPIC_API_KEY",
            "sk-secret",
        )
        .await
        .unwrap();
    assert_eq!(
        got,
        SecretRef {
            name: "ANTHROPIC_API_KEY".into(),
            reference: "/ratchet/opt1/a1/ANTHROPIC_API_KEY".into()
        }
    );
    let p = puts.first();
    assert_eq!(p.name(), Some("/ratchet/opt1/a1/ANTHROPIC_API_KEY"));
    assert_eq!(p.value(), Some("sk-secret"));
    assert_eq!(p.r#type(), Some(&ssmt::ParameterType::SecureString));
    assert_eq!(p.overwrite(), Some(true));
    // PutParameter rejects Tags together with Overwrite, so the tags go in a separate call.
    assert!(p.tags().is_empty());
    let t = tags.first();
    assert_eq!(
        t.resource_type(),
        Some(&ssmt::ResourceTypeForTagging::Parameter)
    );
    assert_eq!(t.resource_id(), Some("/ratchet/opt1/a1/ANTHROPIC_API_KEY"));
    let tv: Vec<_> = t.tags().iter().map(|t| (t.key(), t.value())).collect();
    assert_eq!(tv, [("ratchet:opt", "opt1"), ("ratchet:agent", "a1")]);
}

#[tokio::test]
async fn deletes_all_secrets_under_the_agent_path_paging_and_batching_by_10() {
    let (gets, deletes): (Log<GetParametersByPathInput>, Log<Vec<String>>) =
        (Log::new(), Log::new());
    let param = |i: usize| {
        ssmt::Parameter::builder()
            .name(format!("/ratchet/opt1/a1/K{i}"))
            .build()
    };
    let rules = [
        mock!(aws_sdk_ssm::Client::get_parameters_by_path)
            .match_requests(rec(&gets))
            .sequence()
            .output(move || {
                let mut b = GetParametersByPathOutput::builder().next_token("t");
                for i in 0..7 {
                    b = b.parameters(param(i));
                }
                b.build()
            })
            .output(move || {
                let mut b = GetParametersByPathOutput::builder();
                for i in 7..12 {
                    b = b.parameters(param(i));
                }
                b.build()
            })
            .build(),
        mock!(aws_sdk_ssm::Client::delete_parameters)
            .match_requests(crate::common::rec_with(
                &deletes,
                |i: &DeleteParametersInput| i.names().to_vec(),
            ))
            .then_output(|| DeleteParametersOutput::builder().build()),
    ];
    let r = rig(NO_RULES, NO_RULES, &rules);
    r.compute
        .delete_secrets(&OptimizationId::from("opt1"), &AgentId::from("a1"))
        .await
        .unwrap();
    let g = gets.all();
    assert_eq!(g[0].path(), Some("/ratchet/opt1/a1/"));
    assert_eq!(g[0].recursive(), Some(true));
    assert_eq!(g[0].with_decryption(), Some(false));
    assert_eq!(g[1].next_token(), Some("t"));
    assert_eq!(
        deletes.all().iter().map(Vec::len).collect::<Vec<_>>(),
        [10, 2]
    );
}

#[tokio::test]
async fn delete_secrets_with_none_present_does_not_call_delete() {
    // No DeleteParameters rule: calling it would panic the mock.
    let rule = mock!(aws_sdk_ssm::Client::get_parameters_by_path)
        .then_output(|| GetParametersByPathOutput::builder().build());
    let r = rig(NO_RULES, NO_RULES, &[rule]);
    r.compute
        .delete_secrets(&OptimizationId::from("opt1"), &AgentId::from("a1"))
        .await
        .unwrap();
}

// ---- launch -------------------------------------------------------------------------------------------

#[tokio::test]
async fn launch_runner_tags_imdsv2_terminate_on_shutdown_profile_network_user_data() {
    let log: Log<RunInstancesInput> = Log::new();
    let rule = mock!(aws_sdk_ec2::Client::run_instances)
        .match_requests(rec(&log))
        .then_output(|| {
            RunInstancesOutput::builder()
                .instances(
                    ec2t::Instance::builder()
                        .instance_id("i-1")
                        .launch_time(DateTime::from_millis(5000))
                        .build(),
                )
                .build()
        });
    let r = rig(&[rule], NO_RULES, NO_RULES);
    let info = r.compute.launch_runner(&runner_spec()).await.unwrap();
    assert_eq!(info.id, "i-1");
    assert_eq!(info.role, InstanceRole::AgentRunner);
    assert_eq!(info.optimization_id, "opt1");
    assert_eq!(info.agent_id.as_ref().map(AgentId::as_str), Some("a1"));
    assert_eq!(info.instance_type, "c7i.large");
    assert_eq!(info.state, InstanceState::Pending);
    assert_eq!(info.launched_at, 5000);
    assert_eq!(info.terminated_at, None);

    let input = log.first();
    assert_eq!(input.image_id(), Some("ami-runner"));
    assert_eq!(input.instance_type().map(|t| t.as_str()), Some("c7i.large"));
    assert_eq!((input.min_count(), input.max_count()), (Some(1), Some(1)));
    assert_eq!(input.subnet_id(), Some("subnet-agent"));
    assert_eq!(input.security_group_ids(), ["sg-runner"]);
    assert_eq!(
        input.iam_instance_profile().and_then(|p| p.arn()),
        Some(identity().instance_profile_ref.as_str())
    );
    let md = input.metadata_options().unwrap();
    assert_eq!(md.http_tokens(), Some(&ec2t::HttpTokensState::Required));
    assert_eq!(
        md.http_endpoint(),
        Some(&ec2t::InstanceMetadataEndpointState::Enabled)
    );
    assert_eq!(md.http_put_response_hop_limit(), Some(1));
    assert_eq!(
        input.instance_initiated_shutdown_behavior(),
        Some(&ec2t::ShutdownBehavior::Terminate)
    );

    let inst_tags = tags_of(&input, ec2t::ResourceType::Instance);
    for (k, v) in [
        ("ratchet:opt", "opt1"),
        ("ratchet:agent", "a1"),
        ("ratchet:role", "agent_runner"),
        ("ratchet:sandbox", "demo"),
    ] {
        assert!(has_tag(&inst_tags, k, v), "{k}={v} in {inst_tags:?}");
    }
    let vol_tags = tags_of(&input, ec2t::ResourceType::Volume);
    assert!(has_tag(&vol_tags, "ratchet:opt", "opt1") && has_tag(&vol_tags, "ratchet:agent", "a1"));
    assert!(vol_tags.iter().all(|(k, _)| k != "Name"));

    let ud = decode_user_data(&input);
    assert!(ud.contains("touch /run/ratchet/user-data.done"));
    // the runner loop is started over SSM, never at boot
    assert!(!ud.contains("nohup") && !ud.contains("ratchet runner"));
    assert!(ud.contains("shutdown -h +120"));
    assert!(ud.contains("RATCHET_SECRET_REFS="));
    assert!(ud.contains("/ratchet/opt1/a1/ANTHROPIC_API_KEY"));
    // hostile repo url is single-quoted, not executable
    assert!(ud.contains(r"RATCHET_REPO_URL='https://github.com/o/r'\''; rm -rf / #.git'"));
}

#[tokio::test]
async fn launch_bench_instance_confirm_uses_the_confirm_profile_the_private_bench_subnet_and_bench_sg_no_agent_tag()
 {
    let log: Log<RunInstancesInput> = Log::new();
    let r = rig(&[run_instances_ok(&log, "i-b")], NO_RULES, NO_RULES);
    let info = r
        .compute
        .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Confirm))
        .await
        .unwrap();
    assert_eq!(info.role, InstanceRole::Bench);
    assert_eq!(info.agent_id, None);
    assert_eq!(info.launched_at, 1_000_000); // clock fallback when LaunchTime is absent
    let input = log.first();
    assert_eq!(
        input.iam_instance_profile().and_then(|p| p.arn()),
        cfg().confirm_instance_profile.as_deref()
    );
    assert_eq!(input.subnet_id(), Some("subnet-bench"));
    assert_eq!(input.security_group_ids(), ["sg-bench"]);
    let md = input.metadata_options().unwrap();
    assert_eq!(md.http_tokens(), Some(&ec2t::HttpTokensState::Required));
    assert_eq!(md.http_put_response_hop_limit(), Some(1));
    assert_eq!(
        input.instance_initiated_shutdown_behavior(),
        Some(&ec2t::ShutdownBehavior::Terminate)
    );
    let tags = tags_of(&input, ec2t::ResourceType::Instance);
    assert!(
        has_tag(&tags, "ratchet:opt", "opt1")
            && has_tag(&tags, "ratchet:role", "bench")
            && has_tag(&tags, "ratchet:purpose", "confirm")
    );
    assert!(tags.iter().all(|(k, _)| k != "ratchet:agent"));
    assert!(decode_user_data(&input).contains("touch /run/ratchet/user-data.done"));
}

#[tokio::test]
async fn launch_bench_instance_baseline_and_bench_never_get_the_sealed_capable_profile() {
    for purpose in [BenchLaunchPurpose::Baseline, BenchLaunchPurpose::Bench] {
        let log: Log<RunInstancesInput> = Log::new();
        let r = rig(&[run_instances_ok(&log, "i-b")], NO_RULES, NO_RULES);
        r.compute
            .launch_bench_instance(&bench_spec(purpose))
            .await
            .unwrap();
        let input = log.first();
        assert_eq!(
            input.iam_instance_profile().and_then(|p| p.arn()),
            Some(cfg().bench_instance_profile.as_str())
        );
        assert!(
            !format!("{input:?}").contains("confirm-runner"),
            "{purpose:?}"
        );
        assert_eq!(input.subnet_id(), Some("subnet-bench"));
    }
}

#[test]
fn picks_the_profile_by_purpose_and_fails_closed() {
    let c = cfg();
    assert_eq!(
        bench_profile_for(&c, BenchLaunchPurpose::Baseline).unwrap(),
        c.bench_instance_profile
    );
    assert_eq!(
        bench_profile_for(&c, BenchLaunchPurpose::Bench).unwrap(),
        c.bench_instance_profile
    );
    assert_eq!(
        bench_profile_for(&c, BenchLaunchPurpose::Confirm).unwrap(),
        c.confirm_instance_profile.as_deref().unwrap()
    );
    let only_bench = AwsComputeConfig {
        bench_instance_profile: "b".into(),
        ..Default::default()
    };
    let e = bench_profile_for(&only_bench, BenchLaunchPurpose::Confirm).unwrap_err();
    assert!(
        e.to_string()
            .contains("confirmInstanceProfile is not configured"),
        "{e}"
    );
    // An unknown purpose cannot be built: the type is closed, and the JSON form is rejected.
    assert!(serde_json::from_str::<BenchLaunchPurpose>("\"confirm-ish\"").is_err());
}

#[tokio::test]
async fn refuses_a_confirm_launch_without_a_confirm_profile_instead_of_falling_back_to_the_bench_one()
 {
    let log: Log<RunInstancesInput> = Log::new();
    let mut c = cfg();
    c.confirm_instance_profile = None;
    let r = rig_with(c, &[run_instances_ok(&log, "i-b")], NO_RULES, NO_RULES);
    let e = r
        .compute
        .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Confirm))
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("confirmInstanceProfile is not configured"),
        "{e}"
    );
    assert!(log.is_empty());
    // bench phases need no sealed identity
    r.compute
        .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Bench))
        .await
        .unwrap();
}

#[test]
fn refuses_to_share_one_profile_between_bench_and_confirm() {
    let mut c = cfg();
    c.confirm_instance_profile = Some(c.bench_instance_profile.clone());
    let e = try_new(c).err().expect("must fail");
    assert!(e.to_string().contains("must differ"), "{e}");
}

#[tokio::test]
async fn refuses_to_launch_bench_instances_without_a_bench_subnet_no_fallback_to_the_agent_subnet()
{
    let log: Log<RunInstancesInput> = Log::new();
    let mut c = cfg();
    c.bench_subnet_id = None;
    let r = rig_with(c, &[run_instances_ok(&log, "i-b")], NO_RULES, NO_RULES);
    let e = r
        .compute
        .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Bench))
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("benchSubnetId is not configured"),
        "{e}"
    );
    assert!(log.is_empty());
}

#[tokio::test]
async fn launches_agent_runners_in_the_agent_subnet() {
    let log: Log<RunInstancesInput> = Log::new();
    let r = rig(&[run_instances_ok(&log, "i-r")], NO_RULES, NO_RULES);
    r.compute.launch_runner(&runner_spec()).await.unwrap();
    assert_eq!(log.first().subnet_id(), Some("subnet-agent"));
}

#[tokio::test]
async fn accepts_a_bare_profile_name_for_bench() {
    let log: Log<RunInstancesInput> = Log::new();
    let mut c = cfg();
    c.bench_instance_profile = "bench-profile".into();
    let r = rig_with(c, &[run_instances_ok(&log, "i-b")], NO_RULES, NO_RULES);
    r.compute
        .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Bench))
        .await
        .unwrap();
    let p = log.first().iam_instance_profile().cloned().unwrap();
    assert_eq!((p.name(), p.arn()), (Some("bench-profile"), None));
}

mod deps {
    use super::*;

    async fn launch(c: AwsComputeConfig) -> (Rig, RunInstancesInput) {
        let log: Log<RunInstancesInput> = Log::new();
        let r = rig_with(c, &[run_instances_ok(&log, "i-d")], NO_RULES, NO_RULES);
        let info = r
            .compute
            .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Deps))
            .await
            .unwrap();
        assert_eq!(info.role, InstanceRole::Bench);
        assert_eq!(info.agent_id, None);
        let input = log.first();
        (r, input)
    }

    #[tokio::test]
    async fn runs_in_the_agent_subnet_with_the_agent_runner_group_and_its_own_profile_never_the_bench_or_confirm_ones()
     {
        let (_r, input) = launch(cfg()).await;
        assert_eq!(input.subnet_id(), Some("subnet-agent"));
        assert_eq!(input.security_group_ids(), ["sg-runner"]);
        assert_eq!(
            input.iam_instance_profile().and_then(|p| p.arn()),
            cfg().deps_instance_profile.as_deref()
        );
        let json = format!("{input:?}");
        for needle in ["subnet-bench", "sg-bench", "bench-runner", "confirm-runner"] {
            assert!(!json.contains(needle), "{needle} leaked into {json}");
        }
    }

    #[tokio::test]
    async fn keeps_imdsv2_terminate_on_shutdown_and_the_tags_the_reaper_and_the_teardown_orphan_check_rely_on()
     {
        let (_r, input) = launch(cfg()).await;
        let md = input.metadata_options().unwrap();
        assert_eq!(md.http_tokens(), Some(&ec2t::HttpTokensState::Required));
        assert_eq!(
            md.http_endpoint(),
            Some(&ec2t::InstanceMetadataEndpointState::Enabled)
        );
        assert_eq!(md.http_put_response_hop_limit(), Some(1));
        assert_eq!(
            input.instance_initiated_shutdown_behavior(),
            Some(&ec2t::ShutdownBehavior::Terminate)
        );
        let tags = tags_of(&input, ec2t::ResourceType::Instance);
        for (k, v) in [
            ("ratchet:opt", "opt1"),
            ("ratchet:role", "bench"),
            ("ratchet:purpose", "deps"),
            ("ratchet:sandbox", "demo"),
        ] {
            assert!(has_tag(&tags, k, v), "{k}={v} in {tags:?}");
        }
        assert!(tags.iter().all(|(k, _)| k != "ratchet:agent"));
    }

    #[tokio::test]
    async fn fails_closed_without_a_deps_profile_no_launch_no_fallback_to_the_bench_identity_or_subnet()
     {
        let log: Log<RunInstancesInput> = Log::new();
        let mut c = cfg();
        c.deps_instance_profile = None;
        let r = rig_with(c, &[run_instances_ok(&log, "i-d")], NO_RULES, NO_RULES);
        let e = r
            .compute
            .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Deps))
            .await
            .unwrap_err();
        assert!(
            e.to_string()
                .contains("depsInstanceProfile is not configured"),
            "{e}"
        );
        assert!(log.is_empty());
    }

    #[tokio::test]
    async fn needs_no_bench_subnet_it_is_not_a_bench_and_a_bench_launch_still_needs_one() {
        let mut c = cfg();
        c.bench_subnet_id = None;
        let (r, input) = launch(c).await;
        assert_eq!(input.subnet_id(), Some("subnet-agent"));
        let e = r
            .compute
            .launch_bench_instance(&bench_spec(BenchLaunchPurpose::Bench))
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("benchSubnetId is not configured"),
            "{e}"
        );
    }

    #[test]
    fn refuses_to_share_a_profile_between_the_snapshot_builder_and_the_benches() {
        let mut c = cfg();
        c.deps_instance_profile = Some(c.bench_instance_profile.clone());
        assert!(
            try_new(c)
                .err()
                .expect("must fail")
                .to_string()
                .contains("depsInstanceProfile must differ")
        );
        let mut c = cfg();
        c.deps_instance_profile = c.confirm_instance_profile.clone();
        assert!(
            try_new(c)
                .err()
                .expect("must fail")
                .to_string()
                .contains("depsInstanceProfile must differ")
        );
    }

    #[test]
    fn picks_the_deps_profile_for_deps_and_for_nothing_else() {
        let c = cfg();
        assert_eq!(
            bench_profile_for(&c, BenchLaunchPurpose::Deps).unwrap(),
            c.deps_instance_profile.as_deref().unwrap()
        );
        for p in [
            BenchLaunchPurpose::Baseline,
            BenchLaunchPurpose::Bench,
            BenchLaunchPurpose::Confirm,
        ] {
            assert_ne!(
                bench_profile_for(&c, p).unwrap(),
                c.deps_instance_profile.as_deref().unwrap()
            );
        }
        let partial = AwsComputeConfig {
            bench_instance_profile: "b".into(),
            confirm_instance_profile: Some("c".into()),
            ..Default::default()
        };
        let e = bench_profile_for(&partial, BenchLaunchPurpose::Deps).unwrap_err();
        assert!(
            e.to_string()
                .contains("depsInstanceProfile is not configured"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn reaches_the_launched_instance_proxy_from_the_sandbox_outputs() {
        let mut c = cfg();
        c.egress_proxy_url = Some("http://10.42.3.10:3128".into());
        let (_r, input) = launch(c).await;
        let data = decode_user_data(&input);
        assert!(data.contains("HTTPS_PROXY='http://10.42.3.10:3128'"));
        assert!(data.contains("--uid-owner ratchet-deps"));
    }
}

#[tokio::test]
async fn launch_runner_passes_the_configured_proxy_into_user_data() {
    let log: Log<RunInstancesInput> = Log::new();
    let mut c = cfg();
    c.egress_proxy_url = Some("http://10.42.3.10:3128".into());
    let r = rig_with(c, &[run_instances_ok(&log, "i-r")], NO_RULES, NO_RULES);
    r.compute.launch_runner(&runner_spec()).await.unwrap();
    assert!(decode_user_data(&log.first()).contains("HTTPS_PROXY='http://10.42.3.10:3128'"));
}

#[tokio::test]
async fn derives_the_compute_config_from_sandbox_outputs_bench_profile_for_bench_confirm_profile_for_confirm()
 {
    let input = SandboxInput {
        sandbox_id: "sim".into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["t3.medium".into()],
        max_instances_per_agent: 4,
        max_instance_lifetime_hours: 2.0,
        tags: None,
        environment: None,
    };
    let out = MockSandboxProvider::default().apply(&input).await.unwrap();
    let derived = AwsComputeConfig::from_outputs(&out, "sim");
    assert_eq!(
        derived.bench_instance_profile,
        out.eval_identity.instance_profile_arn
    );
    assert_eq!(
        derived.confirm_instance_profile.as_deref(),
        Some(out.confirm_identity.instance_profile_arn.as_str())
    );
    assert_ne!(
        Some(derived.bench_instance_profile.as_str()),
        derived.confirm_instance_profile.as_deref()
    );
    assert_eq!(
        derived.bench_subnet_id.as_deref(),
        Some(out.network.bench_subnet_id.as_str())
    );
    assert_ne!(
        derived.bench_subnet_id.as_deref(),
        Some(derived.subnet_id.as_str())
    );
    assert_eq!(
        derived.deps_instance_profile.as_deref(),
        Some(
            out.deps_identity
                .as_ref()
                .unwrap()
                .instance_profile_arn
                .as_str()
        )
    );
    let distinct: std::collections::HashSet<_> = [
        Some(derived.bench_instance_profile.clone()),
        derived.confirm_instance_profile.clone(),
        derived.deps_instance_profile.clone(),
    ]
    .into_iter()
    .collect();
    assert_eq!(distinct.len(), 3);
    // The derived config is a valid one
    assert!(try_new(derived).is_ok());
}

#[tokio::test]
async fn retries_while_the_fresh_instance_profile_propagates_then_succeeds() {
    let log: Log<RunInstancesInput> = Log::new();
    let rule = mock!(aws_sdk_ec2::Client::run_instances)
        .match_requests(rec(&log))
        .sequence()
        .error(|| RunInstancesError::generic(meta("InvalidParameterValue", "Value (x) for parameter iamInstanceProfile.arn is invalid. Invalid IAM Instance Profile ARN")))
        .error(|| RunInstancesError::generic(meta("InvalidParameterValue", "Invalid IAM Instance Profile name")))
        .output(|| running("i-2"))
        .build();
    let r = rig(&[rule], NO_RULES, NO_RULES);
    let t0 = r.clock.now();
    let info = r.compute.launch_runner(&runner_spec()).await.unwrap();
    assert_eq!(info.id, "i-2");
    assert_eq!(log.len(), 3);
    assert_eq!(r.clock.now() - t0, 2000 + 4000);
}

#[tokio::test]
async fn does_not_retry_unrelated_errors_and_gives_up_after_the_retry_budget() {
    let log: Log<RunInstancesInput> = Log::new();
    let rule = mock!(aws_sdk_ec2::Client::run_instances)
        .match_requests(rec(&log))
        .then_error(|| RunInstancesError::generic(meta("UnauthorizedOperation", "nope")));
    let r = rig(&[rule], NO_RULES, NO_RULES);
    let e = r.compute.launch_runner(&runner_spec()).await.unwrap_err();
    assert!(e.to_string().contains("UnauthorizedOperation"), "{e}");
    assert_eq!(log.len(), 1);

    let log: Log<RunInstancesInput> = Log::new();
    let rule = mock!(aws_sdk_ec2::Client::run_instances)
        .match_requests(rec(&log))
        .then_error(|| {
            RunInstancesError::generic(meta(
                "InvalidParameterValue",
                "Invalid IAM Instance Profile",
            ))
        });
    let r = rig(&[rule], NO_RULES, NO_RULES);
    let e = r.compute.launch_runner(&runner_spec()).await.unwrap_err();
    assert!(
        e.to_string().contains("Invalid IAM Instance Profile"),
        "{e}"
    );
    assert_eq!(log.len(), 7);
}

#[tokio::test]
async fn refuses_invalid_ids_before_calling_aws() {
    let r = rig(NO_RULES, NO_RULES, NO_RULES);
    let mut spec = runner_spec();
    spec.agent_id = "A B".into();
    let e = r.compute.launch_runner(&spec).await.unwrap_err();
    assert!(e.to_string().contains("invalid agent id"), "{e}");
}

// ---- terminate + listTagged ---------------------------------------------------------------------------

#[tokio::test]
async fn terminates_no_ops_on_empty_tolerates_already_gone_ids() {
    let log: Log<TerminateInstancesInput> = Log::new();
    let ok = mock!(aws_sdk_ec2::Client::terminate_instances)
        .match_requests(rec(&log))
        .then_output(|| TerminateInstancesOutput::builder().build());
    let r = rig(&[ok], NO_RULES, NO_RULES);
    r.compute.terminate(&[]).await.unwrap();
    assert!(log.is_empty());
    r.compute
        .terminate(&["i-1".into(), "i-2".into()])
        .await
        .unwrap();
    assert_eq!(log.first().instance_ids(), ["i-1", "i-2"]);

    let gone = mock!(aws_sdk_ec2::Client::terminate_instances).then_error(|| {
        TerminateInstancesError::generic(meta("InvalidInstanceID.NotFound", "gone"))
    });
    rig(&[gone], NO_RULES, NO_RULES)
        .compute
        .terminate(&["i-9".into()])
        .await
        .unwrap();

    let denied = mock!(aws_sdk_ec2::Client::terminate_instances)
        .then_error(|| TerminateInstancesError::generic(meta("UnauthorizedOperation", "no")));
    assert!(
        rig(&[denied], NO_RULES, NO_RULES)
            .compute
            .terminate(&["i-9".into()])
            .await
            .is_err()
    );
}

fn inst(id: &str, tags: &[(&str, &str)], state: &str, reason: Option<&str>) -> ec2t::Instance {
    let mut b = ec2t::Instance::builder()
        .instance_id(id)
        .instance_type(ec2t::InstanceType::from("c7i.large"))
        .launch_time(DateTime::from_millis(1000))
        .state(
            ec2t::InstanceState::builder()
                .name(ec2t::InstanceStateName::from(state))
                .build(),
        );
    for (k, v) in tags {
        b = b.tags(ec2t::Tag::builder().key(*k).value(*v).build());
    }
    if let Some(r) = reason {
        b = b.state_transition_reason(r);
    }
    b.build()
}

#[tokio::test]
async fn list_tagged_filters_by_tag_pages_infers_roles_and_skips_untagged() {
    let log: Log<DescribeInstancesInput> = Log::new();
    let rule = mock!(aws_sdk_ec2::Client::describe_instances)
        .match_requests(rec(&log))
        .sequence()
        .output(|| {
            DescribeInstancesOutput::builder()
                .reservations(
                    ec2t::Reservation::builder()
                        .instances(inst(
                            "i-r",
                            &[
                                ("ratchet:opt", "o"),
                                ("ratchet:agent", "a"),
                                ("ratchet:role", "agent_runner"),
                            ],
                            "running",
                            None,
                        ))
                        .instances(inst("i-x", &[("Name", "foreign")], "running", None))
                        .build(),
                )
                .next_token("n")
                .build()
        })
        .output(|| {
            DescribeInstancesOutput::builder()
                .reservations(
                    ec2t::Reservation::builder()
                        .instances(inst(
                            "i-e",
                            &[("ratchet:opt", "o"), ("ratchet:agent", "a")],
                            "running",
                            None,
                        ))
                        .instances(inst(
                            "i-b",
                            &[("ratchet:opt", "o"), ("ratchet:role", "bench")],
                            "shutting-down",
                            None,
                        ))
                        .instances(inst(
                            "i-t",
                            &[("ratchet:opt", "o"), ("ratchet:agent", "a")],
                            "terminated",
                            Some("User initiated (2026-01-02 03:04:05 GMT)"),
                        ))
                        .build(),
                )
                .build()
        })
        .build();
    let r = rig(&[rule], NO_RULES, NO_RULES);
    let list = r
        .compute
        .list_tagged(Some(&OptimizationId::from("o")))
        .await
        .unwrap();
    let calls = log.all();
    let f = &calls[0].filters()[0];
    assert_eq!(
        (f.name(), f.values()),
        (Some("tag:ratchet:opt"), ["o".to_string()].as_slice())
    );
    assert_eq!(calls[1].next_token(), Some("n"));
    let got: Vec<_> = list
        .iter()
        .map(|i| (i.id.as_str(), i.role, i.state))
        .collect();
    assert_eq!(
        got,
        [
            ("i-r", InstanceRole::AgentRunner, InstanceState::Running),
            ("i-e", InstanceRole::Experiment, InstanceState::Running),
            ("i-b", InstanceRole::Bench, InstanceState::Stopping),
            ("i-t", InstanceRole::Experiment, InstanceState::Terminated),
        ]
    );
    assert_eq!(list[3].terminated_at, Some(1_767_323_045_000)); // 2026-01-02T03:04:05Z
    assert_eq!(list[0].agent_id.as_ref().map(AgentId::as_str), Some("a"));
    assert_eq!(list[2].agent_id, None);
    assert_eq!(list[0].launched_at, 1000);
}

#[tokio::test]
async fn list_tagged_without_an_optimization_uses_a_tag_key_filter() {
    let log: Log<DescribeInstancesInput> = Log::new();
    let rule = mock!(aws_sdk_ec2::Client::describe_instances)
        .match_requests(rec(&log))
        .then_output(|| DescribeInstancesOutput::builder().build());
    let r = rig(&[rule], NO_RULES, NO_RULES);
    assert!(r.compute.list_tagged(None).await.unwrap().is_empty());
    let first = log.first();
    let f = &first.filters()[0];
    assert_eq!(
        (f.name(), f.values()),
        (Some("tag-key"), ["ratchet:opt".to_string()].as_slice())
    );
}

#[test]
fn to_instance_info_ignores_instances_without_ratchet_opt() {
    assert_eq!(
        to_instance_info(&ec2t::Instance::builder().instance_id("i").build()),
        None
    );
}

// ---- runCommand ---------------------------------------------------------------------------------------

fn invocation(
    status: ssmt::CommandInvocationStatus,
    code: i32,
    out: &str,
    err: &str,
) -> GetCommandInvocationOutput {
    GetCommandInvocationOutput::builder()
        .status(status)
        .response_code(code)
        .standard_output_content(out)
        .standard_error_content(err)
        .build()
}

fn send_ok(log: &Log<SendCommandInput>) -> Rule {
    mock!(aws_sdk_ssm::Client::send_command)
        .match_requests(rec(log))
        .then_output(|| {
            SendCommandOutput::builder()
                .command(ssmt::Command::builder().command_id("c-1").build())
                .build()
        })
}

#[tokio::test]
async fn sends_run_shell_script_with_env_exports_and_polls_to_completion() {
    let (sends, polls): (Log<SendCommandInput>, Log<GetCommandInvocationInput>) =
        (Log::new(), Log::new());
    let poll = mock!(aws_sdk_ssm::Client::get_command_invocation)
        .match_requests(rec(&polls))
        .sequence()
        .error(|| GetCommandInvocationError::generic(meta("InvocationDoesNotExist", "not yet")))
        .output(|| {
            GetCommandInvocationOutput::builder()
                .status(ssmt::CommandInvocationStatus::InProgress)
                .build()
        })
        .output(|| invocation(ssmt::CommandInvocationStatus::Success, 0, "hi\n", ""))
        .build();
    let r = rig(NO_RULES, NO_RULES, &[send_ok(&sends), poll]);
    let cmd = RemoteCommand {
        script: "echo $X".into(),
        env: Some([("X".to_string(), "a'b".to_string())].into()),
        timeout_seconds: Some(120),
    };
    let res = r.compute.run_command("i-1", &cmd).await.unwrap();
    assert_eq!(
        (res.exit_code, res.stdout.as_str(), res.stderr.as_str()),
        (0, "hi\n", "")
    );
    let s = sends.first();
    assert_eq!(s.instance_ids(), ["i-1"]);
    assert_eq!(s.document_name(), Some("AWS-RunShellScript"));
    assert_eq!(s.timeout_seconds(), Some(120));
    let p = s.parameters().unwrap();
    assert_eq!(p["commands"], ["export X='a'\\''b'\necho $X"]);
    assert_eq!(p["executionTimeout"], ["120"]);
    let first = &polls.all()[0];
    assert_eq!(
        (first.command_id(), first.instance_id()),
        (Some("c-1"), Some("i-1"))
    );
}

#[tokio::test]
async fn returns_non_zero_exit_codes_and_timeouts_without_throwing() {
    let failed = mock!(aws_sdk_ssm::Client::get_command_invocation)
        .then_output(|| invocation(ssmt::CommandInvocationStatus::Failed, 3, "", "boom"));
    let r = rig(NO_RULES, NO_RULES, &[send_ok(&Log::new()), failed]);
    let res = r
        .compute
        .run_command("i-1", &RemoteCommand::new("false"))
        .await
        .unwrap();
    assert_eq!(res.exit_code, 3);
    assert!(res.stderr.contains("boom"), "{}", res.stderr);
    assert!(res.stderr.contains("[ssm status: Failed]"));

    let timed_out = mock!(aws_sdk_ssm::Client::get_command_invocation)
        .then_output(|| invocation(ssmt::CommandInvocationStatus::TimedOut, -1, "", ""));
    let r = rig(NO_RULES, NO_RULES, &[send_ok(&Log::new()), timed_out]);
    assert_eq!(
        r.compute
            .run_command("i-1", &RemoteCommand::new("sleep 9"))
            .await
            .unwrap()
            .exit_code,
        124
    );
}

#[tokio::test]
async fn rejects_bad_env_names_and_gives_up_past_the_deadline() {
    let r = rig(NO_RULES, NO_RULES, NO_RULES);
    let cmd = RemoteCommand {
        script: "x".into(),
        env: Some([("A;B".to_string(), "1".to_string())].into()),
        timeout_seconds: None,
    };
    let e = r.compute.run_command("i-1", &cmd).await.unwrap_err();
    assert!(e.to_string().contains("invalid env var"), "{e}");

    let busy = mock!(aws_sdk_ssm::Client::get_command_invocation).then_output(|| {
        GetCommandInvocationOutput::builder()
            .status(ssmt::CommandInvocationStatus::InProgress)
            .build()
    });
    let r = rig(NO_RULES, NO_RULES, &[send_ok(&Log::new()), busy]);
    let cmd = RemoteCommand {
        script: "x".into(),
        env: None,
        timeout_seconds: Some(5),
    };
    let e = r.compute.run_command("i-1", &cmd).await.unwrap_err();
    assert!(e.to_string().contains("still InProgress"), "{e}");
}

#[tokio::test]
async fn a_command_that_never_registers_is_an_error_after_the_deadline() {
    let never = mock!(aws_sdk_ssm::Client::get_command_invocation).then_error(|| {
        GetCommandInvocationError::generic(meta("InvocationDoesNotExist", "not yet"))
    });
    let r = rig(NO_RULES, NO_RULES, &[send_ok(&Log::new()), never]);
    let cmd = RemoteCommand {
        script: "x".into(),
        env: None,
        timeout_seconds: Some(5),
    };
    let e = r.compute.run_command("i-1", &cmd).await.unwrap_err();
    assert!(e.to_string().contains("never registered"), "{e}");
}

// ---- preflight ----------------------------------------------------------------------------------------

#[tokio::test]
async fn preflight_checks_the_account() {
    let r = rig(NO_RULES, NO_RULES, NO_RULES);
    assert_eq!(
        r.compute
            .preflight(Some("123456789012"))
            .await
            .unwrap()
            .account_id,
        "123456789012"
    );
    let e = r.compute.preflight(Some("999999999999")).await.unwrap_err();
    assert!(e.to_string().contains("expected sandbox account"), "{e}");
}
