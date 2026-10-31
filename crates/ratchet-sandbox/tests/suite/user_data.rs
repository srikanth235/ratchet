//! User-data tests: the `aws.test.ts` cases about the generated boot scripts, plus insta snapshots of every script
//! (their bytes were diffed against the TypeScript output when this was ported).

use std::io::Write;
use std::process::{Command, Stdio};

use ratchet_sandbox::compute::user_data::{
    BENCH_OS_USER, CANDIDATE_EXEC_PATH, CANDIDATE_EXEC_SCRIPT, CANDIDATE_LAUNCHER_PATH,
    CANDIDATE_LAUNCHER_SCRIPT, CANDIDATE_SUDOERS, DEPS_OS_USER, HOLDOUT_OS_USER,
    RunnerNetworkOptions, bench_user_data, deps_user_data, runner_user_data,
};
use ratchet_sandbox::domain::{
    AgentIdentity, AgentKind, BenchLaunchSpec, BenchPurpose, RunnerLaunchSpec, SecretRef,
};

fn runner_spec() -> RunnerLaunchSpec {
    RunnerLaunchSpec {
        optimization_id: "opt1".into(),
        agent_id: "a1".into(),
        kind: AgentKind::ClaudeCode,
        instance_type: "c7i.large".into(),
        image_ref: "ami-runner".into(),
        identity: AgentIdentity {
            optimization_id: "opt1".into(),
            agent_id: "a1".into(),
            role_ref: "r".into(),
            instance_profile_ref: "p".into(),
        },
        secrets: vec![SecretRef {
            name: "ANTHROPIC_API_KEY".into(),
            reference: "/ratchet/opt1/a1/ANTHROPIC_API_KEY".into(),
        }],
        repo_url: "https://github.com/o/r'; rm -rf / #.git".into(),
        branch: "ratchet/opt1/a1".into(),
    }
}

fn two_secrets() -> RunnerLaunchSpec {
    let mut s = runner_spec();
    s.secrets.push(SecretRef {
        name: "GITHUB_TOKEN".into(),
        reference: "/ratchet/opt1/a1/GITHUB_TOKEN".into(),
    });
    s
}

fn bench(purpose: BenchPurpose) -> BenchLaunchSpec {
    BenchLaunchSpec {
        optimization_id: "opt1".into(),
        instance_type: "c7i.2xlarge".into(),
        image_ref: "ami-bench".into(),
        purpose,
    }
}

fn proxy() -> RunnerNetworkOptions {
    RunnerNetworkOptions::new("us-east-1", Some("http://10.42.3.10:3128".into()))
}

