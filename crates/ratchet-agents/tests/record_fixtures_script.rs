//! `scripts/record-fixtures.sh` against fake `claude` / `codex` CLIs on a private PATH: it plans without running, refuses a
//! missing CLI, names files `<cli>-<version>-real-<behaviour>.jsonl`, scrubs secrets and machine paths, and deletes a recording
//! that still looks like it holds a credential.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/record-fixtures.sh")
}

/// A PATH with only the tools the script needs, plus whatever fake CLIs a test installs.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in [
            "env", "grep", "sed", "head", "id", "mktemp", "timeout", "perl", "rm", "mkdir", "dirname",
            "cat", "tr", "sh", "bash", "printf", "pwd", "mv", "touch",
        ] {
            let found = ["/usr/bin", "/bin", "/usr/local/bin"]
                .iter()
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.exists());
            if let Some(found) = found {
                std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
            }
        }
        Self { dir }
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("fixtures")
    }

    /// Install a fake CLI: `--version` prints `version`, anything else reads stdin and runs `body`.
    fn fake_cli(&self, name: &str, version: &str, body: &str) {
        let path = self.bin().join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{version}'; exit 0; fi\n{body}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new("bash");
        cmd.arg(script())
            .args(args)
            .arg("--out")
            .arg(self.out())
            .env_clear()
            .env("PATH", self.bin())
            .env("HOME", self.dir.path().join("home"))
            .env("USER", "someone-private");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.out())
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A Claude-like CLI that prints where it ran, its home and its key (all things that must not reach a fixture).
const CHATTY: &str = r#"cat >/dev/null
echo "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-1\",\"cwd\":\"$(pwd)\",\"home\":\"$HOME\",\"key\":\"$ANTHROPIC_API_KEY\",\"args\":\"$*\"}"
echo '{"type":"result","subtype":"success","is_error":false,"result":"pong","session_id":"sess-1"}'"#;

#[test]
fn help_lists_the_options_and_the_exit_codes() {
    let sb = Sandbox::new();
    let out = sb.run(&["--help"], &[]);
    assert!(out.status.success());
    let help = text(&out.stdout);
    for needle in [
        "--cli",
        "--behaviour",
        "--yes",
        "--json",
        "EXIT CODES",
        "EXAMPLES",
        "SECRETS",
    ] {
        assert!(help.contains(needle), "{needle}");
    }
}

#[test]
fn bad_arguments_exit_2_and_say_what_to_use() {
    let sb = Sandbox::new();
    for args in [
        &["--cli", "gemini"][..],
        &["--behaviour", "dancing"],
        &["--nope"],
    ] {
        let out = sb.run(args, &[]);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("record-fixtures:"), "{args:?}");
    }
}

#[test]
fn a_named_cli_that_is_not_installed_is_refused_and_nothing_is_written() {
    let sb = Sandbox::new();
    let out = sb.run(&["--cli", "claude", "--yes"], &[]);
    assert_eq!(out.status.code(), Some(3));
    let err = text(&out.stderr);
    assert!(err.contains("claude is not installed"), "{err}");
    assert!(sb.files().is_empty());
}

#[test]
fn all_skips_missing_clis_and_reports_them() {
    let sb = Sandbox::new();
    let out = sb.run(&["--json"], &[]);
    assert!(out.status.success());
    let results: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = results.as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r["status"] == "skipped"));
}

#[test]
fn without_yes_it_plans_and_neither_runs_the_cli_nor_writes() {
    let sb = Sandbox::new();
    sb.fake_cli("claude", "2.9.1 (Claude Code)", "touch \"$0.ran\"");
    let out = sb.run(&["--cli", "claude", "--json"], &[]);
    assert!(out.status.success());
    let results: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = results.as_array().unwrap();
    assert_eq!(results.len(), 4);
    assert!(results.iter().all(|r| r["status"] == "planned"));
    assert_eq!(results[0]["file"], "claude-2.9.1-real-success.jsonl");
    assert_eq!(results[0]["version"], "2.9.1");
    assert!(sb.files().is_empty());
    assert!(!sb.bin().join("claude.ran").exists(), "the CLI was not run");
}

