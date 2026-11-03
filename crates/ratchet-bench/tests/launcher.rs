#![cfg(target_os = "linux")]
//! Port of `infra/tests/src/candidate-launcher.test.ts`: the setuid-root launcher the harness uses inside the bench container.
//!
//! The behaviour tests run the real binary code (`ratchet-candidate-launcher-seam`: the same `launcher::run`, with the
//! configuration path taken from the environment) made setuid root in a scratch directory, so they need root (to make it
//! setuid and to switch uids) and `setpriv`: they run in CI containers and on root dev boxes, and say so when they skip. The
//! decisions (configuration parsing, caller check, environment, argument refusal) are also unit tested in
//! `src/launcher.rs` without privileges.

use std::fs;
use std::os::unix::fs::{PermissionsExt, chown};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CALLER: u32 = 65534;
const TARGET: u32 = 65533;

fn have(cmd: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {cmd}")])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

/// A scratch directory every uid can traverse (the launcher runs as unprivileged uids in it).
fn open_dir() -> Option<tempfile::TempDir> {
    for base in [std::env::temp_dir(), PathBuf::from("/tmp")] {
        let Ok(d) = tempfile::Builder::new()
            .prefix("ratchet-launcher-")
            .tempdir_in(&base)
        else {
            continue;
        };
        fs::set_permissions(d.path(), fs::Permissions::from_mode(0o755)).ok()?;
        let probe = Command::new("setpriv")
            .args([
                &format!("--reuid={CALLER}"),
                &format!("--regid={CALLER}"),
                "--clear-groups",
                "test",
                "-x",
            ])
            .arg(d.path())
            .output()
            .ok()?;
        if probe.status.success() {
            return Some(d);
        }
    }
    None
}

/// The tests copy executables and then run them. A fork in another thread between the `open` for writing and the `exec`
/// of the child would leave the copy "text file busy" (`ETXTBSY`), so the tests of this file run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Rig {
    _serial: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    dir: PathBuf,
    bin: PathBuf,
    conf: PathBuf,
    work: PathBuf,
}

fn rig() -> Option<Rig> {
    let guard = serial();
    if !root() || !have("setpriv") {
        eprintln!("skipping: the launcher behaviour tests need root and setpriv");
        return None;
    }
    let Some(d) = open_dir() else {
        eprintln!("skipping: no scratch directory that uid {CALLER} can traverse");
        return None;
    };
    let dir = d.path().to_path_buf();
    let bin = dir.join("cand");
    fs::copy(env!("CARGO_BIN_EXE_ratchet-candidate-launcher-seam"), &bin)
        .expect("copy the launcher");
    chown(&bin, Some(0), Some(0)).expect("chown");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o4755)).expect("setuid");
    let work = dir.join("work");
    fs::create_dir_all(&work).expect("mkdir");
    fs::set_permissions(&work, fs::Permissions::from_mode(0o777)).expect("chmod");
    let conf = dir.join("launcher.conf");
    Some(Rig {
        _serial: guard,
        _dir: d,
        dir,
        bin,
        conf,
        work,
    })
}

impl Rig {
    fn setup(&self, content: &str) {
        fs::write(&self.conf, content).expect("write conf");
        fs::set_permissions(&self.conf, fs::Permissions::from_mode(0o644)).expect("chmod");
        chown(&self.conf, Some(0), Some(0)).expect("chown");
    }

    fn setup_ok(&self) {
        self.setup(&format!("{CALLER} {TARGET} {TARGET}\n"));
    }

    fn as_caller(&self, args: &[&str], extra_env: &[&str], uid: u32) -> Output {
        Command::new("setpriv")
            .args([
                &format!("--reuid={uid}"),
                &format!("--regid={uid}"),
                "--clear-groups",
                "env",
            ])
            .arg(format!(
                "RATCHET_LAUNCHER_TEST_CONF={}",
                self.conf.display()
            ))
            .arg(format!("RATCHET_CANDIDATE_DIR={}", self.work.display()))
            .arg(format!("RATCHET_CANDIDATE_HOME={}", self.work.display()))
            .arg(format!("RATCHET_CANDIDATE_TMP={}", self.work.display()))
            .args(extra_env)
            .arg(&self.bin)
            .args(args)
            .output()
            .expect("setpriv runs")
    }

    fn call(&self, args: &[&str]) -> Output {
        self.as_caller(args, &[], CALLER)
    }
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn runs_a_command_as_the_target_uid_with_no_groups_no_new_privs_a_clean_environment_and_the_checkout_as_cwd()
 {
    let Some(r) = rig() else { return };
    r.setup_ok();
    let out = r.as_caller(
        &[
            "FOO=bar",
            "sh",
            "-c",
            "id -u; id -g; pwd; env | sort; grep -E \"^(NoNewPrivs|Groups)\" /proc/self/status",
        ],
        &["RATCHET_TRACE=secret", "AWS_SECRET_ACCESS_KEY=x", "OTHER=1"],
        CALLER,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..3],
        [
            TARGET.to_string(),
            TARGET.to_string(),
            r.work.display().to_string()
        ]
    );
    assert!(text.contains("FOO=bar") && text.contains("USER=ratchet-bench"));
    assert!(
        regex::Regex::new(r"NoNewPrivs:\s*1")
            .expect("re")
            .is_match(&text),
        "{text}"
    );
    assert!(
        regex::Regex::new(r"(?m)Groups:\s*$")
            .expect("re")
            .is_match(&text),
        "{text}"
    );
    for leak in ["RATCHET_TRACE", "AWS_SECRET", "OTHER=1"] {
        assert!(!text.contains(leak), "{leak}");
    }
}

