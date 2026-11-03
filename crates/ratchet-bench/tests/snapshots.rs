//! Reviewable snapshots of the generated scripts that carry the security-relevant steps (user switch, holdout handling, image
//! restore). They equal the TypeScript output byte for byte (`tests/golden.rs` proves that for the same inputs); the
//! snapshots are for reading the scripts in review and for catching an accidental change in a diff.

use ratchet_bench::deps::{Compression, NPM, PNPM};
use ratchet_bench::deps_scripts::{DepsBuildInput, deps_build_script};
use ratchet_bench::image_scripts::{ContainerRuntime, ImageRestore};
use ratchet_bench::remote_scripts::{
    CheckoutScriptInput, DepsRestore, Isolation, Layout, StagedArchive, StepKind, StepScriptInput,
    checkout_script, step_script,
};

fn staged(n: &str) -> StagedArchive {
    StagedArchive::new(
        format!("s3://ratchet-test-artifacts/bench-inputs/opt1/job1/{n}"),
        "a".repeat(64),
        "c".repeat(40),
    )
}

fn layout() -> Layout {
    Layout::new("/var/tmp/ratchet-bench/opt1-1234")
}

fn restore_image() -> ImageRestore {
    let key = format!("img-{}", "f".repeat(40));
    ImageRestore {
        uri: format!(
            "s3://ratchet-test-artifacts/bench-images/{key}/{}.tar.zst",
            "d".repeat(64)
        ),
        sha256: "d".repeat(64),
        compression: Compression::Zstd,
        image_id: format!("sha256:{}", "e".repeat(64)),
        rt: ContainerRuntime::new(format!("ratchet-env:{key}")),
    }
}

fn step(kind: StepKind, name: &str, command: &str) -> StepScriptInput {
    let mut i = StepScriptInput::new(layout(), kind, name, command, 60_000);
    i.env.insert("RATCHET_PHASE".into(), "confirm".into());
    i
}

#[test]
fn checkout_with_user_isolation() {
    let i = CheckoutScriptInput::new(
        layout(),
        staged("code.tgz"),
        staged("bench.tgz"),
        vec!["bench/**".into(), "ratchet.yaml".into()],
    );
    insta::assert_snapshot!(checkout_script(&i).expect("script"));
}

#[test]
fn checkout_with_the_snapshot_and_the_container_image() {
    let mut i = CheckoutScriptInput::new(
        layout(),
        staged("code.tgz"),
        staged("bench.tgz"),
        vec!["bench/**".into()],
    );
    i.pinned = vec!["pnpm-lock.yaml".into(), "pnpm-workspace.yaml".into()];
    i.deps = Some(DepsRestore {
        strategy: PNPM,
        uri: format!(
            "s3://ratchet-test-artifacts/bench-deps/pnpm-{}/{}.tar.zst",
            "f".repeat(40),
            "d".repeat(64)
        ),
        sha256: "d".repeat(64),
        compression: Compression::Zstd,
        node: "v22.22.0".into(),
        pm: "10.28.0".into(),
    });
    i.image = Some(restore_image());
    insta::assert_snapshot!(checkout_script(&i).expect("script"));
}

#[test]
fn confirm_bench_step_on_the_host() {
    let mut i = step(
        StepKind::Bench,
        "bench run 1/3",
        "k6 run bench/k6/search.js",
    );
    i.holdout = Some("s3://sealed/holdout/trace.json".into());
    insta::assert_snapshot!(step_script(&i).expect("script"));
}

#[test]
fn confirm_bench_step_in_the_container() {
    let mut i = step(
        StepKind::Bench,
        "bench run 1/3",
        "k6 run bench/k6/search.js",
    );
    i.holdout = Some("s3://sealed/holdout/trace.json".into());
    i.container = Some(restore_image().rt);
    insta::assert_snapshot!(step_script(&i).expect("script"));
}

#[test]
fn setup_step_in_the_container_with_the_dependency_snapshot_mounted() {
    let mut i = step(StepKind::Setup, "setup", "pnpm install --frozen-lockfile");
    i.container = Some(restore_image().rt);
    i.deps_mounted = true;
    insta::assert_snapshot!(step_script(&i).expect("script"));
}

#[test]
fn dependency_snapshot_job_for_npm() {
    let i = DepsBuildInput {
        layout: Layout::new("/var/tmp/ratchet-bench/deps-opt1-1234"),
        strategy: NPM,
        base: staged("deps-base.tgz"),
        key: format!("npm-{}", "e".repeat(40)),
        lockfiles: [("package-lock.json".to_owned(), "a".repeat(64))]
            .into_iter()
            .collect(),
        image: "ami-1".into(),
        arch: "x86_64".into(),
        bucket: "ratchet-test-artifacts".into(),
        timeout_ms: 600_000,
        isolation: Isolation::User,
        container: None,
    };
    insta::assert_snapshot!(deps_build_script(&i).expect("script"));
}
