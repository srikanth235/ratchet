//! Port of `packages/bench/src/node-path.test.ts`.
//!
//! The TypeScript tests ran the real `node` through the shims. The Rust crate needs no JavaScript runtime, so the "node" here
//! is a bash script that prints how it was started: `electron-as-node:<args>` (only when `ELECTRON_RUN_AS_NODE=1`, like the
//! Electron binary) or `plain-node:<args>`; the fake Electron also runs its first argument as a bash script, so the local
//! runner tests can use `node bench/bench.sh`.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use indexmap::IndexMap;
use ratchet_bench::node_path::{NodeRuntime, WithNodeOnPathOptions};
use ratchet_bench::testkit::{
    Change, ConfigOptions, MemoryStore, make_config, make_fixture_repo, real_clock,
};
use ratchet_bench::{
    LocalBenchRunner, LocalBenchRunnerOptions, NodeRuntimeInfo, ensure_node_shim, node_dir_for,
    node_shim_script, resolve_node_on_path, with_node_on_path,
};
use ratchet_core::OptimizationId;
use ratchet_core::ports::{BaselineMetrics, BenchJob, BenchJobPhase, BenchRunner};

struct Rig {
    root: tempfile::TempDir,
    fake_electron: PathBuf,
    plain_node: PathBuf,
}

