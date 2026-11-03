#![cfg(target_os = "linux")] // the generated bench-host scripts run under GNU tools; see PARITY.md
//! Port of `packages/bench/src/remote-deps.test.ts`.
//!
//! Dependency snapshots end to end with the REAL generated scripts run by local bash (the same approach as `runners.rs`):
//! `aws s3 cp` is a directory copy, `pnpm`/`npm` are shims that behave like the real ones where it matters (fetch fills a
//! store, install refuses to run unless the network is switched off and every package is in the store), and `node` is a shim
//! that reports `v22.22.0` (the TypeScript tests used the real one and compared against `process.version`).

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use common::*;
use ratchet_bench::ProgressBenchRunner as _;
use ratchet_bench::deps::{
    DepsManifest, PNPM, arch_from_instance_type, deps_key, manifest_key, parse_deps_manifest,
};
use ratchet_bench::testkit::{
    Change, ConfigOptions, DirStore, FixtureRepo, MemoryStore, make_config, make_fixture_repo,
    real_clock,
};
use ratchet_bench::{
    BenchError, BenchProgress, DepsKeyInput, LocalBenchRunner, LocalBenchRunnerOptions,
    RemoteBenchRunner, RemoteBenchRunnerOptions,
};
use ratchet_core::RatchetConfig;
use ratchet_core::ports::{BenchJobPhase, BenchLaunchPurpose, CancellationToken};
use sha2::{Digest, Sha256};

const LOCK: &str = "lockfileVersion: 9\ndep:left-pad\ndep:is-odd\n";
const SETUP: &str = "pnpm install --frozen-lockfile && echo 77 > src/p95.txt";
const NODE: &str = "v22.22.0";

fn files() -> [(&'static str, &'static str); 2] {
    [
        ("pnpm-lock.yaml", LOCK),
        ("package.json", "{\"name\":\"fx\"}\n"),
    ]
}

fn pnpm_shim(log: &Path, flags: &Path) -> String {
    let (log, flags) = (log.display(), flags.display());
    format!(
        r#"
case "${{1:-}}" in
  --version) echo 10.99.0; exit 0;;
  fetch)
    echo "fetch cwd=$PWD store=${{npm_config_store_dir:-}} scripts=${{npm_config_ignore_scripts:-}} aws=${{AWS_ACCESS_KEY_ID:-none}} proxy=${{HTTPS_PROXY:-none}} user=$(id -un) home=$HOME" >> {log}
    [ -e {flags}/slow-fetch ] && sleep 30
    [ -e {flags}/fail-fetch ] && {{ echo "ERR_PNPM_FETCH_FAIL registry unreachable" >&2; exit 1; }}
    [ -f pnpm-lock.yaml ] || {{ echo "no lockfile" >&2; exit 1; }}
    s="$npm_config_store_dir/v10"; mkdir -p "$s/files" "$s/projects"
    while read -r l; do n="${{l#dep:}}"; echo "pkg $n" > "$s/files/$n"; done < <(grep '^dep:' pnpm-lock.yaml)
    ln -s "$PWD" "$s/projects/job";;
  install)
    echo "install cwd=$PWD store=${{npm_config_store_dir:-}} offline=${{npm_config_offline:-unset}} import=${{npm_config_package_import_method:-}} pm_versions=${{npm_config_manage_package_manager_versions:-}}" >> {log}
    [ "${{npm_config_offline:-}}" = true ] || {{ echo "network needed (offline is not set)" >&2; exit 1; }}
    if grep -q newdep package.json 2>/dev/null; then echo "ERR_PNPM_OUTDATED_LOCKFILE specifiers in the lockfile do not match package.json" >&2; exit 1; fi
    s="$npm_config_store_dir/v10"
    ln -s "$PWD" "$s/projects/cand-$$" || {{ echo "EACCES cannot register the project in the store" >&2; exit 1; }}
    mkdir -p node_modules
    while read -r l; do n="${{l#dep:}}"; [ -f "$s/files/$n" ] || {{ echo "ERR_PNPM_NO_OFFLINE_TARBALL $n" >&2; exit 1; }}; cp "$s/files/$n" "node_modules/$n"; done < <(grep '^dep:' pnpm-lock.yaml);;
  *) echo "unexpected pnpm $*" >&2; exit 2;;
esac"#
    )
}

fn npm_shim(log: &Path) -> String {
    let log = log.display();
    format!(
        r#"
case "${{1:-}}" in
  --version) echo 10.9.9; exit 0;;
  ci)
    echo "ci cwd=$PWD cache=${{npm_config_cache:-}} offline=${{npm_config_offline:-unset}} logs=${{npm_config_logs_dir:-}} ignore_scripts=${{npm_config_ignore_scripts:-}}" >> {log}
    c="$npm_config_cache/_cacache/content"
    if [ "${{npm_config_offline:-}}" = true ]; then
      [ -f "$c/left-pad" ] || {{ echo "ENOTCACHED left-pad" >&2; exit 1; }}
      mkdir -p "$npm_config_logs_dir" node_modules && cp "$c/left-pad" node_modules/left-pad
    else
      mkdir -p "$c" "$npm_config_cache/_logs" && echo "pkg left-pad" > "$c/left-pad" && echo log > "$npm_config_cache/_logs/1.log"
      echo x > "$npm_config_cache/_update-notifier-last-checked"
    fi;;
  *) echo "unexpected npm $*" >&2; exit 2;;
esac"#
    )
}

/// The jobs of these tests run once (the shared `job` repeats 3 times).
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

fn cfg_with(f: impl FnOnce(&mut ConfigOptions)) -> RatchetConfig {
    no_guardrails(f)
}

fn host_compression() -> &'static str {
    if Command::new("sh")
        .args(["-c", "command -v zstd"])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        "zstd"
    } else {
        "gzip"
    }
}

fn archive_ext(c: &str) -> &'static str {
    if c == "zstd" { "tar.zst" } else { "tgz" }
}

