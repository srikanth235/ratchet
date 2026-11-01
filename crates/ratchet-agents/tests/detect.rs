mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::env_of;
use ratchet_agents::process::KillSignal;
use ratchet_agents::testing::{FakeCall, FakeProcessSpec, FakeSpawner};
use ratchet_agents::{
    DetectFs, DetectOptions, HarnessAuth, HarnessInfo, HarnessKind, Platform, detect_harnesses,
    fallback_path_dirs, local_agent_env, parse_version, resolve_login_path,
};

const HOME: &str = "/home/u";
const CLAUDE_BIN: &str = "/usr/local/bin/claude";
const CODEX_BIN: &str = "/usr/local/bin/codex";
const GROK_BIN: &str = "/usr/local/bin/grok";

#[derive(Default)]
struct FakeFs {
    executables: Vec<String>,
    existing: Vec<String>,
    dirs: HashMap<String, Vec<String>>,
}

impl FakeFs {
    fn new(executables: &[&str], existing: &[&str]) -> Self {
        Self {
            executables: executables.iter().map(|s| s.to_string()).collect(),
            existing: existing.iter().map(|s| s.to_string()).collect(),
            dirs: HashMap::new(),
        }
    }
    fn with_dirs(mut self, dirs: &[(&str, &[&str])]) -> Self {
        self.dirs = dirs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect();
        self
    }
}

impl DetectFs for FakeFs {
    fn is_executable(&self, p: &str) -> bool {
        self.executables.iter().any(|e| e == p)
    }
    fn exists(&self, p: &str) -> bool {
        self.is_executable(p) || self.existing.iter().any(|e| e == p) || self.dirs.contains_key(p)
    }
    fn list_dir(&self, p: &str) -> Vec<String> {
        self.dirs.get(p).cloned().unwrap_or_default()
    }
}

type Rule = Box<dyn Fn(&FakeCall) -> Option<FakeProcessSpec> + Send + Sync>;

fn rule(f: impl Fn(&FakeCall) -> Option<FakeProcessSpec> + Send + Sync + 'static) -> Rule {
    Box::new(f)
}

fn args_is(call: &FakeCall, args: &[&str]) -> bool {
    call.args.len() == args.len() && call.args.iter().zip(args).all(|(a, b)| a == b)
}

/// Spawner whose behaviour comes from the first rule that answers; anything unmatched exits 127 (command not found).
fn spawner_of(rules: Vec<Rule>) -> FakeSpawner {
    FakeSpawner::with_plan(move |call, _| {
        for r in &rules {
            if let Some(spec) = r(call) {
                return spec;
            }
        }
        FakeProcessSpec::exit(127).with_stderr("not found")
    })
}

fn shell_rule(path: &'static str, noise: &'static str) -> Rule {
    rule(move |c| {
        (c.command.ends_with("sh") && c.args.first().map(String::as_str) == Some("-lc")).then(
            || FakeProcessSpec::stdout(format!("{noise}__RATCHET_PATH__{path}__RATCHET_PATH__")),
        )
    })
}

fn version_rule(binary: Option<&'static str>, output: &'static str) -> Rule {
    rule(move |c| {
        (args_is(c, &["--version"]) && binary.is_none_or(|b| c.command == b))
            .then(|| FakeProcessSpec::stdout(output))
    })
}

fn opts(fake: &FakeSpawner, over: impl FnOnce(&mut DetectOptions)) -> DetectOptions {
    let mut o = DetectOptions {
        spawner: Some(Arc::new(fake.clone())),
        platform: Some(Platform::Linux),
        home: Some(HOME.into()),
        shell: Some("/bin/zsh".into()),
        timeout: Some(Duration::from_millis(200)),
        shell_timeout: Some(Duration::from_millis(200)),
        env: Some(env_of(&[("PATH", "/usr/bin")])),
        fs: Some(Arc::new(FakeFs::default())),
        ..DetectOptions::default()
    };
    over(&mut o);
    o
}

fn fs(f: FakeFs) -> Option<Arc<dyn DetectFs>> {
    Some(Arc::new(f))
}

#[test]
fn parse_version_finds_a_semver_in_the_first_line() {
    assert_eq!(
        parse_version("2.1.4 (Claude Code)\n").as_deref(),
        Some("2.1.4")
    );
    assert_eq!(
        parse_version("codex-cli 0.46.0\n").as_deref(),
        Some("0.46.0")
    );
    assert_eq!(
        parse_version("v1.2.3-beta.1 extra").as_deref(),
        Some("1.2.3-beta.1")
    );
}

#[test]
fn parse_version_falls_back_to_the_first_line_and_to_none_when_empty() {
    assert_eq!(
        parse_version("weird build\n").as_deref(),
        Some("weird build")
    );
    assert_eq!(parse_version(""), None);
}