#[test]
fn it_records_versioned_real_fixtures_and_scrubs_secrets_and_paths() {
    let sb = Sandbox::new();
    sb.fake_cli("claude", "claude 2.9.1 (Claude Code)", CHATTY);
    let secret = "test-secret-value-0123456789";
    let out = sb.run(
        &[
            "--cli",
            "claude",
            "--yes",
            "--behaviour",
            "success,deltas,resume,auth-expired",
        ],
        &[("ANTHROPIC_API_KEY", secret)],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        sb.files(),
        [
            "claude-2.9.1-real-auth-expired.jsonl",
            "claude-2.9.1-real-deltas.jsonl",
            "claude-2.9.1-real-resume.jsonl",
            "claude-2.9.1-real-success.jsonl",
        ]
    );
    for file in sb.files() {
        let body = std::fs::read_to_string(sb.out().join(&file)).unwrap();
        assert!(body.contains("\"session_id\":\"sess-1\""), "{file}");
        assert!(!body.contains(secret), "{file}: the key was redacted");
        assert!(!body.contains("someone-private"), "{file}");
        assert!(
            !body.contains(sb.dir.path().to_str().unwrap()),
            "{file}: machine paths were replaced"
        );
        assert!(
            body.contains("/work/recording"),
            "{file}: the working directory is a placeholder"
        );
        // `auth-expired` runs with an empty home inside the scratch directory instead of the real one
        assert!(
            file.contains("auth-expired") || body.contains("/home/recorder"),
            "{file}: the home directory is a placeholder"
        );
    }
    // the second turn of `resume` continues the first one's session
    let resume = std::fs::read_to_string(sb.out().join("claude-2.9.1-real-resume.jsonl")).unwrap();
    assert!(resume.contains("--resume sess-1"), "{resume}");
    // partial text is asked for only by `deltas`
    let deltas = std::fs::read_to_string(sb.out().join("claude-2.9.1-real-deltas.jsonl")).unwrap();
    assert!(deltas.contains("--include-partial-messages"));
    let success =
        std::fs::read_to_string(sb.out().join("claude-2.9.1-real-success.jsonl")).unwrap();
    assert!(!success.contains("--include-partial-messages"));
    // a rejected key is the recording of `auth-expired`, and no real login is read
    let auth =
        std::fs::read_to_string(sb.out().join("claude-2.9.1-real-auth-expired.jsonl")).unwrap();
    assert!(!auth.contains(secret));
    assert!(auth.contains("invalid-key-for-recording"), "{auth}");
    assert!(sb.out().read_dir().unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")
    }));
}

#[test]
fn a_recording_that_still_holds_a_credential_is_deleted_and_the_run_fails() {
    let sb = Sandbox::new();
    sb.fake_cli(
        "claude",
        "2.9.1",
        "cat >/dev/null\necho '{\"type\":\"system\",\"session_id\":\"s\",\"auth\":\"Bearer abcdef0123456789\"}'",
    );
    let out = sb.run(
        &[
            "--cli",
            "claude",
            "--yes",
            "--behaviour",
            "success",
            "--json",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(4));
    assert!(text(&out.stderr).contains("LEAK"), "{}", text(&out.stderr));
    assert!(sb.files().is_empty(), "{:?}", sb.files());
    let results: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(results[0]["status"], "leak");
}

#[test]
fn a_cli_that_fails_or_prints_nothing_is_a_failed_recording_not_an_empty_fixture() {
    let sb = Sandbox::new();
    sb.fake_cli("claude", "2.9.1", "cat >/dev/null\nexit 1");
    let out = sb.run(&["--cli", "claude", "--yes", "--behaviour", "success"], &[]);
    assert_eq!(out.status.code(), Some(4));
    assert!(sb.files().is_empty());

    sb.fake_cli("codex", "codex-cli 0.200.1", "cat >/dev/null");
    let out = sb.run(&["--cli", "codex", "--yes", "--behaviour", "success"], &[]);
    assert_eq!(out.status.code(), Some(4));
    assert!(text(&out.stderr).contains("printed nothing"));
}

#[test]
fn codex_has_no_partial_text_to_record() {
    let sb = Sandbox::new();
    sb.fake_cli("codex", "codex-cli 0.200.1", "cat >/dev/null\necho '{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}'\necho \"{\\\"args\\\":\\\"$*\\\"}\"");
    let out = sb.run(
        &["--cli", "codex", "--yes", "--behaviour", "deltas,resume"],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(sb.files(), ["codex-0.200.1-real-resume.jsonl"]);
    let resume = std::fs::read_to_string(sb.out().join("codex-0.200.1-real-resume.jsonl")).unwrap();
    assert!(
        resume.contains("exec resume --json --skip-git-repo-check t-1"),
        "{resume}"
    );
}

#[test]
fn the_canary_workflow_is_committed_disabled_and_wired_to_the_recorder() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/harness-canary.yml");
    let yaml = std::fs::read_to_string(path).unwrap();
    // no active trigger but a manual one: every `schedule`, `push` and `pull_request` mention is a comment
    for line in yaml.lines() {
        let code = line.split('#').next().unwrap_or("").trim_end();
        for trigger in ["schedule:", "push:", "pull_request:", "cron:"] {
            assert!(
                !code.contains(trigger),
                "an active trigger would switch the canary on: {line}"
            );
        }
    }
    assert!(yaml.contains("workflow_dispatch:"));
    assert!(
        yaml.contains("DISABLED ON PURPOSE"),
        "the file says why the schedule is off"
    );
    assert!(
        yaml.contains("# schedule:") && yaml.contains("#   - cron:"),
        "the schedule is kept, commented out"
    );
    for step in [
        "scripts/record-fixtures.sh",
        "--test conformance",
        "RATCHET_FIXTURES_DIR",
        "harness-canary",
        "gh issue create",
        "gh issue comment",
    ] {
        assert!(yaml.contains(step), "{step}");
    }
    assert!(
        !yaml.contains("issues/10"),
        "the canary never touches the review issue"
    );
}