fn list_archive(path: &Path) -> String {
    let script = if host_compression() == "zstd" {
        "zstd -dc -- \"$1\" | tar -tf -"
    } else {
        "tar -tzf \"$1\""
    };
    let out = Command::new("sh")
        .args(["-c", script, "sh"])
        .arg(path)
        .output()
        .expect("list");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn key_for(lock: &str, arch: &str, image: &str) -> String {
    deps_key(&DepsKeyInput {
        strategy: PNPM,
        lockfiles: [("pnpm-lock.yaml".to_owned(), sha256_hex(lock.as_bytes()))]
            .into_iter()
            .collect(),
        image: image.into(),
        arch: arch.into(),
    })
}

fn key(lock: &str) -> String {
    key_for(lock, "x86_64", "ami-test")
}

/// A rig with the package-manager shims installed (setup = `SETUP` unless overridden).
fn snapshots_rig(fx: &FixtureRepo, tweak: impl FnOnce(&mut RemoteBenchRunnerOptions)) -> Rig {
    let r = rig(fx, &[], |o| {
        o.setup = Some(SETUP.into());
        tweak(o);
    });
    r.compute.add_shim("pnpm", &pnpm_shim(&r.log, &r.flags));
    r.compute.add_shim("npm", &npm_shim(&r.log));
    r.compute.add_shim("node", &format!("echo {NODE}"));
    r
}

fn read_manifest(r: &Rig, key: &str) -> DepsManifest {
    let text = fs::read_to_string(r.bucket_path(&manifest_key(key))).expect("manifest");
    serde_json::from_str(&text).expect("manifest json")
}

fn e(err: &BenchError) -> String {
    err.to_string()
}

fn count(v: &[String], s: &str) -> usize {
    v.iter().filter(|t| *t == s).count()
}

mod first_job_builds_the_snapshot {
    use super::*;

    #[tokio::test]
    async fn launches_the_deps_instance_first_terminates_it_before_the_bench_instance_exists_and_installs_offline()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let progress: Mutex<Vec<String>> = Mutex::default();
        let cb = |p: BenchProgress| {
            progress.lock().expect("lock").push(
                match p {
                    BenchProgress::Deps => "deps",
                    BenchProgress::Checkout => "checkout",
                    BenchProgress::Setup => "setup",
                    BenchProgress::Evaluate => "evaluate",
                    BenchProgress::Bench { .. } => "bench",
                    BenchProgress::Guardrails { .. } => "guardrails",
                }
                .to_owned(),
            );
        };
        let out = r
            .runner
            .run_with_progress(&job(&fx.base), &cfg(), None, None, Some(&cb))
            .await
            .expect("outcome");
        assert_eq!(out.samples, [77.0]); // setup ran `pnpm install` from the store, then wrote the number
        assert!(out.guardrails.is_empty());
        assert_eq!(
            r.timeline(),
            [
                "launch:deps",
                "terminate:i-fake1",
                "launch:bench",
                "terminate:i-fake2"
            ]
        );
        let launched: Vec<_> = r
            .compute
            .launched()
            .into_iter()
            .map(|l| {
                (
                    format!("{:?}", l.purpose),
                    l.optimization_id.to_string(),
                    l.image_ref,
                    l.instance_type,
                )
            })
            .collect();
        assert_eq!(
            launched,
            [
                (
                    "Deps".to_owned(),
                    "opt1".to_owned(),
                    "ami-test".to_owned(),
                    "c7i.2xlarge".to_owned()
                ),
                (
                    "Bench".to_owned(),
                    "opt1".to_owned(),
                    "ami-test".to_owned(),
                    "c7i.2xlarge".to_owned()
                ),
            ]
        );
        let p = progress.lock().expect("lock").clone();
        assert_eq!(p[0], "deps"); // before checkout, so the UI can show "dependencies"
        assert_eq!(p[1..3], ["checkout", "setup"]);
    }

    #[tokio::test]
    async fn stores_the_archive_under_its_key_and_the_manifest_last_and_the_manifest_matches_what_the_controller_computes()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let k = key(LOCK);
        let text = fs::read_to_string(r.bucket_path(&manifest_key(&k))).expect("manifest");
        let input = DepsKeyInput {
            strategy: PNPM,
            lockfiles: [("pnpm-lock.yaml".to_owned(), sha256_hex(LOCK.as_bytes()))]
                .into_iter()
                .collect(),
            image: "ami-test".into(),
            arch: "x86_64".into(),
        };
        let m = parse_deps_manifest(&text, &input, &k).expect("valid manifest");
        // The builder packs with zstd when the machine has it (CI runners do), else gzip; the shimmed zstd test covers both.
        assert_eq!(m.compression.as_str(), host_compression());
        assert_eq!(
            r.objects(&format!("bench-deps/{k}/")),
            [format!(
                "bench-deps/{k}/{}.{}",
                m.sha256,
                archive_ext(host_compression())
            )]
        );
        assert_eq!(m.node, NODE);
        assert_eq!(m.pm, "10.99.0");
    }

    #[tokio::test]
    async fn the_archive_holds_the_fetched_packages_but_not_the_job_tree_registration() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let k = key(LOCK);
        let archive = r.objects(&format!("bench-deps/{k}/")).remove(0);
        let listing = list_archive(&r.bucket_path(&archive));
        assert!(
            listing.contains("v10/files/left-pad") && listing.contains("v10/files/is-odd"),
            "{listing}"
        );
        assert!(!listing.contains("projects/"), "{listing}");
    }

    #[tokio::test]
    async fn runs_the_fetch_as_the_snapshot_job_intends_only_fetch_scripts_off_no_credentials() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let log = r.shim_log();
        let fetches: Vec<&str> = log.lines().filter(|l| l.starts_with("fetch ")).collect();
        assert_eq!(fetches.len(), 1);
        assert!(fetches[0].contains("scripts=true") && fetches[0].contains("aws=none"));
        assert!(
            regex::Regex::new(r"store=.*/store ")
                .expect("re")
                .is_match(fetches[0]),
            "{}",
            fetches[0]
        );
    }

    #[tokio::test]
    async fn never_stages_or_downloads_the_candidate_for_the_snapshot_job() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let sha = fx.candidate(&[("src/p95.txt", Change::write("55"))]);
        run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        let deps_script = r
            .compute
            .scripts()
            .into_iter()
            .find(|s| s.contains("pnpm fetch"))
            .expect("deps script");
        assert!(deps_script.contains("deps-base.tgz"));
        assert!(!deps_script.contains("code.tgz") && !deps_script.contains(&sha));
        let inputs = r.objects("bench-inputs/");
        assert_eq!(
            inputs
                .iter()
                .filter(|k| k.ends_with("deps-base.tgz"))
                .count(),
            1
        );
        assert_eq!(inputs.iter().filter(|k| k.ends_with("code.tgz")).count(), 1);
    }

    #[tokio::test]
    async fn runs_before_the_bench_instance_is_even_launched_so_before_the_baseline_in_its_own_instance()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let j = job_with(&fx.base, |j| {
            j.phase = BenchJobPhase::Baseline;
            j.candidate_id = None;
            j.repeats = 3;
        });
        run(&r.runner, &j, &cfg(), None).await.expect("outcome");
        assert_eq!(
            r.timeline()[..3],
            ["launch:deps", "terminate:i-fake1", "launch:baseline"]
        );
    }

    #[tokio::test]
    async fn setup_and_the_guardrail_commands_get_the_restored_store_and_the_package_manager_offline_the_harness_does_not()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let c = make_config(&ConfigOptions {
            guardrails: Some(vec!["- run: \"true\"".into()]),
            ..ConfigOptions::default()
        });
        run(&r.runner, &job(&fx.base), &c, None)
            .await
            .expect("outcome");
        let scripts = r.compute.scripts();
        let re = |p: &str| regex::Regex::new(p).expect("re");
        let setup = scripts
            .iter()
            .find(|s| s.contains("pnpm install --frozen-lockfile"))
            .expect("setup");
        assert!(re(r"export npm_config_offline='true'").is_match(setup));
        assert!(re(r"export npm_config_store_dir='[^']*/deps/store'").is_match(setup));
        assert!(re(r"export PNPM_STORE_DIR='[^']*/deps/store'").is_match(setup));
        let guard = scripts
            .iter()
            .find(|s| s.contains("bash -c 'true'"))
            .expect("guardrail");
        assert!(re(r"export npm_config_offline='true'").is_match(guard));
        let bench = scripts
            .iter()
            .find(|s| s.contains("bench.sh"))
            .expect("bench");
        assert!(!bench.contains("npm_config_offline") && !bench.contains("npm_config_store_dir"));
        let log = r.shim_log();
        let install = log
            .lines()
            .find(|l| l.starts_with("install "))
            .expect("install line");
        assert!(
            install.contains("offline=true")
                && install.contains("import=clone-or-copy")
                && install.contains("pm_versions=false"),
            "{install}"
        );
    }
}