#[tokio::test]
async fn resolve_login_path_asks_a_login_shell_ignores_rc_noise_and_merges_process_path_and_existing_fallback_dirs()
 {
    let fake = spawner_of(vec![shell_rule(
        "/opt/nvm/bin:/usr/bin:/opt/homebrew/bin",
        "Welcome to zsh!\nnvm loaded\n",
    )]);
    let r = resolve_login_path(&opts(&fake, |o| {
        o.fs = fs(FakeFs::new(&[], &["/opt/homebrew/bin", "/usr/local/bin"]));
        o.env = Some(env_of(&[
            ("PATH", "/usr/bin:/proc/path"),
            ("SHELL", "/bin/zsh"),
        ]));
    }))
    .await;
    assert!(r.from_login_shell);
    assert_eq!(
        r.path.split(':').collect::<Vec<_>>(),
        [
            "/opt/nvm/bin",
            "/usr/bin",
            "/opt/homebrew/bin",
            "/proc/path",
            "/usr/local/bin"
        ]
    );
    let call = fake.call(0);
    assert_eq!(call.command, "/bin/zsh");
    assert_eq!(call.args[0], "-lc");
    assert!(call.args[1].contains("$PATH"));
}

#[tokio::test]
async fn resolve_login_path_falls_back_to_process_path_and_common_dirs_when_the_shell_times_out_and_kills_it()
 {
    let fake = hanging_shell();
    let r = resolve_login_path(&opts(&fake, |o| {
        o.fs = fs(FakeFs::new(
            &[],
            &[&format!("{HOME}/.local/bin"), "/opt/homebrew/bin"],
        ));
        o.shell_timeout = Some(Duration::from_millis(30));
        o.env = Some(env_of(&[("PATH", "/usr/bin")]));
    }))
    .await;
    assert!(!r.from_login_shell);
    assert_eq!(
        r.path,
        [
            "/usr/bin",
            "/opt/homebrew/bin",
            &format!("{HOME}/.local/bin")
        ]
        .join(":")
    );
    assert!(fake.call(0).signals.contains(&KillSignal::Term));
}

fn hanging_shell() -> FakeSpawner {
    spawner_of(vec![rule(|c| {
        (c.args.first().map(String::as_str) == Some("-lc")).then(FakeProcessSpec::hang)
    })])
}

#[tokio::test]
async fn resolve_login_path_falls_back_when_the_shell_cannot_be_started_or_prints_no_markers() {
    let broken = spawner_of(vec![rule(|c| {
        (c.args.first().map(String::as_str) == Some("-lc"))
            .then(|| FakeProcessSpec::spawn_error("ENOENT"))
    })]);
    assert!(
        !resolve_login_path(&opts(&broken, |_| {}))
            .await
            .from_login_shell
    );
    let silent = spawner_of(vec![rule(|c| {
        (c.args.first().map(String::as_str) == Some("-lc"))
            .then(|| FakeProcessSpec::stdout("no markers here\n"))
    })]);
    let r = resolve_login_path(&opts(&silent, |o| o.env = Some(env_of(&[("PATH", "/x")])))).await;
    assert_eq!((r.path.as_str(), r.from_login_shell), ("/x", false));
}

#[tokio::test]
async fn resolve_login_path_uses_string_join_for_fish_whose_path_is_a_list() {
    let fake = spawner_of(vec![shell_rule("/a:/b", "")]);
    let r = resolve_login_path(&opts(&fake, |o| {
        o.shell = Some("/opt/homebrew/bin/fish".into())
    }))
    .await;
    assert!(fake.call(0).args[1].contains("string join : $PATH"));
    assert!(r.path.starts_with("/a:/b"));
}

#[tokio::test]
async fn resolve_login_path_does_not_start_a_shell_on_windows_and_defaults_the_shell_by_platform() {
    let fake = spawner_of(vec![]);
    resolve_login_path(&opts(&fake, |o| {
        o.platform = Some(Platform::Win32);
        o.shell = None;
        o.env = Some(env_of(&[("PATH", "C:\\x")]));
    }))
    .await;
    assert_eq!(fake.call_count(), 0);
    let mac = spawner_of(vec![shell_rule("/m", "")]);
    resolve_login_path(&DetectOptions {
        spawner: Some(Arc::new(mac.clone())),
        platform: Some(Platform::Darwin),
        home: Some(HOME.into()),
        env: Some(HashMap::new()),
        fs: fs(FakeFs::default()),
        ..DetectOptions::default()
    })
    .await;
    assert_eq!(mac.call(0).command, "/bin/zsh");
}

