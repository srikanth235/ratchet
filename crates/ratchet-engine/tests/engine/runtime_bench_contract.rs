//! `packages/runtime/src/bench-contract.test.ts`: the bench scripts (ratchet-bench) and the instance user-data (ratchet-sandbox)
//! are written by different crates but must agree on the OS users and the launcher path: a mismatch would make every live job
//! fail its isolation check, or worse, be checked against the wrong names.

use ratchet_bench::{
    BENCH_USER, CANDIDATE_LAUNCHER, DEPS_BASE_ARCHIVE, DEPS_PREFIX, DEPS_USER, HOLDOUT_USER,
    STAGING_PREFIX,
};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::{BenchLaunchPurpose, BenchLaunchSpec};
use ratchet_sandbox::compute::{
    BENCH_OS_USER, CANDIDATE_LAUNCHER_PATH, DEPS_OS_USER, HOLDOUT_OS_USER, bench_user_data,
    deps_user_data,
};

fn spec(purpose: BenchLaunchPurpose) -> BenchLaunchSpec {
    BenchLaunchSpec {
        optimization_id: OptimizationId::new("opt1"),
        instance_type: "c7i.2xlarge".into(),
        image_ref: "ami".into(),
        purpose,
    }
}

#[test]
fn agree_on_the_candidate_user_the_holdout_user_and_the_launcher_path() {
    assert_eq!(BENCH_OS_USER, BENCH_USER);
    assert_eq!(HOLDOUT_OS_USER, HOLDOUT_USER);
    assert_eq!(CANDIDATE_LAUNCHER_PATH, CANDIDATE_LAUNCHER);
}

#[test]
fn the_user_data_of_a_confirm_instance_sets_up_every_user_and_the_launcher_the_scripts_check_for() {
    let ud = bench_user_data(&spec(BenchLaunchPurpose::Confirm), 2.0).unwrap();
    assert!(ud.contains(&format!(
        "useradd --system --no-create-home --shell /usr/sbin/nologin {BENCH_USER}"
    )));
    assert!(ud.contains(&format!(
        "useradd --system --no-create-home --shell /usr/sbin/nologin {HOLDOUT_USER}"
    )));
    assert!(ud.contains(&format!(
        "install -D -m 755 -o root -g root /dev/null {CANDIDATE_LAUNCHER}"
    )));
    for u in [BENCH_USER, HOLDOUT_USER] {
        assert!(
            ud.contains(&format!("--uid-owner {u} -d 169.254.169.254 -j REJECT")),
            "{u}"
        );
    }
}

#[test]
fn agree_on_the_dependency_snapshot_user_and_the_user_data_of_a_deps_instance_sets_it_up_and_blocks_metadata_for_it()
 {
    assert_eq!(DEPS_OS_USER, DEPS_USER);
    let ud = deps_user_data(&spec(BenchLaunchPurpose::Deps), 2.0, None).unwrap();
    assert!(ud.contains(&format!(
        "useradd --system --no-create-home --shell /usr/sbin/nologin {DEPS_USER}"
    )));
    assert!(ud.contains(&format!(
        "--uid-owner {DEPS_USER} -d 169.254.169.254 -j REJECT"
    )));
}

#[test]
fn agree_on_the_prefixes_the_deps_identity_is_scoped_to_in_infra_aws() {
    // reads bench-inputs/*/deps-base.tgz, writes bench-deps/*
    let policy = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../infra/aws/policies/deps_runner_policy.json.tftpl"
    ))
    .unwrap();
    assert!(policy.contains(&format!("/{STAGING_PREFIX}*/{DEPS_BASE_ARCHIVE}")));
    assert!(policy.contains(&format!("/{DEPS_PREFIX}*")));
}