mod later_jobs_reuse_the_snapshot {
    use super::*;

    #[tokio::test]
    async fn does_not_launch_a_deps_instance_again_for_the_same_lockfile_in_this_process_or_a_fresh_one_sharing_the_bucket()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        r.compute.clear_timeline();
        let progress: Mutex<Vec<&'static str>> = Mutex::default();
        let cb = |p: BenchProgress| {
            progress
                .lock()
                .expect("lock")
                .push(if matches!(p, BenchProgress::Deps) {
                    "deps"
                } else {
                    "other"
                })
        };
        let out = r
            .runner
            .run_with_progress(&job(&fx.base), &cfg(), None, None, Some(&cb))
            .await
            .expect("outcome");
        assert_eq!(out.samples, [77.0]);
        assert_eq!(r.timeline(), ["launch:bench", "terminate:i-fake3"]);
        assert!(!progress.lock().expect("lock").contains(&"deps"));
        assert!(
            r.store
                .get_text("bench/opt1/cand1/bench.log")
                .expect("log")
                .contains("reusing snapshot")
        );

        // a new runner (a restarted app) finds the manifest in the bucket
        let mut o = RemoteBenchRunnerOptions::new(
            r.compute.clone(),
            fx.host.clone(),
            real_clock(),
            "ami-test",
        );
        o.staging_bucket = Some(BUCKET.into());
        o.staging_store = Some(DirStore::new(r.s3.join(BUCKET)));
        o.isolation = ratchet_bench::Isolation::None;
        o.work_root = Some(fx.path().join("work-again").to_string_lossy().into_owned());
        o.setup = Some(SETUP.into());
        let again = RemoteBenchRunner::new(o);
        r.compute.clear_timeline();
        run(&again, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(count(&r.timeline(), "launch:deps"), 0);
    }