#[test]
fn fallback_path_dirs_lists_nvm_and_fnm_versions_newest_first_and_honours_nvm_bin_volta_home_pnpm_home()
 {
    let nvm = format!("{HOME}/.nvm/versions/node");
    let fnm = format!("{HOME}/.fnm/node-versions");
    let fake_fs = FakeFs::new(&[], &["/n/bin", "/v/bin", "/p"]).with_dirs(&[
        (nvm.as_str(), &["v18.9.0", "v22.1.0", "v20.10.0"]),
        (&format!("{nvm}/v22.1.0/bin"), &[]),
        (&format!("{nvm}/v20.10.0/bin"), &[]),
        (fnm.as_str(), &["v22.0.0"]),
        (&format!("{fnm}/v22.0.0/installation/bin"), &[]),
    ]);
    let env = env_of(&[
        ("NVM_BIN", "/n/bin"),
        ("VOLTA_HOME", "/v"),
        ("PNPM_HOME", "/p"),
    ]);
    let dirs = fallback_path_dirs(&fake_fs, &env, HOME);
    assert_eq!(
        dirs,
        vec![
            "/n/bin".to_string(),
            "/v/bin".into(),
            "/p".into(),
            format!("{nvm}/v22.1.0/bin"),
            format!("{nvm}/v20.10.0/bin"),
            format!("{fnm}/v22.0.0/installation/bin"),
        ]
    );
}

// ---------------------------------------------------------------- install and version

fn auth_probe_fails() -> Rule {
    rule(|c| {
        matches!(c.args.first().map(String::as_str), Some("auth" | "login"))
            .then(|| FakeProcessSpec::exit(1).with_stderr("unknown"))
    })
}

#[tokio::test]
async fn reports_not_installed_with_an_install_hint() {
    let fake = spawner_of(vec![shell_rule("/usr/bin", "")]);
    let found = detect_harnesses(&opts(&fake, |_| {})).await;
    let (cc, cx) = (&found[0], &found[1]);
    assert_eq!(
        (
            cc.kind,
            cc.installed,
            cc.version.clone(),
            cc.path.clone(),
            cc.auth
        ),
        (
            HarnessKind::ClaudeCode,
            false,
            None,
            None,
            HarnessAuth::SignedOut
        )
    );
    assert!(cc.hint.contains("npm install -g @anthropic-ai/claude-code"));
    assert_eq!(
        (cx.kind, cx.installed, cx.path.clone()),
        (HarnessKind::Codex, false, None)
    );
    assert!(cx.hint.contains("@openai/codex"));
}

#[tokio::test]
async fn finds_binaries_on_the_login_shell_path_and_runs_them_with_that_path() {
    let fake = spawner_of(vec![
        shell_rule("/usr/local/bin:/usr/bin", ""),
        version_rule(Some(CLAUDE_BIN), "2.1.4 (Claude Code)\n"),
        version_rule(Some(CODEX_BIN), "codex-cli 0.46.0\n"),
        auth_probe_fails(),
    ]);
    let found = detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(FakeFs::new(&[CLAUDE_BIN, CODEX_BIN], &[]));
        o.env = Some(env_of(&[("PATH", "/bin")]));
    }))
    .await;
    assert_eq!(
        (
            found[0].installed,
            found[0].version.as_deref(),
            found[0].path.as_deref()
        ),
        (true, Some("2.1.4"), Some(CLAUDE_BIN))
    );
    assert_eq!(
        (
            found[1].installed,
            found[1].version.as_deref(),
            found[1].path.as_deref()
        ),
        (true, Some("0.46.0"), Some(CODEX_BIN))
    );
    let version_call = fake
        .calls()
        .into_iter()
        .find(|c| {
            c.command == CLAUDE_BIN && c.args.first().map(String::as_str) == Some("--version")
        })
        .unwrap();
    assert_eq!(
        version_call.options.env["PATH"].split(':').next(),
        Some("/usr/local/bin")
    ); // npm shims need node on it
}

#[tokio::test]
async fn finds_a_binary_in_a_fallback_dir_when_the_login_shell_is_unavailable() {
    let bin = format!("{HOME}/.npm-global/bin/codex");
    let bin2 = bin.clone();
    let fake = spawner_of(vec![
        rule(|c| {
            (c.args.first().map(String::as_str) == Some("-lc"))
                .then(|| FakeProcessSpec::spawn_error("ENOENT"))
        }),
        rule(move |c| {
            (c.command == bin2 && args_is(c, &["--version"]))
                .then(|| FakeProcessSpec::stdout("codex-cli 1.0.0"))
        }),
    ]);
    let found = detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(FakeFs::new(&[&bin], &[&format!("{HOME}/.npm-global/bin")]));
        o.kinds = Some(vec![HarnessKind::Codex]);
    }))
    .await;
    assert_eq!(
        (
            found[0].installed,
            found[0].path.as_deref(),
            found[0].version.as_deref()
        ),
        (true, Some(bin.as_str()), Some("1.0.0"))
    );
}