#[test]
fn refuses_to_pass_the_holdout_or_credentials_as_arguments() {
    let Some(r) = rig() else { return };
    r.setup_ok();
    for a in ["RATCHET_TRACE=/x", "AWS_SECRET_ACCESS_KEY=x"] {
        let out = r.call(&[a, "true"]);
        assert_eq!(code(&out), 78);
        assert!(stderr(&out).contains("refusing to pass"));
    }
}

#[test]
fn only_works_for_the_configured_caller_real_uid_including_root() {
    let Some(r) = rig() else { return };
    r.setup_ok();
    let as_root = Command::new(&r.bin)
        .arg("true")
        .env("RATCHET_LAUNCHER_TEST_CONF", &r.conf)
        .output()
        .expect("runs");
    assert_eq!(code(&as_root), 77);
    assert_eq!(code(&r.as_caller(&["true"], &[], 65000)), 77);
}

#[test]
fn refuses_a_configuration_that_is_group_writable_malformed_missing_or_names_root_or_the_caller_as_the_target()
 {
    let Some(r) = rig() else { return };
    r.setup_ok();
    fs::set_permissions(&r.conf, fs::Permissions::from_mode(0o664)).expect("chmod");
    assert_eq!(code(&r.call(&["true"])), 77);
    for bad in [
        format!("{CALLER} 0 0\n"),
        format!("{CALLER} {CALLER} {CALLER}\n"),
        "nonsense\n".to_owned(),
        format!("{CALLER} {TARGET}\n"),
        format!("{CALLER} {TARGET} {TARGET} extra\n"),
        String::new(),
    ] {
        r.setup(&bad);
        assert_eq!(code(&r.call(&["true"])), 77, "{bad:?}");
    }
    fs::remove_file(&r.conf).expect("rm");
    assert_eq!(code(&r.call(&["true"])), 77);
}

#[test]
fn refuses_a_configuration_that_is_not_owned_by_root() {
    let Some(r) = rig() else { return };
    r.setup_ok();
    chown(&r.conf, Some(CALLER), Some(CALLER)).expect("chown");
    let out = r.call(&["true"]);
    assert_eq!(code(&out), 77);
    assert!(stderr(&out).contains("root-owned"), "{}", stderr(&out));
}

#[test]
fn does_not_follow_a_symlinked_configuration() {
    let Some(r) = rig() else { return };
    r.setup_ok();
    let real = r.dir.join("real.conf");
    fs::rename(&r.conf, &real).expect("rename");
    std::os::unix::fs::symlink(&real, &r.conf).expect("symlink");
    assert_eq!(code(&r.call(&["true"])), 77);
}

#[test]
fn does_not_let_a_setuid_binary_inside_the_image_hand_root_back_to_the_candidate_no_new_privs() {
    let Some(r) = rig() else { return };
    r.setup_ok();
    let suid = r.dir.join("suid-id");
    fs::copy(first_existing(&["/usr/bin/id", "/bin/id"]), &suid).expect("copy id");
    chown(&suid, Some(0), Some(0)).expect("chown");
    fs::set_permissions(&suid, fs::Permissions::from_mode(0o4755)).expect("setuid");
    let out = r.call(&[&suid.to_string_lossy(), "-u"]);
    assert_eq!(stdout(&out).trim(), TARGET.to_string());
}

fn first_existing(paths: &[&str]) -> PathBuf {
    paths
        .iter()
        .map(Path::new)
        .find(|p| p.exists())
        .expect("a path exists")
        .to_path_buf()
}

#[test]
fn keeps_the_candidate_out_of_files_only_the_evaluator_can_read() {
    let Some(r) = rig() else { return };
    r.setup_ok();
    let secret = r.dir.join("holdout.txt");
    fs::write(&secret, "sealed").expect("write");
    chown(&secret, Some(CALLER), Some(CALLER)).expect("chown");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).expect("chmod");
    assert_ne!(code(&r.call(&["cat", &secret.to_string_lossy()])), 0);
    let direct = Command::new("setpriv")
        .args([
            &format!("--reuid={CALLER}"),
            &format!("--regid={CALLER}"),
            "--clear-groups",
            "cat",
        ])
        .arg(&secret)
        .output()
        .expect("runs");
    assert_eq!(stdout(&direct), "sealed");
}

#[test]
fn reports_a_missing_command_and_a_missing_usage_distinctly() {
    let Some(r) = rig() else { return };
    r.setup_ok();
    assert_eq!(code(&r.call(&["FOO=bar"])), 78);
    assert!([126, 127].contains(&code(&r.call(&["no-such-command-xyz"]))));
}

#[test]
fn the_production_binary_has_the_configuration_path_compiled_in() {
    let _serial = serial();
    // The real binary takes no path from its caller: without /ratchet/launcher.conf it refuses, whatever the environment says.
    let out = Command::new(env!("CARGO_BIN_EXE_ratchet-candidate-launcher"))
        .arg("true")
        .env("RATCHET_LAUNCHER_TEST_CONF", "/nonexistent")
        .output()
        .expect("runs");
    if Path::new("/ratchet/launcher.conf").exists() {
        return; // inside a bench container: the real file decides
    }
    assert_eq!(code(&out), 77);
    assert!(
        stderr(&out).contains("only works inside a bench container"),
        "{}",
        stderr(&out)
    );
}