    #[tokio::test]
    async fn jobs_that_need_the_same_snapshot_at_the_same_time_share_one_build() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let sha = fx.candidate(&[("src/p95.txt", Change::write("60"))]);
        let (j1, j2) = (
            job_with(&fx.base, |j| {
                j.candidate_id = Some(ratchet_core::CandidateId::new("c1"))
            }),
            job_with(&sha, |j| {
                j.candidate_id = Some(ratchet_core::CandidateId::new("c2"))
            }),
        );
        let c = cfg();
        let (a, b) = tokio::join!(run(&r.runner, &j1, &c, None), run(&r.runner, &j2, &c, None));
        assert_eq!(a.expect("a").samples, [77.0]);
        assert_eq!(b.expect("b").samples, [77.0]);
        assert_eq!(count(&r.timeline(), "launch:deps"), 1);
        assert_eq!(count(&r.timeline(), "launch:bench"), 2);
    }

    #[tokio::test]
    async fn a_different_lockfile_at_the_base_ref_or_another_image_or_architecture_is_another_snapshot()
     {
        let other_lock = format!("{LOCK}dep:lodash\n");
        let other = make_fixture_repo(&[
            ("pnpm-lock.yaml", &other_lock),
            ("package.json", "{\"name\":\"fx\"}\n"),
        ]);
        let r = snapshots_rig(&other, |_| {});
        run(&r.runner, &job(&other.base), &cfg(), None)
            .await
            .expect("outcome");
        let jsons: Vec<String> = r
            .objects("bench-deps/")
            .into_iter()
            .filter(|k| k.ends_with(".json"))
            .collect();
        assert_eq!(jsons, [manifest_key(&key(&other_lock))]);
        assert_ne!(key(LOCK), key(&other_lock));
        assert_ne!(key_for(LOCK, "arm64", "ami-test"), key(LOCK));
        assert_ne!(key_for(LOCK, "x86_64", "ami-other"), key(LOCK));
        assert_eq!(arch_from_instance_type("c7i.2xlarge"), "x86_64");
    }

    #[tokio::test]
    async fn rebuilds_when_the_manifest_is_for_something_else_or_when_its_archive_has_expired() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let k = key(LOCK);
        let manifest_path = r.bucket_path(&manifest_key(&k));

        fs::write(
            &manifest_path,
            format!("{{\"v\":1,\"key\":\"pnpm-{}\"}}", "0".repeat(40)),
        )
        .expect("write");
        r.compute.clear_timeline();
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(count(&r.timeline(), "launch:deps"), 1);
        assert!(
            r.store
                .get_text("bench/opt1/cand1/bench.log")
                .expect("log")
                .contains("ignoring bench-deps/")
        );

        let m = read_manifest(&r, &k);
        fs::remove_file(r.bucket_path(&m.archive)).expect("rm archive");
        r.compute.clear_timeline();
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(count(&r.timeline(), "launch:deps"), 1);
        assert!(
            r.store
                .get_text("bench/opt1/cand1/bench.log")
                .expect("log")
                .contains("is gone from the bucket")
        );
    }
}

mod instance_discipline {
    use super::*;

    async fn wait_for_fetch(r: &Rig) {
        wait_for(
            || r.compute.scripts().iter().any(|s| s.contains("pnpm fetch")),
            20_000,
        )
        .await;
    }

    #[tokio::test]
    async fn terminates_the_deps_instance_and_launches_no_bench_instance_when_the_fetch_fails() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        r.flag("fail-fetch");
        let err = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("fails");
        let re = regex::Regex::new(r"(?s)building the pnpm dependency snapshot failed \(exit 50\).*package fetch \(pnpm\) exited with status 1").expect("re");
        assert!(re.is_match(&e(&err)), "{err}");
        assert_eq!(r.timeline(), ["launch:deps", "terminate:i-fake1"]);
        assert!(r.objects("bench-deps/").is_empty());
        assert!(
            r.store
                .get_text("bench/opt1/cand1/bench.log")
                .expect("log")
                .contains("agent subnet")
        );
    }

    #[tokio::test]
    async fn failure_is_an_infrastructure_error_not_a_verdict_about_the_candidate() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        r.flag("fail-fetch");
        let err = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("fails");
        assert!(matches!(err, BenchError::Infra(_)));
    }

    #[tokio::test]
    async fn aborting_during_the_snapshot_build_stops_waiting_terminates_the_deps_instance_and_launches_nothing_else()
     {
        let fx = make_fixture_repo(&files());
        let r = Arc::new(snapshots_rig(&fx, |_| {}));
        r.flag("slow-fetch");
        let token = CancellationToken::new();
        let (r2, t2) = (r.clone(), token.clone());
        let j = job(&fx.base);
        let task = tokio::spawn(async move {
            r2.runner
                .run_with_progress(&j, &cfg(), None, Some(&t2), None)
                .await
        });
        wait_for_fetch(&r).await;
        token.cancel();
        assert_eq!(
            task.await.expect("joined").expect_err("aborted"),
            BenchError::Aborted
        );
        assert_eq!(r.timeline(), ["launch:deps", "terminate:i-fake1"]);
    }

    #[tokio::test]
    async fn a_job_whose_build_was_aborted_by_another_job_does_not_fail_with_it_it_builds_itself() {
        let fx = make_fixture_repo(&files());
        let r = Arc::new(snapshots_rig(&fx, |_| {}));
        r.flag("slow-fetch");
        let token = CancellationToken::new();
        let (r1, t1) = (r.clone(), token.clone());
        let j1 = job_with(&fx.base, |j| {
            j.candidate_id = Some(ratchet_core::CandidateId::new("c1"))
        });
        let first = tokio::spawn(async move {
            r1.runner
                .run_with_progress(&j1, &cfg(), None, Some(&t1), None)
                .await
        });
        wait_for_fetch(&r).await;
        let r2 = r.clone();
        let j2 = job_with(&fx.base, |j| {
            j.candidate_id = Some(ratchet_core::CandidateId::new("c2"))
        });
        let second = tokio::spawn(async move {
            r2.runner
                .run_with_progress(&j2, &cfg(), None, None, None)
                .await
        }); // waits for the build in flight
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        r.unflag("slow-fetch"); // the second build is fast
        token.cancel();
        assert_eq!(
            first.await.expect("joined").expect_err("aborted"),
            BenchError::Aborted
        );
        assert_eq!(
            second.await.expect("joined").expect("second").samples,
            [77.0]
        );
        assert_eq!(count(&r.timeline(), "launch:deps"), 2);
    }

    #[tokio::test]
    async fn reports_but_survives_a_deps_instance_that_cannot_be_terminated_the_reaper_is_the_backstop()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        r.compute.set_fail_terminate(true);
        let out = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [77.0]);
        assert!(
            r.warnings()
                .iter()
                .any(|w| w.contains("FAILED to terminate dependency snapshot instance i-fake1")),
            "{:?}",
            r.warnings()
        );
    }

    #[tokio::test]
    async fn turns_a_failed_launch_into_a_clear_error() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        r.compute.fail_launch(
            BenchLaunchPurpose::Deps,
            "depsInstanceProfile is not configured (deps_identity_ref)",
        );
        let err = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("fails");
        assert!(e(&err).contains("cannot launch the dependency snapshot instance: depsInstanceProfile is not configured"), "{err}");
    }
}