#[tokio::test]
async fn a_binary_whose_version_fails_is_not_installed_with_the_reason_in_the_hint() {
    let claude_only = |o: &mut DetectOptions| {
        o.fs = fs(FakeFs::new(&[CLAUDE_BIN], &[]));
        o.kinds = Some(vec![HarnessKind::ClaudeCode]);
    };
    let exit1 = spawner_of(vec![
        shell_rule("/usr/local/bin", ""),
        rule(|c| {
            args_is(c, &["--version"]).then(|| {
                FakeProcessSpec::exit(1).with_stderr("env: node: No such file or directory")
            })
        }),
    ]);
    let cc = detect_harnesses(&opts(&exit1, claude_only)).await.remove(0);
    assert_eq!(
        (cc.installed, cc.path.as_deref(), cc.version.clone()),
        (false, Some(CLAUDE_BIN), None)
    );
    assert!(
        cc.hint.contains("exited with 1") && cc.hint.contains("node"),
        "{}",
        cc.hint
    );

    let slow = spawner_of(vec![
        shell_rule("/usr/local/bin", ""),
        rule(|c| args_is(c, &["--version"]).then(FakeProcessSpec::hang)),
    ]);
    let timed_out = detect_harnesses(&opts(&slow, |o| {
        claude_only(o);
        o.timeout = Some(Duration::from_millis(30));
    }))
    .await
    .remove(0);
    assert_eq!(
        (timed_out.installed, timed_out.path.as_deref()),
        (false, Some(CLAUDE_BIN))
    );
    assert!(timed_out.hint.contains("timed out"));

    let cannot = spawner_of(vec![
        shell_rule("/usr/local/bin", ""),
        rule(|c| args_is(c, &["--version"]).then(|| FakeProcessSpec::spawn_error("EACCES"))),
    ]);
    assert!(
        detect_harnesses(&opts(&cannot, claude_only)).await[0]
            .hint
            .contains("cannot run it")
    );
}

#[tokio::test]
async fn respects_the_kinds_filter_and_a_precomputed_login_path_without_a_shell_probe() {
    let fake = spawner_of(vec![
        version_rule(None, "codex-cli 0.46.0\n"),
        rule(|c| {
            (c.args.first().map(String::as_str) == Some("login"))
                .then(|| FakeProcessSpec::exit(1).with_stderr("Not logged in"))
        }),
    ]);
    let found = detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(FakeFs::new(&[CODEX_BIN], &[]));
        o.kinds = Some(vec![HarnessKind::Codex]);
        o.login_path = Some("/usr/local/bin".into());
    }))
    .await;
    assert_eq!(
        found.iter().map(|r| r.kind).collect::<Vec<_>>(),
        vec![HarnessKind::Codex]
    );
    assert!(
        !fake
            .calls()
            .iter()
            .any(|c| c.args.first().map(String::as_str) == Some("-lc"))
    );
}

// ---------------------------------------------------------------- Claude Code auth

fn claude_rules(extra: Vec<Rule>) -> FakeSpawner {
    let mut rules = vec![
        shell_rule("/usr/local/bin", ""),
        version_rule(None, "2.1.4 (Claude Code)"),
    ];
    rules.extend(extra);
    spawner_of(rules)
}

async fn claude(
    extra: Vec<Rule>,
    over: impl FnOnce(&mut DetectOptions),
    fake_fs: FakeFs,
) -> HarnessInfo {
    let fake = claude_rules(extra);
    detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(fake_fs);
        o.kinds = Some(vec![HarnessKind::ClaudeCode]);
        over(o);
    }))
    .await
    .remove(0)
}

fn claude_fs() -> FakeFs {
    FakeFs::new(&[CLAUDE_BIN], &[])
}

fn status(json: serde_json::Value, exit_code: i32) -> Rule {
    let text = json.to_string();
    rule(move |c| {
        (c.command == CLAUDE_BIN && args_is(c, &["auth", "status"]))
            .then(|| FakeProcessSpec::stdout(text.clone()).with_exit(exit_code))
    })
}

#[tokio::test]
async fn claude_signed_in_when_auth_status_says_logged_in() {
    let r = claude(vec![status(serde_json::json!({"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"}), 0)], |_| {}, claude_fs()).await;
    assert_eq!(r.auth, HarnessAuth::SignedIn);
    assert!(r.hint.contains("your own login"));
}

#[tokio::test]
async fn claude_api_key_when_the_status_reports_an_api_key_login() {
    let r = claude(
        vec![status(
            serde_json::json!({"loggedIn": true, "authMethod": "api_key"}),
            0,
        )],
        |_| {},
        claude_fs(),
    )
    .await;
    assert_eq!(r.auth, HarnessAuth::ApiKey);
}