/// `bash -n`: the script parses.
fn assert_valid_bash(script: &str) {
    let mut child = Command::new("bash")
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bash");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "bash -n failed: {}\n{script}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- snapshots (byte-identical to the TypeScript output) ----------------------------------------------

#[test]
fn snapshots() {
    insta::assert_snapshot!(
        "runner_no_proxy",
        runner_user_data(&two_secrets(), 2.0, None).unwrap()
    );
    insta::assert_snapshot!(
        "runner_proxy",
        runner_user_data(&two_secrets(), 2.0, Some(&proxy())).unwrap()
    );
    let mut no_secrets = runner_spec();
    no_secrets.secrets.clear();
    insta::assert_snapshot!(
        "runner_no_secrets_half_hour",
        runner_user_data(
            &no_secrets,
            0.5,
            Some(&RunnerNetworkOptions::new("eu-west-1", None))
        )
        .unwrap()
    );
    insta::assert_snapshot!(
        "bench_confirm",
        bench_user_data(&bench(BenchPurpose::Confirm), 2.0).unwrap()
    );
    insta::assert_snapshot!(
        "bench_baseline_3h",
        bench_user_data(&bench(BenchPurpose::Baseline), 3.0).unwrap()
    );
    insta::assert_snapshot!(
        "deps_no_proxy",
        deps_user_data(&bench(BenchPurpose::Deps), 2.0, None).unwrap()
    );
    insta::assert_snapshot!(
        "deps_proxy",
        deps_user_data(&bench(BenchPurpose::Deps), 2.0, Some(&proxy())).unwrap()
    );
    insta::assert_snapshot!(
        "deps_half_hour",
        deps_user_data(&bench(BenchPurpose::Deps), 0.5, None).unwrap()
    );
    insta::assert_snapshot!("candidate_exec_script", CANDIDATE_EXEC_SCRIPT);
    insta::assert_snapshot!("candidate_launcher_script", CANDIDATE_LAUNCHER_SCRIPT);
    insta::assert_snapshot!("candidate_sudoers", CANDIDATE_SUDOERS);
}

// ---- runner ------------------------------------------------------------------------------------------

#[test]
fn runner_user_data_lists_several_secret_refs_in_the_comma_format_the_runner_parses() {
    let ud = runner_user_data(&two_secrets(), 2.0, None).unwrap();
    let line = ud
        .lines()
        .find(|l| l.contains("RATCHET_SECRET_REFS="))
        .unwrap();
    assert!(line.contains("ANTHROPIC_API_KEY=/ratchet/opt1/a1/ANTHROPIC_API_KEY,GITHUB_TOKEN=/ratchet/opt1/a1/GITHUB_TOKEN"), "{line}");
    for (name, reference) in [("A", "/x,B=/y"), ("a-b", "/x")] {
        let mut s = runner_spec();
        s.secrets = vec![SecretRef {
            name: name.into(),
            reference: reference.into(),
        }];
        let e = runner_user_data(&s, 2.0, None).unwrap_err();
        assert!(
            e.to_string().contains("invalid secret ref"),
            "{name}={reference}: {e}"
        );
    }
}

#[test]
fn secret_values_never_appear_only_ssm_refs() {
    let ud = runner_user_data(&two_secrets(), 2.0, None).unwrap();
    assert!(!ud.contains("sk-"));
    assert!(ud.contains("/ratchet/opt1/a1/GITHUB_TOKEN"));
}

#[test]
fn runner_user_data_exports_the_proxy_only_when_one_is_configured() {
    let none = runner_user_data(
        &runner_spec(),
        2.0,
        Some(&RunnerNetworkOptions::new("us-east-1", None)),
    )
    .unwrap();
    assert!(!none.contains("HTTPS_PROXY") && !none.contains("profile.d"));
    let ud = runner_user_data(&runner_spec(), 2.0, Some(&proxy())).unwrap();
    assert!(ud.contains("HTTPS_PROXY='http://10.42.3.10:3128'"));
    assert!(ud.contains("https_proxy='http://10.42.3.10:3128'"));
    // AWS service endpoints and IMDS bypass the proxy
    let no_proxy = ud.lines().find(|l| l.starts_with("NO_PROXY='")).unwrap();
    assert!(
        no_proxy.contains("169.254.169.254") && no_proxy.contains("ssm.us-east-1.amazonaws.com")
    );
    assert!(no_proxy.find("169.254.169.254") < no_proxy.find("ssm.us-east-1.amazonaws.com"));
    assert!(ud.contains("/etc/profile.d/ratchet-proxy.sh"));
    let hostile = RunnerNetworkOptions::new("us-east-1", Some("http://x:1'; curl evil #".into()));
    let e = runner_user_data(&runner_spec(), 2.0, Some(&hostile)).unwrap_err();
    assert!(e.to_string().contains("invalid egress proxy url"), "{e}");
}

#[test]
fn proxy_urls_must_be_plain_ip_and_port() {
    for bad in [
        "https://10.0.0.1:3128",
        "http://proxy.example.com:3128",
        "http://10.0.0.1",
        "http://10.0.0.1:3128/",
        "http://10.0.0.1:3128\n",
        "",
        "http://:3128",
    ] {
        let net = RunnerNetworkOptions::new("us-east-1", Some(bad.into()));
        let r = runner_user_data(&runner_spec(), 2.0, Some(&net));
        if bad.is_empty() {
            assert!(r.is_ok(), "an empty url means no proxy");
        } else {
            assert!(r.is_err(), "{bad:?} must be refused");
        }
    }
}

#[test]
fn every_generated_script_is_valid_bash() {
    assert_valid_bash(&runner_user_data(&two_secrets(), 2.0, Some(&proxy())).unwrap());
    assert_valid_bash(&runner_user_data(&runner_spec(), 2.0, None).unwrap());
}

// ---- dependency snapshots (purpose 'deps') ------------------------------------------------------------

mod deps {
    use super::*;

    fn ud() -> String {
        deps_user_data(&bench(BenchPurpose::Deps), 2.0, None).unwrap()
    }

    #[test]
    fn creates_the_unprivileged_fetch_user_and_blocks_instance_metadata_for_it_v4_and_v6_before_anything_else_runs()
     {
        let ud = ud();
        assert_eq!(DEPS_OS_USER, "ratchet-deps");
        assert!(ud.contains(&format!("id -u {DEPS_OS_USER} >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin {DEPS_OS_USER}")));
        assert!(ud.contains(&format!(
            "iptables -I OUTPUT 1 -m owner --uid-owner {DEPS_OS_USER} -d 169.254.169.254 -j REJECT"
        )));
        assert!(ud.contains(&format!(
            "ip6tables -I OUTPUT 1 -m owner --uid-owner {DEPS_OS_USER} -d fd00:ec2::254 -j REJECT"
        )));
        assert!(
            ud.find("iptables -I OUTPUT").unwrap()
                < ud.find("touch /run/ratchet/user-data.done").unwrap()
        );
    }

    #[test]
    fn has_none_of_the_bench_machinery_no_candidate_launcher_no_sudo_rule_no_bench_or_holdout_user()
    {
        let ud = ud();
        for needle in [
            "ratchet-bench",
            "ratchet-holdout",
            "sudoers",
            CANDIDATE_LAUNCHER_PATH,
            CANDIDATE_EXEC_PATH,
        ] {
            assert!(!ud.contains(needle), "{needle}");
        }
    }

    #[test]
    fn has_a_shorter_lifetime_guard_than_a_bench_a_snapshot_build_is_minutes() {
        assert!(ud().contains("shutdown -h +60"));
        assert!(
            deps_user_data(&bench(BenchPurpose::Deps), 0.5, None)
                .unwrap()
                .contains("shutdown -h +30")
        );
    }

    #[test]
    fn without_a_proxy_writes_no_proxy_variables_with_one_the_agent_subnet_proxy_variables_and_nothing_sensitive()
     {
        let plain = ud();
        assert!(!plain.contains("PROXY") && !plain.contains("proxy"));
        let proxied = deps_user_data(&bench(BenchPurpose::Deps), 2.0, Some(&proxy())).unwrap();
        assert!(proxied.contains("HTTPS_PROXY='http://10.42.3.10:3128'"));
        assert!(
            proxied.contains(
                "NO_PROXY='localhost,127.0.0.1,169.254.169.254,ssm.us-east-1.amazonaws.com"
            )
        );
        assert!(proxied.contains("RATCHET_BENCH_PURPOSE='deps'"));
        let hostile =
            RunnerNetworkOptions::new("us-east-1", Some("http://10.0.0.1:3128'; rm -rf /".into()));
        let e = deps_user_data(&bench(BenchPurpose::Deps), 2.0, Some(&hostile)).unwrap_err();
        assert!(e.to_string().contains("invalid egress proxy url"), "{e}");
    }

    #[test]
    fn blocks_instance_metadata_for_the_containers_it_builds_and_fetches_in_forwarded_traffic_before_docker_starts()
     {
        let ud = ud();
        assert!(ud.contains("iptables -I DOCKER-USER 1 -d 169.254.169.254 -j REJECT"));
        assert!(ud.contains("iptables -I FORWARD 1 -d 169.254.169.254 -j REJECT"));
        assert!(ud.contains("ip6tables -I FORWARD 1 -d fd00:ec2::254 -j REJECT"));
        assert!(ud.find("DOCKER-USER 1").unwrap() < ud.find("systemctl start docker").unwrap());
        assert_valid_bash(&ud);
    }

    #[test]
    fn gives_the_docker_daemon_the_allow_list_proxy_only_when_there_is_one() {
        assert!(!ud().contains("docker.service.d"));
        let proxied = deps_user_data(&bench(BenchPurpose::Deps), 2.0, Some(&proxy())).unwrap();
        assert!(proxied.contains("/etc/systemd/system/docker.service.d/ratchet-proxy.conf"));
        assert!(proxied.contains("Environment=\"HTTPS_PROXY=http://10.42.3.10:3128\""));
        assert!(
            proxied.find("daemon-reload").unwrap()
                < proxied.find("systemctl start docker").unwrap()
        );
        assert_valid_bash(&proxied);
    }
}

// ---- bench user-data: candidate / holdout split ---------------------------------------------------------

mod bench_split {
    use super::*;

    fn ud() -> String {
        bench_user_data(&bench(BenchPurpose::Confirm), 2.0).unwrap()
    }

    fn idx(ud: &str, needle: &str) -> usize {
        ud.find(needle)
            .unwrap_or_else(|| panic!("user data contains {needle}"))
    }

    #[test]
    fn creates_both_unprivileged_users_and_blocks_imds_for_both_before_user_data_reports_done() {
        let ud = ud();
        for u in ["ratchet-bench", "ratchet-holdout"] {
            assert!(ud.contains(&format!(
                "useradd --system --no-create-home --shell /usr/sbin/nologin {u}"
            )));
            assert!(ud.contains(&format!(
                "iptables -I OUTPUT 1 -m owner --uid-owner {u} -d 169.254.169.254 -j REJECT"
            )));
            assert!(ud.contains(&format!(
                "ip6tables -I OUTPUT 1 -m owner --uid-owner {u} -d fd00:ec2::254 -j REJECT"
            )));
            assert!(
                idx(&ud, &format!("--uid-owner {u} -d 169.254.169.254"))
                    < idx(&ud, "user-data.done")
            );
        }
    }

    #[test]
    fn hides_each_user_from_the_other_hidepid_and_installs_the_launcher_and_sudo_rule_before_user_data_reports_done()
     {
        let ud = ud();
        assert!(ud.contains("hidepid=2"));
        for p in [
            CANDIDATE_EXEC_PATH,
            CANDIDATE_LAUNCHER_PATH,
            "/etc/sudoers.d/ratchet-holdout",
        ] {
            assert!(idx(&ud, p) < idx(&ud, "user-data.done"), "{p}");
        }
        // the sudoers rule is validated before it is moved into place; an invalid one aborts the boot
        assert!(idx(&ud, "visudo -cf") < idx(&ud, "mv /etc/sudoers.d/ratchet-holdout.tmp"));
        let visudo = ud.lines().find(|l| l.starts_with("visudo -cf")).unwrap();
        assert!(
            visudo.contains("|| { rm -f ") && visudo.ends_with("exit 1; }"),
            "{visudo}"
        );
    }

    #[test]
    fn starts_the_docker_engine_the_repo_container_runs_on_skipped_on_images_without_docker_and_never_adds_a_user_to_the_docker_group()
     {
        let ud = ud();
        assert!(ud.contains(
            "command -v docker >/dev/null 2>&1 && systemctl start docker >/dev/null 2>&1 || true"
        ));
        for needle in ["usermod", "gpasswd", "docker.sock"] {
            assert!(!ud.contains(needle), "{needle}");
        }
    }

    #[test]
    fn the_launcher_scripts_and_the_user_data_are_valid_bash() {
        for body in [
            ud().as_str(),
            CANDIDATE_EXEC_SCRIPT,
            CANDIDATE_LAUNCHER_SCRIPT,
        ] {
            assert_valid_bash(body);
        }
    }

    #[test]
    fn the_sudo_rule_gives_ratchet_holdout_exactly_one_thing_the_launcher_script_as_ratchet_bench_without_a_password()
     {
        let rules: Vec<&str> = CANDIDATE_SUDOERS
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("Defaults"))
            .collect();
        assert_eq!(
            rules,
            [format!(
                "ratchet-holdout ALL=(ratchet-bench) NOPASSWD: {CANDIDATE_EXEC_PATH}"
            )]
        );
        for bad in ["ALL=(ALL", "(root)", "NOPASSWD: ALL", "SETENV"] {
            assert!(!CANDIDATE_SUDOERS.contains(bad), "{bad}");
        }
        // only the location of the job's checkout crosses sudo; the holdout never does
        assert!(CANDIDATE_SUDOERS.contains(r#"env_keep += "RATCHET_CANDIDATE_DIR RATCHET_CANDIDATE_HOME RATCHET_CANDIDATE_TMP RATCHET_CANDIDATE_PATH""#));
        assert!(!CANDIDATE_SUDOERS.contains("RATCHET_TRACE"));
        // stdin/stdout pipes between harness and candidate stay pipes
        assert!(CANDIDATE_SUDOERS.contains("!use_pty"));
    }

    #[test]
    fn the_launcher_starts_candidate_code_with_a_clean_environment_and_refuses_to_forward_the_holdout_or_credentials()
     {
        assert!(CANDIDATE_EXEC_SCRIPT.contains("exec env -i PATH="));
        assert!(CANDIDATE_EXEC_SCRIPT.contains("RATCHET_TRACE=*|AWS_*=*"));
        assert!(CANDIDATE_EXEC_SCRIPT.contains("must run as ratchet-bench"));
        // it enters the candidate's checkout, set by root, and never reads the holdout path itself
        assert!(CANDIDATE_EXEC_SCRIPT.contains("cd \"$RATCHET_CANDIDATE_DIR\""));
        assert!(
            CANDIDATE_LAUNCHER_SCRIPT
                .contains(&format!("sudo -n -u ratchet-bench {CANDIDATE_EXEC_PATH}"))
        );
    }

    #[test]
    fn the_launcher_script_refuses_to_run_as_anyone_but_the_bench_user() {
        let out = Command::new("bash")
            .args(["-c", CANDIDATE_EXEC_SCRIPT, "x", "id"])
            .env("RATCHET_CANDIDATE_DIR", "/")
            .env("RATCHET_CANDIDATE_HOME", "/")
            .env("RATCHET_CANDIDATE_TMP", "/")
            .env("RATCHET_CANDIDATE_PATH", "/bin")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(77));
        assert!(String::from_utf8_lossy(&out.stderr).contains("must run as ratchet-bench"));
    }

    #[test]
    fn the_constants_the_bench_scripts_rely_on_match() {
        assert_eq!(
            [BENCH_OS_USER, HOLDOUT_OS_USER, CANDIDATE_LAUNCHER_PATH],
            [
                "ratchet-bench",
                "ratchet-holdout",
                "/usr/local/bin/ratchet-candidate"
            ]
        );
        // the scripts are written with the user names inlined: keep them in step with the constants
        assert!(CANDIDATE_EXEC_SCRIPT.contains(&format!("[ \"$(id -un)\" = {BENCH_OS_USER} ]")));
        assert!(
            CANDIDATE_EXEC_SCRIPT
                .contains(&format!("USER={BENCH_OS_USER} LOGNAME={BENCH_OS_USER}"))
        );
        assert!(
            CANDIDATE_LAUNCHER_SCRIPT
                .contains(&format!("-u {BENCH_OS_USER} {CANDIDATE_EXEC_PATH}"))
        );
        assert!(CANDIDATE_SUDOERS.contains(&format!("Defaults:{HOLDOUT_OS_USER} ")));
        assert!(CANDIDATE_SUDOERS.contains(&format!(
            "{HOLDOUT_OS_USER} ALL=({BENCH_OS_USER}) NOPASSWD: {CANDIDATE_EXEC_PATH}"
        )));
    }

    #[test]
    fn a_heredoc_delimiter_cannot_appear_inside_the_scripts_they_wrap() {
        for (body, delimiter) in [
            (CANDIDATE_EXEC_SCRIPT, "RATCHET_CANDIDATE_EXEC_EOF"),
            (CANDIDATE_LAUNCHER_SCRIPT, "RATCHET_CANDIDATE_LAUNCHER_EOF"),
            (CANDIDATE_SUDOERS, "RATCHET_SUDOERS_EOF"),
        ] {
            assert!(!body.contains(delimiter));
        }
    }
}