mod a_lockfile_change_is_blocked {
    use super::*;

    fn dependency_guardrail(
        out: &ratchet_core::ports::BenchOutcome,
    ) -> &ratchet_core::GuardrailResult {
        assert_eq!(
            (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
            ("dependencies unchanged", false),
            "{:?}",
            out.guardrails
        );
        &out.guardrails[0]
    }

    #[tokio::test]
    async fn blocks_a_candidate_whose_lockfile_differs_before_the_instance_runs_anything() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("baseline builds the snapshot");
        let sha = fx.candidate(&[
            (
                "pnpm-lock.yaml",
                Change::write(&format!("{LOCK}dep:evil-package\n")),
            ),
            (
                "package.json",
                Change::write("{\"name\":\"fx\",\"dependencies\":{\"evil-package\":\"1.0.0\"}}\n"),
            ),
        ]);
        r.compute.clear_timeline();
        r.compute.clear_commands();
        let out = run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.runs.is_empty());
        assert_eq!(out.guardrails.len(), 1);
        let d = dependency_guardrail(&out).detail.clone().expect("detail");
        assert!(
            d.contains("dependency changes are not allowed in v0: the bench has no network")
                && d.contains("pnpm-lock.yaml"),
            "{d}"
        );
        // nothing ran on the instance: no ping, no checkout, no setup, no deps rebuild
        assert!(r.compute.commands().is_empty());
        let t = r.timeline();
        assert!(
            t[0] == "launch:bench" && t[1].starts_with("terminate:") && t.len() == 2,
            "{t:?}"
        );
        assert!(
            r.store
                .get_text("bench/opt1/cand1/bench.log")
                .expect("log")
                .contains("blocked before running anything")
        );
    }

    #[tokio::test]
    async fn blocks_a_candidate_that_only_deletes_the_lockfile_or_swaps_it_for_a_symlink() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let gone = fx.candidate(&[("pnpm-lock.yaml", Change::Delete)]);
        let out_gone = run(&r.runner, &job(&gone), &cfg(), None)
            .await
            .expect("outcome");
        dependency_guardrail(&out_gone);
        let link = fx.candidate(&[("pnpm-lock.yaml", Change::symlink("/etc/hostname"))]);
        let out_link = run(&r.runner, &job(&link), &cfg(), None)
            .await
            .expect("outcome");
        dependency_guardrail(&out_link);
    }

    #[tokio::test]
    async fn blocks_a_candidate_that_adds_or_edits_install_steering_config() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        for (name, body) in [
            (".npmrc", "registry=https://evil.example/\n"),
            ("pnpm-workspace.yaml", "onlyBuiltDependencies: [evil]\n"),
        ] {
            let out = run(
                &r.runner,
                &job(&fx.candidate(&[(name, Change::write(body))])),
                &cfg(),
                None,
            )
            .await
            .expect("outcome");
            assert!(
                dependency_guardrail(&out)
                    .detail
                    .clone()
                    .expect("detail")
                    .contains(name)
            );
        }
    }

    #[tokio::test]
    async fn lets_a_candidate_through_that_changes_everything_except_the_dependencies() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let sha = fx.candidate(&[
            ("src/p95.txt", Change::write("10")),
            (
                "package.json",
                Change::write("{\"name\":\"fx\",\"scripts\":{\"build\":\"true\"}}\n"),
            ),
        ]);
        let out = run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.guardrails.is_empty());
        assert_eq!(out.samples, [77.0]);
    }

    #[tokio::test]
    async fn explains_a_failed_offline_install_package_json_asked_for_something_the_lockfile_does_not_have()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let sha = fx.candidate(&[(
            "package.json",
            Change::write("{\"name\":\"fx\",\"dependencies\":{\"newdep\":\"1.0.0\"}}\n"),
        )]);
        let out = run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.runs.is_empty());
        assert_eq!(out.guardrails.len(), 1);
        assert_eq!(
            (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
            ("setup", false)
        );
        let d = out.guardrails[0].detail.clone().expect("detail");
        assert!(
            d.contains("exited with status 1")
                && d.contains("with no network")
                && d.contains("dependency changes are not allowed in v0"),
            "{d}"
        );
        assert!(
            r.store
                .get_text("bench/opt1/cand1/bench.log")
                .expect("log")
                .contains("ERR_PNPM_OUTDATED_LOCKFILE")
        );
    }

    #[tokio::test]
    async fn a_setup_that_rewrites_the_lockfile_is_caught_lockfiles_are_protected_without_being_listed()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |o| {
            o.setup = Some(format!("{SETUP} && echo dep:sneaky >> pnpm-lock.yaml"))
        });
        let c = make_config(&ConfigOptions {
            guardrails: Some(vec!["- run: \"true\"".into()]),
            ..ConfigOptions::default()
        });
        let out = run(&r.runner, &job(&fx.base), &c, None)
            .await
            .expect("outcome");
        assert!(out.runs.is_empty());
        assert_eq!(out.guardrails[0].name, "protected files intact");
        assert!(
            out.guardrails[0]
                .detail
                .clone()
                .expect("detail")
                .contains("pnpm-lock.yaml")
        );
    }

    #[tokio::test]
    async fn the_user_protecting_nothing_is_still_an_error_the_pinned_lockfile_does_not_stand_in_for_a_protected_bench()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let c = cfg_with(|o| {
            o.protected_globs = Some(vec!["nothing/**".into()]);
            o.run = Some("sh ./score.sh".into());
        });
        let err = run(&r.runner, &job(&fx.base), &c, None)
            .await
            .expect_err("refused");
        assert!(
            e(&err).contains("contains no files matching the protected globs"),
            "{err}"
        );
    }
}