#[tokio::test]
async fn claude_signed_out_when_the_cli_says_so_unless_a_key_is_in_the_environment() {
    let out = claude(
        vec![status(serde_json::json!({"loggedIn": false}), 1)],
        |_| {},
        claude_fs(),
    )
    .await;
    assert_eq!(out.auth, HarnessAuth::SignedOut);
    assert!(
        out.hint.contains("sign in") && out.hint.contains("ANTHROPIC_API_KEY"),
        "{}",
        out.hint
    );
    let with_key = claude(
        vec![status(serde_json::json!({"loggedIn": false}), 1)],
        |o| {
            o.env = Some(env_of(&[
                ("PATH", "/usr/bin"),
                ("ANTHROPIC_API_KEY", "sk-ant-secret-value"),
            ]))
        },
        claude_fs(),
    )
    .await;
    assert_eq!(with_key.auth, HarnessAuth::ApiKey);
}

#[tokio::test]
async fn claude_asks_the_cli_about_the_stored_login_only_key_variables_are_removed_from_the_status_probe()
 {
    let fake = claude_rules(vec![status(serde_json::json!({"loggedIn": true}), 0)]);
    detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(claude_fs());
        o.kinds = Some(vec![HarnessKind::ClaudeCode]);
        o.env = Some(env_of(&[
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_API_KEY", "sk-ant-secret-value"),
            ("HOME", HOME),
        ]));
    }))
    .await;
    let call = fake
        .calls()
        .into_iter()
        .find(|c| args_is(c, &["auth", "status"]))
        .unwrap();
    assert!(!call.options.env.contains_key("ANTHROPIC_API_KEY"));
    assert_eq!(call.options.env["HOME"], HOME);
}

#[tokio::test]
async fn claude_falls_back_to_the_credentials_file_when_the_status_command_is_unavailable() {
    let creds = || {
        FakeFs::new(
            &[CLAUDE_BIN],
            &[&format!("{HOME}/.claude/.credentials.json")],
        )
    };
    let unsupported = rule(|c| {
        args_is(c, &["auth", "status"])
            .then(|| FakeProcessSpec::exit(1).with_stderr("error: unknown command 'auth'"))
    });
    assert_eq!(
        claude(vec![unsupported], |_| {}, creds()).await.auth,
        HarnessAuth::SignedIn
    );
    let garbage = rule(|c| {
        args_is(c, &["auth", "status"]).then(|| FakeProcessSpec::stdout("Logged in as someone"))
    });
    assert_eq!(
        claude(vec![garbage], |_| {}, creds()).await.auth,
        HarnessAuth::SignedIn
    );
    let hang = rule(|c| args_is(c, &["auth", "status"]).then(FakeProcessSpec::hang));
    assert_eq!(
        claude(
            vec![hang],
            |o| o.timeout = Some(Duration::from_millis(40)),
            creds()
        )
        .await
        .auth,
        HarnessAuth::SignedIn
    );
}

#[tokio::test]
async fn claude_honours_claude_config_dir_for_the_credentials_file() {
    let fake_fs = || FakeFs::new(&[CLAUDE_BIN], &["/cfg/.credentials.json"]);
    let unsupported =
        || rule(|c| args_is(c, &["auth", "status"]).then(|| FakeProcessSpec::exit(1)));
    let with_dir = claude(
        vec![unsupported()],
        |o| {
            o.env = Some(env_of(&[
                ("PATH", "/usr/bin"),
                ("CLAUDE_CONFIG_DIR", "/cfg"),
            ]))
        },
        fake_fs(),
    )
    .await;
    assert_eq!(with_dir.auth, HarnessAuth::SignedIn);
    assert_eq!(
        claude(vec![unsupported()], |_| {}, fake_fs()).await.auth,
        HarnessAuth::SignedOut
    );
}

fn unsupported_status() -> Rule {
    rule(|c| args_is(c, &["auth", "status"]).then(|| FakeProcessSpec::exit(1)))
}

fn keychain(exit_code: i32) -> Rule {
    rule(move |c| {
        (c.command == "/usr/bin/security").then(|| {
            FakeProcessSpec::stdout(
                "keychain: \"/Users/u/Library/Keychains/login.keychain-db\"\nattributes:\n",
            )
            .with_exit(exit_code)
        })
    })
}