fn script(path: &Path, body: &str) {
    fs::write(path, body).expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn rig() -> Rig {
    let root = tempfile::tempdir().expect("tmp");
    let fake_electron = root.path().join("Ratchet");
    script(
        &fake_electron,
        "#!/bin/sh\n[ \"$ELECTRON_RUN_AS_NODE\" = 1 ] || { echo \"not run as node\" >&2; exit 97; }\nif [ -f \"$1\" ]; then exec bash \"$@\"; fi\necho \"electron-as-node:$*\"\n",
    );
    let target = root.path().join("plain-node-target");
    script(&target, "#!/bin/sh\necho \"plain-node:$*\"\n");
    let bin = root.path().join("bin");
    fs::create_dir(&bin).expect("mkdir");
    let plain_node = bin.join("node");
    symlink(&target, &plain_node).expect("symlink");
    Rig {
        root,
        fake_electron,
        plain_node,
    }
}

fn electron(r: &Rig) -> NodeRuntimeInfo {
    NodeRuntimeInfo::electron(&r.fake_electron, "33.2.0")
}

fn opts(r: &Rig, runtime: NodeRuntimeInfo) -> WithNodeOnPathOptions {
    WithNodeOnPathOptions {
        runtime: Some(runtime),
        shim_root: Some(r.root.path().to_path_buf()),
        base_path: None,
    }
}

fn env(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// `/bin/sh -c script` with exactly this environment.
fn sh(script: &str, env: &IndexMap<String, String>) -> String {
    let out = Command::new("/bin/sh")
        .args(["-c", script])
        .env_clear()
        .envs(env)
        .output()
        .expect("sh runs");
    assert!(
        out.status.success(),
        "{script}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn first_dir(path: &str) -> PathBuf {
    PathBuf::from(path.split(':').next().expect("a first entry"))
}

mod node_shim_script_and_ensure_node_shim {
    use super::*;

    #[test]
    fn sets_electron_run_as_node_only_for_electron_and_quotes_the_path() {
        assert!(
            node_shim_script("/App's/Ratchet", true)
                .contains("ELECTRON_RUN_AS_NODE=1 exec '/App'\\''s/Ratchet' \"$@\"")
        );
        assert!(!node_shim_script("/usr/bin/node22", false).contains("ELECTRON_RUN_AS_NODE"));
    }

    #[test]
    fn creates_an_executable_node_in_a_private_dir_reuses_it_and_recreates_it_if_it_vanished() {
        let r = rig();
        let dir = ensure_node_shim(&r.fake_electron, true, Some(r.root.path())).expect("shim");
        assert_eq!(dir.parent(), Some(r.root.path()));
        assert_ne!(
            fs::metadata(dir.join("node"))
                .expect("stat")
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert_eq!(
            fs::metadata(&dir).expect("stat").permissions().mode() & 0o077,
            0
        );
        assert_eq!(
            fs::read_to_string(dir.join("node")).expect("read"),
            node_shim_script(&r.fake_electron.to_string_lossy(), true)
        );
        assert_eq!(
            ensure_node_shim(&r.fake_electron, true, Some(r.root.path())).expect("shim"),
            dir
        );
        assert_ne!(
            ensure_node_shim(&r.fake_electron, false, Some(r.root.path())).expect("shim"),
            dir
        );
        fs::remove_dir_all(&dir).expect("rm");
        let again = ensure_node_shim(&r.fake_electron, true, Some(r.root.path())).expect("shim");
        assert!(again.join("node").is_file());
    }
}

mod with_node_on_path_electron {
    use super::*;

    #[test]
    fn prepends_a_shim_so_node_runs_the_electron_binary_as_node_even_with_no_node_on_path() {
        let r = rig();
        let out = with_node_on_path(
            &env(&[("PATH", "/usr/bin:/bin"), ("CI", "1")]),
            &opts(&r, electron(&r)),
        );
        let path = out["PATH"].clone();
        let (first, rest) = path.split_once(':').expect("a prefix");
        assert_eq!(rest, "/usr/bin:/bin");
        assert!(
            fs::read_to_string(Path::new(first).join("node"))
                .expect("shim")
                .contains("ELECTRON_RUN_AS_NODE=1")
        );
        assert_eq!(out["CI"], "1");
        assert!(
            !out.contains_key("ELECTRON_RUN_AS_NODE"),
            "only the shim sets it"
        );
        assert_eq!(sh("node -p \"40 + 2\"", &out), "electron-as-node:-p 40 + 2");
    }

    #[test]
    fn wins_over_a_real_node_that_is_already_on_path_and_handles_a_missing_path() {
        let r = rig();
        let path_with_node = format!(
            "{}:/usr/bin:/bin",
            r.plain_node.parent().expect("dir").display()
        );
        let out = with_node_on_path(&env(&[("PATH", &path_with_node)]), &opts(&r, electron(&r)));
        assert_eq!(sh("node -p \"1 + 1\"", &out), "electron-as-node:-p 1 + 1");
        assert_ne!(sh("command -v node", &out), r.plain_node.to_string_lossy());
        let mut o = opts(&r, electron(&r));
        o.base_path = Some(String::new());
        let bare = with_node_on_path(&env(&[]), &o);
        let shim_dir = sh(
            "p=$(command -v node); echo \"${p%/*}\"",
            &env(&[("PATH", &bare["PATH"])]),
        );
        assert_eq!(bare["PATH"], shim_dir);
        assert!(
            !bare["PATH"].contains(':'),
            "no trailing empty entry (which means cwd)"
        );
    }

    #[test]
    fn does_not_mutate_its_input() {
        let r = rig();
        let input = env(&[("PATH", "/bin")]);
        let out = with_node_on_path(&input, &opts(&r, electron(&r)));
        assert_eq!(input, env(&[("PATH", "/bin")]));
        assert!(out["PATH"].ends_with(":/bin"));
    }
}

mod with_node_on_path_plain_node {
    use super::*;

    fn plain(r: &Rig) -> NodeRuntimeInfo {
        NodeRuntimeInfo::plain(&r.plain_node)
    }

    #[test]
    fn prepends_dirname_of_exec_path_when_path_has_no_node() {
        let r = rig();
        let out = with_node_on_path(&env(&[("PATH", "/usr/bin:/bin")]), &opts(&r, plain(&r)));
        let want = format!(
            "{}:/usr/bin:/bin",
            r.plain_node.parent().expect("dir").display()
        );
        assert_eq!(out["PATH"], want);
        assert_eq!(
            sh("node -p \"6 * 7\"", &env(&[("PATH", &out["PATH"])])),
            "plain-node:-p 6 * 7"
        );
    }

    #[test]
    fn prepends_dirname_when_a_different_node_would_win() {
        let r = rig();
        let other = r.root.path().join("other");
        fs::create_dir(&other).expect("mkdir");
        symlink("/bin/true", other.join("node")).expect("symlink");
        let both = format!(
            "{}:{}",
            other.display(),
            r.plain_node.parent().expect("dir").display()
        );
        assert_eq!(
            resolve_node_on_path(&both),
            Some(fs::canonicalize("/bin/true").expect("real"))
        );
        let out = with_node_on_path(
            &env(&[("PATH", &format!("{}:/usr/bin", other.display()))]),
            &opts(&r, plain(&r)),
        );
        assert!(out["PATH"].starts_with(&format!(
            "{}:",
            r.plain_node.parent().expect("dir").display()
        )));
    }

    #[test]
    fn leaves_path_alone_when_the_first_node_on_path_already_is_this_runtime() {
        let r = rig();
        let p = r
            .plain_node
            .parent()
            .expect("dir")
            .to_string_lossy()
            .into_owned();
        assert_eq!(node_dir_for(&plain(&r), &p, None), None);
        assert_eq!(
            with_node_on_path(&env(&[("PATH", &p)]), &opts(&r, plain(&r)))["PATH"],
            p
        );
    }

    #[test]
    fn uses_a_shim_without_electron_run_as_node_when_the_binary_is_not_called_node() {
        let r = rig();
        let odd = r.root.path().join("bin/node22");
        symlink(r.root.path().join("plain-node-target"), &odd).expect("symlink");
        let mut o = opts(&r, NodeRuntimeInfo::plain(&odd));
        o.shim_root = Some(r.root.path().to_path_buf());
        let out = with_node_on_path(&env(&[("PATH", "/usr/bin:/bin")]), &o);
        let shim = fs::read_to_string(first_dir(&out["PATH"]).join("node")).expect("shim");
        assert!(!shim.contains("ELECTRON_RUN_AS_NODE"));
        assert_eq!(sh("node -p \"1 + 2\"", &out), "plain-node:-p 1 + 2");
    }

    #[test]
    fn is_a_no_op_on_windows_and_works_in_a_path_less_environment() {
        let r = rig();
        let win = NodeRuntimeInfo {
            exec_path: "C:\\node.exe".into(),
            electron: None,
            platform: Some("win32".into()),
        };
        let o = WithNodeOnPathOptions {
            runtime: Some(win),
            ..WithNodeOnPathOptions::default()
        };
        assert_eq!(
            with_node_on_path(&env(&[("PATH", "C:\\x")]), &o)["PATH"],
            "C:\\x"
        );
        let out = with_node_on_path(&env(&[("PATH", "/nonexistent")]), &opts(&r, plain(&r)));
        assert_eq!(sh("node -p x", &out), "plain-node:-p x");
    }
}

mod local_bench_runner_under_simulated_electron {
    use super::*;

    fn job(sha: &str) -> BenchJob {
        BenchJob {
            optimization_id: OptimizationId::new("o"),
            candidate_id: Some(ratchet_core::CandidateId::new("c")),
            sha: sha.into(),
            phase: BenchJobPhase::Bench,
            repeats: 1,
            trace: None,
        }
    }

    fn runner(
        fx: &ratchet_bench::testkit::FixtureRepo,
        runtime: NodeRuntime,
        setup: Option<&str>,
    ) -> LocalBenchRunner {
        let mut o =
            LocalBenchRunnerOptions::new(fx.host.clone(), fx.path().join("scratch"), real_clock());
        o.artifacts = Some(MemoryStore::new());
        o.env = env(&[("PATH", "/usr/bin:/bin")]);
        o.node_runtime = runtime;
        o.setup = setup.map(str::to_owned);
        LocalBenchRunner::new(o)
    }

    #[tokio::test]
    async fn runs_setup_command_guardrails_and_the_evaluator_through_the_shell_with_no_node_on_path()
     {
        let r = rig();
        let fx = make_fixture_repo(&[]);
        let sha = fx.candidate(&[("src/p95.txt", Change::write("60"))]);
        let config = make_config(&ConfigOptions {
            run: Some("node bench/bench.sh".into()),
            ..ConfigOptions::default()
        });
        let baseline = BaselineMetrics {
            metrics: [("peak_rss_mb".to_owned(), 100.0)].into(),
        };
        let runner = runner(
            &fx,
            NodeRuntime::Explicit(electron(&r)),
            Some("node tests/check.sh"),
        );
        let out = runner
            .run(&job(&sha), &config, Some(&baseline), None)
            .await
            .expect("outcome");
        assert_eq!(out.samples, [60.0]);
        assert!(
            out.guardrails.iter().all(|g| g.passed),
            "{:?}",
            out.guardrails
        );
    }

    #[tokio::test]
    async fn node_runtime_disabled_leaves_path_alone_and_then_the_evaluator_cannot_find_node() {
        if ["/usr/bin/node", "/bin/node"]
            .iter()
            .any(|p| Path::new(p).exists())
        {
            return; // this machine has a system node: the premise (no node on that PATH) does not hold
        }
        let fx = make_fixture_repo(&[]);
        let sha = fx.candidate(&[("src/p95.txt", Change::write("61"))]);
        let config = make_config(&ConfigOptions {
            run: Some("node bench/bench.sh".into()),
            guardrails: Some(vec![]),
            ..ConfigOptions::default()
        });
        let out = runner(&fx, NodeRuntime::Disabled, None)
            .run(&job(&sha), &config, None, None)
            .await
            .expect("outcome");
        assert!(out.samples.is_empty());
        assert!(out.guardrails.iter().any(|g| !g.passed));
    }
}