mod when_there_is_nothing_to_snapshot {
    use super::*;

    #[tokio::test]
    async fn without_setup_nothing_is_installed_so_nothing_is_snapshotted_and_nothing_is_blocked() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |o| o.setup = None);
        let sha = fx.candidate(&[(
            "pnpm-lock.yaml",
            Change::write(&format!("{LOCK}dep:other\n")),
        )]);
        let out = run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.guardrails.is_empty());
        assert_eq!(r.timeline(), ["launch:bench", "terminate:i-fake1"]);
        assert!(r.objects("bench-deps/").is_empty());
    }

    #[tokio::test]
    async fn evaluator_deps_none_disables_it_even_with_a_lockfile() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |o| o.setup = Some("echo 66 > src/p95.txt".into()));
        let sha = fx.candidate(&[(
            "pnpm-lock.yaml",
            Change::write(&format!("{LOCK}dep:other\n")),
        )]);
        let out = run(
            &r.runner,
            &job(&sha),
            &cfg_with(|o| o.deps = Some("none".into())),
            None,
        )
        .await
        .expect("outcome");
        assert_eq!(out.samples, [66.0]);
        assert_eq!(r.timeline(), ["launch:bench", "terminate:i-fake1"]);
    }

    #[tokio::test]
    async fn a_repository_without_a_lockfile_runs_setup_as_before() {
        let plain = make_fixture_repo(&[]);
        let r = snapshots_rig(&plain, |o| o.setup = Some("echo 66 > src/p95.txt".into()));
        let out = run(&r.runner, &job(&plain.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [66.0]);
        assert_eq!(r.timeline(), ["launch:bench", "terminate:i-fake1"]);
    }

    #[tokio::test]
    async fn an_explicit_package_manager_without_its_lockfile_at_the_base_ref_is_a_loud_configuration_error()
     {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        let err = run(
            &r.runner,
            &job(&fx.base),
            &cfg_with(|o| o.deps = Some("npm".into())),
            None,
        )
        .await
        .expect_err("refused");
        assert!(
            e(&err).contains("evaluator.deps is \"npm\" but bench.ref has no package-lock.json"),
            "{err}"
        );
        assert!(
            r.timeline().is_empty(),
            "nothing was launched for a misconfiguration"
        );
    }
}

mod integrity_on_the_way_back {
    use super::*;

    #[tokio::test]
    async fn refuses_a_snapshot_whose_bytes_changed_after_the_manifest_was_written() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let m = read_manifest(&r, &key(LOCK));
        fs::write(r.bucket_path(&m.archive), "not the archive").expect("tamper");
        let err = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(
            e(&err).contains("sha256 mismatch for the dependency snapshot"),
            "{err}"
        );
        assert_eq!(r.compute.terminated().len(), r.compute.launched().len());
    }

    #[tokio::test]
    async fn refuses_to_restore_on_an_image_with_another_node_than_the_builder_had() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let p = r.bucket_path(&manifest_key(&key(LOCK)));
        let text = fs::read_to_string(&p).expect("read");
        fs::write(
            &p,
            text.replace(&format!("\"node\":\"{NODE}\""), "\"node\":\"v1.2.3\""),
        )
        .expect("write");
        let err = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(e(&err).contains("built with node v1.2.3"), "{err}");
    }

    #[tokio::test]
    async fn refuses_a_snapshot_that_contains_symlinks() {
        let fx = make_fixture_repo(&files());
        let r = snapshots_rig(&fx, |_| {});
        // a builder that was tricked into leaving a symlink in the store: simulate by editing the archive and the manifest consistently
        run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        let k = key(LOCK);
        let mp = r.bucket_path(&manifest_key(&k));
        let m = read_manifest(&r, &k);
        let work = fx.path().join("evil");
        fs::create_dir_all(work.join("v10/files")).expect("mkdir");
        std::os::unix::fs::symlink("/etc", work.join("v10/files/x")).expect("symlink");
        let evil = fx.path().join("evil.tgz");
        assert!(
            Command::new("tar")
                .args(["-czf"])
                .arg(&evil)
                .arg("-C")
                .arg(&work)
                .arg(".")
                .status()
                .expect("tar")
                .success()
        );
        let bytes = fs::read(&evil).expect("read");
        let sum = sha256_hex(&bytes);
        let archive = format!("bench-deps/{k}/{sum}.tgz");
        fs::create_dir_all(r.bucket_path(&format!("bench-deps/{k}"))).expect("mkdir");
        fs::write(r.bucket_path(&archive), &bytes).expect("write");
        let mut v: serde_json::Value = serde_json::to_value(&m).expect("json");
        v["compression"] = "gzip".into();
        v["archive"] = archive.into();
        v["sha256"] = sum.into();
        v["bytes"] = bytes.len().into();
        fs::write(&mp, v.to_string()).expect("write");
        let err = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(e(&err).contains("contains symlinks"), "{err}");
    }
}