#[tokio::test]
async fn macos_keychain_item_present_is_signed_in_and_the_secret_is_never_requested() {
    let fake = claude_rules(vec![unsupported_status(), keychain(0)]);
    let found = detect_harnesses(&opts(&fake, |o| {
        o.platform = Some(Platform::Darwin);
        o.fs = fs(claude_fs());
        o.kinds = Some(vec![HarnessKind::ClaudeCode]);
    }))
    .await;
    assert_eq!(found[0].auth, HarnessAuth::SignedIn);
    let sec = fake
        .calls()
        .into_iter()
        .find(|c| c.command == "/usr/bin/security")
        .unwrap();
    assert_eq!(
        sec.args,
        ["find-generic-password", "-s", "Claude Code-credentials"]
    );
    assert!(!sec.args.contains(&"-w".to_string()) && !sec.args.contains(&"-g".to_string()));
}

#[tokio::test]
async fn macos_keychain_item_missing_falls_through_to_the_api_key_then_signed_out() {
    let mk = |env: Vec<(&'static str, &'static str)>| {
        let mut pairs = vec![("PATH", "/usr/bin")];
        pairs.extend(env);
        claude(
            vec![unsupported_status(), keychain(44)],
            move |o| {
                o.platform = Some(Platform::Darwin);
                o.env = Some(env_of(&pairs));
            },
            claude_fs(),
        )
    };
    assert_eq!(
        mk(vec![("ANTHROPIC_API_KEY", "sk-ant-x")]).await.auth,
        HarnessAuth::ApiKey
    );
    assert_eq!(mk(vec![]).await.auth, HarnessAuth::SignedOut);
}

#[tokio::test]
async fn macos_keychain_is_not_probed_on_linux() {
    let fake = claude_rules(vec![unsupported_status(), keychain(0)]);
    let found = detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(claude_fs());
        o.kinds = Some(vec![HarnessKind::ClaudeCode]);
    }))
    .await;
    assert_eq!(found[0].auth, HarnessAuth::SignedOut);
    assert!(
        !fake
            .calls()
            .iter()
            .any(|c| c.command == "/usr/bin/security")
    );
}

#[tokio::test]
async fn claude_never_puts_a_key_value_in_the_result() {
    let r = claude(
        vec![status(serde_json::json!({"loggedIn": false}), 1)],
        |o| {
            o.env = Some(env_of(&[
                ("PATH", "/usr/bin"),
                ("ANTHROPIC_API_KEY", "sk-ant-TOPSECRET"),
            ]))
        },
        claude_fs(),
    )
    .await;
    assert!(!serde_json::to_string(&r).unwrap().contains("TOPSECRET"));
}

// ---------------------------------------------------------------- Codex auth

async fn codex(
    extra: Vec<Rule>,
    over: impl FnOnce(&mut DetectOptions),
    fake_fs: FakeFs,
) -> HarnessInfo {
    let mut rules = vec![
        shell_rule("/usr/local/bin", ""),
        version_rule(None, "codex-cli 0.46.0"),
    ];
    rules.extend(extra);
    let fake = spawner_of(rules);
    detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(fake_fs);
        o.kinds = Some(vec![HarnessKind::Codex]);
        over(o);
    }))
    .await
    .remove(0)
}

fn codex_fs() -> FakeFs {
    FakeFs::new(&[CODEX_BIN], &[])
}

fn login_status(spec: FakeProcessSpec) -> Rule {
    rule(move |c| args_is(c, &["login", "status"]).then(|| spec.clone()))
}

#[tokio::test]
async fn codex_reads_login_status_from_stderr_chatgpt_is_signed_in() {
    let r = codex(
        vec![login_status(
            FakeProcessSpec::default().with_stderr("Logged in using ChatGPT\n"),
        )],
        |_| {},
        codex_fs(),
    )
    .await;
    assert_eq!(r.auth, HarnessAuth::SignedIn);
}

#[tokio::test]
async fn codex_a_stored_api_key_is_api_key_and_its_masked_form_never_reaches_the_result() {
    let spec = FakeProcessSpec::default().with_stderr("API key configured - sk-testt***t1234\nAuthentication is checked when you send a request.\n");
    let r = codex(vec![login_status(spec)], |_| {}, codex_fs()).await;
    assert_eq!(r.auth, HarnessAuth::ApiKey);
    let json = serde_json::to_string(&r).unwrap();
    assert!(!json.contains("sk-") && !json.contains("t1234"), "{json}");
}

#[tokio::test]
async fn codex_not_logged_in_is_signed_out_or_api_key_when_a_key_variable_is_set() {
    let out = || login_status(FakeProcessSpec::exit(1).with_stderr("Not logged in\n"));
    let r = codex(vec![out()], |_| {}, codex_fs()).await;
    assert_eq!(r.auth, HarnessAuth::SignedOut);
    assert!(r.hint.contains("codex login"));
    for key in ["OPENAI_API_KEY", "CODEX_API_KEY"] {
        let with = codex(
            vec![out()],
            move |o| o.env = Some(env_of(&[("PATH", "/usr/bin"), (key, "sk-x")])),
            codex_fs(),
        )
        .await;
        assert_eq!(with.auth, HarnessAuth::ApiKey, "{key}");
    }
}

#[tokio::test]
async fn codex_exit_zero_with_unknown_wording_counts_as_signed_in() {
    assert_eq!(
        codex(
            vec![login_status(FakeProcessSpec::stdout("ok"))],
            |_| {},
            codex_fs()
        )
        .await
        .auth,
        HarnessAuth::SignedIn
    );
}

#[tokio::test]
async fn codex_removes_key_variables_from_the_status_probe() {
    let fake = spawner_of(vec![
        shell_rule("/usr/local/bin", ""),
        version_rule(None, "codex-cli 1.0.0"),
        login_status(FakeProcessSpec::exit(1).with_stderr("Not logged in")),
    ]);
    detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(codex_fs());
        o.kinds = Some(vec![HarnessKind::Codex]);
        o.env = Some(env_of(&[
            ("PATH", "/usr/bin"),
            ("OPENAI_API_KEY", "sk-x"),
            ("CODEX_API_KEY", "sk-y"),
        ]));
    }))
    .await;
    let env = fake
        .calls()
        .into_iter()
        .find(|c| args_is(c, &["login", "status"]))
        .unwrap()
        .options
        .env;
    assert!(!env.contains_key("OPENAI_API_KEY") && !env.contains_key("CODEX_API_KEY"));
}

#[tokio::test]
async fn codex_falls_back_to_auth_json_existence_only_and_codex_home_when_the_status_command_fails_or_times_out()
 {
    let broken = || login_status(FakeProcessSpec::exit(2).with_stderr("unrecognized subcommand"));
    let fs_home = || FakeFs::new(&[CODEX_BIN], &[&format!("{HOME}/.codex/auth.json")]);
    assert_eq!(
        codex(vec![broken()], |_| {}, fs_home()).await.auth,
        HarnessAuth::SignedIn
    );
    let hang = login_status(FakeProcessSpec::hang());
    assert_eq!(
        codex(
            vec![hang],
            |o| o.timeout = Some(Duration::from_millis(40)),
            fs_home()
        )
        .await
        .auth,
        HarnessAuth::SignedIn
    );
    let custom = || FakeFs::new(&[CODEX_BIN], &["/elsewhere/auth.json"]);
    let with_home = codex(
        vec![broken()],
        |o| {
            o.env = Some(env_of(&[
                ("PATH", "/usr/bin"),
                ("CODEX_HOME", "/elsewhere"),
            ]))
        },
        custom(),
    )
    .await;
    assert_eq!(with_home.auth, HarnessAuth::SignedIn);
    assert_eq!(
        codex(vec![broken()], |_| {}, custom()).await.auth,
        HarnessAuth::SignedOut
    );
}

#[tokio::test]
async fn codex_no_status_no_file_an_api_key_variable_is_api_key_nothing_is_signed_out() {
    let broken = || login_status(FakeProcessSpec::spawn_error("EACCES"));
    let keyed = codex(
        vec![broken()],
        |o| o.env = Some(env_of(&[("PATH", "/usr/bin"), ("OPENAI_API_KEY", "sk-z")])),
        codex_fs(),
    )
    .await;
    assert_eq!(keyed.auth, HarnessAuth::ApiKey);
    assert_eq!(
        codex(vec![broken()], |_| {}, codex_fs()).await.auth,
        HarnessAuth::SignedOut
    );
}

// ---------------------------------------------------------------- grok

async fn grok(over: impl FnOnce(&mut DetectOptions), fake_fs: FakeFs) -> HarnessInfo {
    let fake = spawner_of(vec![
        shell_rule("/usr/local/bin", ""),
        version_rule(None, "grok 1.0.67 (2fe064390d07)"),
    ]);
    detect_harnesses(&opts(&fake, |o| {
        o.fs = fs(fake_fs);
        o.kinds = Some(vec![HarnessKind::Grok]);
        over(o);
    }))
    .await
    .remove(0)
}

#[tokio::test]
async fn grok_is_probed_by_default_after_claude_and_codex_and_not_installed_gets_an_install_hint() {
    let fake = spawner_of(vec![shell_rule("/usr/bin", "")]);
    let found = detect_harnesses(&opts(&fake, |_| {})).await;
    assert_eq!(
        found.iter().map(|h| h.kind).collect::<Vec<_>>(),
        [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Grok
        ]
    );
    let gk = &found[2];
    assert_eq!((gk.installed, gk.path.clone()), (false, None));
    assert!(gk.hint.contains("@xai-official/grok"));
}