#[tokio::test]
async fn compression_uses_zstd_when_the_image_has_it_shimmed_here_and_the_restore_uses_it_too() {
    let fx = make_fixture_repo(&files());
    let r = snapshots_rig(&fx, |_| {});
    let zlog = fx.path().join("zstd.log");
    r.compute.add_shim(
        "zstd",
        &format!(
            "echo \"zstd $*\" >> {}\nif [ \"$1\" = -dc ]; then shift; [ \"$1\" = -- ] && shift; exec gzip -dc \"$1\"; fi\nout=; while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=\"$2\"; shift; done\ngzip -c > \"$out\"",
            zlog.display()
        ),
    );
    run(&r.runner, &job(&fx.base), &cfg(), None)
        .await
        .expect("outcome");
    let k = key(LOCK);
    let m = read_manifest(&r, &k);
    assert_eq!(m.compression.as_str(), "zstd");
    assert!(m.archive.ends_with(".tar.zst"), "{}", m.archive);
    r.compute.clear_timeline();
    let out = run(&r.runner, &job(&fx.base), &cfg(), None)
        .await
        .expect("outcome");
    assert_eq!(out.samples, [77.0]);
    let log = fs::read_to_string(&zlog).expect("zstd log");
    assert!(
        regex::Regex::new(r"(?s)zstd -T0 -3 -q -o .*\nzstd -dc -- ")
            .expect("re")
            .is_match(&log),
        "{log}"
    );
}

#[tokio::test]
async fn npm_snapshots_the_cache_and_installs_offline_from_it() {
    let npm_fx = make_fixture_repo(&[
        ("package-lock.json", "{\"lockfileVersion\":3}\n"),
        ("package.json", "{\"name\":\"fx\"}\n"),
    ]);
    let r = snapshots_rig(&npm_fx, |o| {
        o.setup = Some("npm ci && echo 77 > src/p95.txt".into())
    });
    let out = run(&r.runner, &job(&npm_fx.base), &cfg(), None)
        .await
        .expect("outcome");
    assert_eq!(out.samples, [77.0]);
    assert_eq!(
        r.timeline()[..3],
        ["launch:deps", "terminate:i-fake1", "launch:bench"]
    );
    let log = r.shim_log();
    assert!(
        regex::Regex::new(r"ci cwd=.*cache=.*/store offline=unset .*ignore_scripts=true")
            .expect("re")
            .is_match(&log),
        "{log}"
    );
    assert!(
        regex::Regex::new(r"ci cwd=.* offline=true logs=.*\.ratchet-deps/npm-logs")
            .expect("re")
            .is_match(&log),
        "{log}"
    );
    let key = r
        .objects("bench-deps/")
        .into_iter()
        .find(|k| k.ends_with(".json"))
        .expect("a manifest");
    assert!(
        regex::Regex::new(r"^bench-deps/npm-[0-9a-f]{40}\.json$")
            .expect("re")
            .is_match(&key),
        "{key}"
    );
    let m: DepsManifest =
        serde_json::from_str(&fs::read_to_string(r.bucket_path(&key)).expect("read"))
            .expect("json");
    let listing = list_archive(&r.bucket_path(&m.archive));
    assert!(listing.contains("_cacache/content/left-pad"));
    assert!(!listing.contains("_logs") && !listing.contains("_update-notifier-last-checked"));
    // and a changed package-lock.json is blocked the same way
    let sha = npm_fx.candidate(&[(
        "package-lock.json",
        Change::write("{\"lockfileVersion\":3,\"x\":1}\n"),
    )]);
    let blocked = run(&r.runner, &job(&sha), &cfg(), None)
        .await
        .expect("outcome");
    assert_eq!(
        (
            blocked.guardrails[0].name.as_str(),
            blocked.guardrails[0].passed
        ),
        ("dependencies unchanged", false)
    );
    assert!(
        blocked.guardrails[0]
            .detail
            .clone()
            .expect("detail")
            .contains("package-lock.json")
    );
}

mod isolation_user_shimmed {
    use super::*;

    const ID: &str = r#"case "$2" in ratchet-bench|ratchet-holdout|ratchet-deps) [ "$1" = -u ] && exit 0;; esac; exec /usr/bin/id "$@""#;

    fn user_snapshots_rig(fx: &FixtureRepo, overrides: &[(&str, &str)]) -> Rig {
        let mut all: Vec<(&str, &str)> = vec![("id", ID), ("pkill", "exit 0")];
        all.extend_from_slice(overrides);
        let r = user_rig(fx, &all, |o| o.setup = Some(SETUP.into()));
        r.compute.add_shim("pnpm", &pnpm_shim(&r.log, &r.flags));
        r.compute.add_shim("node", &format!("echo {NODE}"));
        r
    }

    #[tokio::test]
    async fn builds_as_ratchet_deps_imds_rule_checked_first_and_restores_as_root_before_the_candidate_user_runs_setup()
     {
        let fx = make_fixture_repo(&files());
        let r = user_snapshots_rig(&fx, &[]);
        let out = run(&r.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [77.0]);
        let text = r.shim_log();
        assert!(text.contains(
            "iptables -C OUTPUT -m owner --uid-owner ratchet-deps -d 169.254.169.254 -j REJECT"
        ));
        assert!(regex::Regex::new(r"setpriv --reuid=ratchet-deps --regid=ratchet-deps --clear-groups --no-new-privs -- env -i PATH=").expect("re").is_match(&text));
        assert!(
            regex::Regex::new(r"chown -hR root:root .*/deps")
                .expect("re")
                .is_match(&text)
        ); // the restored store belongs to root
        assert!(
            regex::Regex::new(r"chown -R ratchet-bench: .*\.ratchet-deps")
                .expect("re")
                .is_match(&text)
        ); // the candidate user owns only its scratch dir
        // the fetch (as ratchet-deps) happened before the restore (root) which happened before setup (ratchet-bench)
        let fetch_at = text.find("--reuid=ratchet-deps").expect("fetch");
        let restore_at = text.find("chown -hR root:root").expect("restore");
        let setup_at = text[restore_at..]
            .find("--reuid=ratchet-bench --regid=ratchet-bench --clear-groups --no-new-privs -- env -u AWS_ACCESS_KEY_ID")
            .map(|i| i + restore_at)
            .expect("setup");
        assert!(restore_at > fetch_at && setup_at > restore_at);
    }