#[tokio::test]
async fn grok_the_login_file_in_dot_grok_is_signed_in_and_only_its_existence_is_checked() {
    let r = grok(
        |_| {},
        FakeFs::new(&[GROK_BIN], &[&format!("{HOME}/.grok/auth.json")]),
    )
    .await;
    assert_eq!(
        (r.installed, r.version.as_deref(), r.auth),
        (true, Some("1.0.67"), HarnessAuth::SignedIn)
    );
    assert_eq!(r.path.as_deref(), Some(GROK_BIN));
    assert!(r.hint.contains("auth.json"));
}

#[tokio::test]
async fn grok_without_a_login_file_an_api_key_variable_is_api_key_nothing_is_signed_out() {
    let keyed = grok(
        |o| o.env = Some(env_of(&[("PATH", "/usr/bin"), ("XAI_API_KEY", "xai-z")])),
        FakeFs::new(&[GROK_BIN], &[]),
    )
    .await;
    assert_eq!(keyed.auth, HarnessAuth::ApiKey);
    assert!(!keyed.hint.contains("xai-z"));
    let none = grok(|_| {}, FakeFs::new(&[GROK_BIN], &[])).await;
    assert_eq!(none.auth, HarnessAuth::SignedOut);
    assert!(none.hint.contains("grok login"));
}

// ---------------------------------------------------------------- environment for local agents

fn base_env() -> HashMap<String, String> {
    env_of(&[
        ("HOME", "/home/u"),
        ("USER", "u"),
        ("SHELL", "/bin/zsh"),
        ("PATH", "/old"),
        ("ANTHROPIC_API_KEY", "sk-ant"),
        ("OPENAI_API_KEY", "sk-oai"),
        ("CODEX_API_KEY", "sk-cx"),
        ("XAI_API_KEY", "xai-k"),
        ("GITHUB_TOKEN", "ghp_x"),
        ("GH_TOKEN", "ghp_y"),
        ("AWS_ACCESS_KEY_ID", "AKIA"),
        ("AWS_SECRET_ACCESS_KEY", "s"),
        ("AWS_PROFILE", "work"),
        ("NPM_TOKEN", "n"),
        ("SSH_AUTH_SOCK", "/tmp/agent.sock"),
        ("DB_PASSWORD", "p"),
        ("LANG", "en_US.UTF-8"),
        ("HTTPS_PROXY", "http://proxy:3128"),
    ])
}

#[test]
fn local_agent_env_keeps_the_user_environment_sets_the_login_path_and_strips_credentials() {
    let env = local_agent_env(
        HarnessKind::ClaudeCode,
        HarnessAuth::SignedIn,
        &base_env(),
        "/login/bin:/usr/bin",
    );
    assert_eq!(
        env,
        env_of(&[
            ("HOME", "/home/u"),
            ("USER", "u"),
            ("SHELL", "/bin/zsh"),
            ("PATH", "/login/bin:/usr/bin"),
            ("LANG", "en_US.UTF-8"),
            ("HTTPS_PROXY", "http://proxy:3128")
        ])
    );
}

#[test]
fn local_agent_env_passes_the_model_key_only_for_a_harness_that_reports_api_key_auth_and_only_its_own()
 {
    let base = base_env();
    let claude = local_agent_env(HarnessKind::ClaudeCode, HarnessAuth::ApiKey, &base, "/p");
    assert_eq!(
        claude.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-ant")
    );
    assert!(!claude.contains_key("OPENAI_API_KEY"));
    let codex = local_agent_env(HarnessKind::Codex, HarnessAuth::ApiKey, &base, "/p");
    assert_eq!(
        codex.get("OPENAI_API_KEY").map(String::as_str),
        Some("sk-oai")
    );
    assert_eq!(
        codex.get("CODEX_API_KEY").map(String::as_str),
        Some("sk-cx")
    );
    assert!(!codex.contains_key("ANTHROPIC_API_KEY"));
    assert!(
        !local_agent_env(HarnessKind::Codex, HarnessAuth::SignedIn, &base, "/p")
            .contains_key("OPENAI_API_KEY")
    );
    let grok = local_agent_env(HarnessKind::Grok, HarnessAuth::ApiKey, &base, "/p");
    assert_eq!(grok.get("XAI_API_KEY").map(String::as_str), Some("xai-k"));
    assert!(!grok.contains_key("OPENAI_API_KEY") && !grok.contains_key("ANTHROPIC_API_KEY"));
    assert!(
        !local_agent_env(HarnessKind::Grok, HarnessAuth::SignedIn, &base, "/p")
            .contains_key("XAI_API_KEY")
    );
}

#[test]
fn local_agent_env_api_key_auth_without_the_key_set_passes_nothing() {
    assert_eq!(
        local_agent_env(
            HarnessKind::ClaudeCode,
            HarnessAuth::ApiKey,
            &env_of(&[("HOME", "/h")]),
            "/p"
        ),
        env_of(&[("HOME", "/h"), ("PATH", "/p")])
    );
}