    #[tokio::test]
    async fn refuses_to_build_when_the_image_has_no_deps_user_or_the_imds_rule_is_missing() {
        let fx = make_fixture_repo(&files());
        let bad = user_snapshots_rig(&fx, &[("id", "exit 1")]);
        let err = run(&bad.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(
            e(&err).contains("snapshot user ratchet-deps does not exist"),
            "{err}"
        );
        assert_eq!(bad.timeline(), ["launch:deps", "terminate:i-fake1"]);
        let no_rule = user_snapshots_rig(&fx, &[("id", "exit 0"), ("iptables", "exit 1")]);
        let err = run(&no_rule.runner, &job(&fx.base), &cfg(), None)
            .await
            .expect_err("refused");
        assert!(
            e(&err).contains("IMDS is not blocked for ratchet-deps"),
            "{err}"
        );
    }
}

mod dependency_changes_in_the_local_bench_runner {
    use super::*;

    struct Local {
        runner: LocalBenchRunner,
        marker: std::path::PathBuf,
    }

    fn local(fx: &FixtureRepo, setup: Option<&str>) -> Local {
        let n = seq();
        let marker = fx.path().join(format!("local-marker-{n}"));
        let mut o = LocalBenchRunnerOptions::new(
            fx.host.clone(),
            fx.path().join(format!("local-scratch-{n}")),
            real_clock(),
        );
        o.artifacts = Some(MemoryStore::new());
        o.setup = setup.map(|s| format!("touch {}; {s}", marker.display()));
        Local {
            runner: LocalBenchRunner::new(o),
            marker,
        }
    }

    fn local_default(fx: &FixtureRepo) -> Local {
        local(fx, Some("echo 66 > src/p95.txt"))
    }

    #[tokio::test]
    async fn blocks_a_candidate_whose_lockfile_differs_from_bench_ref_without_running_setup() {
        let fx = make_fixture_repo(&files());
        let l = local_default(&fx);
        let sha = fx.candidate(&[(
            "pnpm-lock.yaml",
            Change::write(&format!("{LOCK}dep:evil\n")),
        )]);
        let out = run(&l.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.runs.is_empty());
        assert_eq!(out.guardrails.len(), 1);
        assert_eq!(
            (out.guardrails[0].name.as_str(), out.guardrails[0].passed),
            ("dependencies unchanged", false)
        );
        assert!(out.guardrails[0].detail.clone().expect("detail").contains("dependency changes are not allowed in v0: the bench has no network (pnpm-lock.yaml differs from ratchet/bench)"));
        assert!(!l.marker.exists());
    }

    #[tokio::test]
    async fn gives_the_same_verdict_text_as_the_remote_runner() {
        let fx = make_fixture_repo(&files());
        let l = local_default(&fx);
        let sha = fx.candidate(&[("pnpm-lock.yaml", Change::write("lockfileVersion: 9\n"))]);
        let a = run(&l.runner, &job(&sha), &cfg(), None)
            .await
            .expect("local");
        let r = snapshots_rig(&fx, |_| {});
        let b = run(&r.runner, &job(&sha), &cfg(), None)
            .await
            .expect("remote");
        assert_eq!(a.guardrails, b.guardrails);
    }

    #[tokio::test]
    async fn lets_a_candidate_through_that_leaves_the_lockfile_alone() {
        let fx = make_fixture_repo(&files());
        let l = local_default(&fx);
        let sha = fx.candidate(&[("src/p95.txt", Change::write("1"))]);
        let out = run(&l.runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(out.guardrails.is_empty());
        assert_eq!(out.samples, [66.0]);
        assert!(l.marker.exists());
    }

    #[tokio::test]
    async fn applies_only_when_there_is_a_setup_and_not_with_deps_none() {
        let fx = make_fixture_repo(&files());
        let sha = fx.candidate(&[(
            "pnpm-lock.yaml",
            Change::write(&format!("{LOCK}dep:evil\n")),
        )]);
        let no_setup = run(&local(&fx, None).runner, &job(&sha), &cfg(), None)
            .await
            .expect("outcome");
        assert!(no_setup.guardrails.is_empty());
        let none = run(
            &local_default(&fx).runner,
            &job(&sha),
            &cfg_with(|o| o.deps = Some("none".into())),
            None,
        )
        .await
        .expect("outcome");
        assert!(none.guardrails.is_empty());
        assert_eq!(none.samples, [66.0]);
    }

    #[tokio::test]
    async fn treats_the_lockfile_as_protected_a_setup_that_rewrites_it_is_caught_and_the_file_is_restored()
     {
        let fx = make_fixture_repo(&files());
        let l = local(&fx, Some("echo dep:sneaky >> pnpm-lock.yaml"));
        let c = make_config(&ConfigOptions {
            guardrails: Some(vec!["- run: \"true\"".into()]),
            ..ConfigOptions::default()
        });
        let out = run(&l.runner, &job(&fx.base), &c, None)
            .await
            .expect("outcome");
        assert_eq!(out.guardrails[0].name, "protected files intact");
        assert!(!out.guardrails[0].passed);
        assert!(
            out.guardrails[0]
                .detail
                .clone()
                .expect("detail")
                .contains("pnpm-lock.yaml")
        );
    }

    #[tokio::test]
    async fn an_explicit_strategy_without_its_lockfile_at_bench_ref_is_a_configuration_error_here_too()
     {
        let fx = make_fixture_repo(&files());
        let l = local_default(&fx);
        let err = run(
            &l.runner,
            &job(&fx.base),
            &cfg_with(|o| o.deps = Some("npm".into())),
            None,
        )
        .await
        .expect_err("refused");
        assert!(e(&err).contains("evaluator.deps is \"npm\""), "{err}");
    }

    #[test]
    fn lockfile_changes_is_what_both_runners_use() {
        let h = |v: &str| -> ratchet_bench::FileHashes {
            [("pnpm-lock.yaml".to_owned(), v.to_owned())]
                .into_iter()
                .collect()
        };
        assert_eq!(
            ratchet_bench::lockfile_changes(&h("a"), &h("b")),
            ["pnpm-lock.yaml"]
        );
    }
}
