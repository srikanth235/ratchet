//! EC2 user data for the three kinds of machines Ratchet launches: agent runners, bench machines and
//! dependency-snapshot builders.
//!
//! Security-relevant: these scripts create the unprivileged OS users, block instance metadata (IMDS) for them, hide
//! processes between users (`hidepid=2`), install the candidate launcher and its one sudo rule, and write the
//! environment file. Secret VALUES never appear here, only SSM parameter names. Everything interpolated into a shell
//! file is single-quoted with [`sh_quote`] or validated first. The output is byte-identical to the TypeScript
//! `packages/aws/src/compute/aws.ts`; the snapshot tests pin it.

use crate::domain::{BenchLaunchSpec, RunnerLaunchSpec};
use crate::error::{Error, Result};
use crate::util::{json_str, sh_quote};
use regex::Regex;
use std::sync::LazyLock;

const COMMON_SCRIPT_HEAD: &str = "#!/bin/bash\nset -euo pipefail\n";

/// Local backstop to the reaper: shutdown terminates (`InstanceInitiatedShutdownBehavior=terminate`).
fn lifetime_guard(hours: f64) -> String {
    // JS: Math.max(1, Math.round(hours * 60)). `f64::round` and `Math.round` agree for positive values.
    let minutes = (hours * 60.0).round().max(1.0) as i64;
    format!("shutdown -h +{minutes} >/dev/null 2>&1 || true\n")
}

/// `/etc/ratchet/runner.env` written root-only; the vars keep their order.
fn env_file(vars: &[(String, String)]) -> String {
    let lines = vars
        .iter()
        .map(|(k, v)| format!("{k}={}", sh_quote(v)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "install -d -m 700 /etc/ratchet\numask 077\ncat > /etc/ratchet/runner.env <<'RATCHET_ENV_EOF'\n{lines}\nRATCHET_ENV_EOF\n"
    )
}

fn kv(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

/// Network settings of a machine in the agent subnet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerNetworkOptions {
    pub region: String,
    /// Egress proxy URL (`http://ip:3128`); `None` or empty = direct HTTPS.
    pub proxy_url: Option<String>,
}

impl RunnerNetworkOptions {
    pub fn new(region: impl Into<String>, proxy_url: Option<String>) -> Self {
        Self {
            region: region.into(),
            proxy_url,
        }
    }

    fn proxy(&self) -> Option<&str> {
        self.proxy_url.as_deref().filter(|p| !p.is_empty())
    }
}

/// Service hostnames reached through the VPC interface/gateway endpoints, never through the proxy.
fn no_proxy_list(region: &str) -> String {
    [
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "169.254.169.254".to_string(),
        format!("ssm.{region}.amazonaws.com"),
        format!("ssmmessages.{region}.amazonaws.com"),
        format!("ec2messages.{region}.amazonaws.com"),
        format!("logs.{region}.amazonaws.com"),
        format!("s3.{region}.amazonaws.com"),
        format!(".s3.{region}.amazonaws.com"),
    ]
    .join(",")
}

static PROXY_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^http://[0-9.]+:[0-9]+$").expect("static regex"));

/// HTTP(S)_PROXY and friends for a machine in the agent subnet; empty without a proxy. Validates the URL (it ends up in a
/// shell env file).
fn proxy_env(net: Option<&RunnerNetworkOptions>) -> Result<Vec<(String, String)>> {
    let Some(net) = net else { return Ok(vec![]) };
    let Some(url) = net.proxy() else {
        return Ok(vec![]);
    };
    if !PROXY_URL_RE.is_match(url) {
        return Err(Error::invalid(format!(
            "invalid egress proxy url {}",
            json_str(url)
        )));
    }
    let no_proxy = no_proxy_list(&net.region);
    Ok(vec![
        kv("HTTPS_PROXY", url),
        kv("https_proxy", url),
        kv("HTTP_PROXY", url),
        kv("http_proxy", url),
        kv("NO_PROXY", no_proxy.clone()),
        kv("no_proxy", no_proxy),
        // Node's built-in fetch ignores the variables above unless this is set (the agent CLIs are Node programs).
        kv("NODE_USE_ENV_PROXY", "1"),
    ])
}

static SECRET_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z_][A-Z0-9_]*$").expect("static regex"));
static SECRET_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.\-/]+$").expect("static regex"));

/// Prepares an agent runner; the app then starts `ratchet-runner` over SSM. Ends by touching
/// `/run/ratchet/user-data.done`. Secret VALUES never appear here: only SSM refs, fetched with the instance profile.
pub fn runner_user_data(
    spec: &RunnerLaunchSpec,
    max_lifetime_hours: f64,
    net: Option<&RunnerNetworkOptions>,
) -> Result<String> {
    // Comma-separated: the format the runner's secret-ref parser reads. Neither env names nor SSM parameter names allow commas.
    for s in &spec.secrets {
        if !SECRET_NAME_RE.is_match(&s.name) || !SECRET_REF_RE.is_match(&s.reference) {
            return Err(Error::invalid(format!(
                "invalid secret ref {}",
                json_str(&format!("{}={}", s.name, s.reference))
            )));
        }
    }
    let refs = spec
        .secrets
        .iter()
        .map(|s| format!("{}={}", s.name, s.reference))
        .collect::<Vec<_>>()
        .join(",");
    let proxy = proxy_env(net)?;
    let mut vars = vec![
        kv("RATCHET_OPTIMIZATION_ID", spec.optimization_id.clone()),
        kv("RATCHET_AGENT_ID", spec.agent_id.clone()),
        kv("RATCHET_AGENT_KIND", spec.kind.as_str()),
        kv("RATCHET_REPO_URL", spec.repo_url.clone()),
        kv("RATCHET_BRANCH", spec.branch.clone()),
        kv("RATCHET_SECRET_REFS", refs),
    ];
    vars.extend(proxy);
    let mut out = String::new();
    out.push_str(COMMON_SCRIPT_HEAD);
    out.push_str(&lifetime_guard(max_lifetime_hours));
    out.push_str(&env_file(&vars));
    out.push_str("set -a; . /etc/ratchet/runner.env; set +a\n");
    // Login shells (SSM sessions, agents spawning bash) get the proxy too.
    if net.is_some_and(|n| n.proxy().is_some()) {
        out.push_str(
            "grep -E '^(HTTPS?_PROXY|https?_proxy|NO_PROXY|no_proxy|NODE_USE_ENV_PROXY)=' /etc/ratchet/runner.env | sed 's/^/export /' > /etc/profile.d/ratchet-proxy.sh\nchmod 644 /etc/profile.d/ratchet-proxy.sh\n",
        );
    }
    out.push_str("mkdir -p /run/ratchet && touch /run/ratchet/user-data.done\n");
    Ok(out)
}

/// Unprivileged OS user that runs setup, guardrails and every process of the candidate. Must match remote-scripts `BENCH_USER`.
pub const BENCH_OS_USER: &str = "ratchet-bench";
/// Unprivileged OS user of the evaluator harness: the only one that can open the holdout (mode 0600). Must match remote-scripts `HOLDOUT_USER`.
pub const HOLDOUT_OS_USER: &str = "ratchet-holdout";
/// What the harness runs to start a candidate process as [`BENCH_OS_USER`]. Must match remote-scripts `CANDIDATE_LAUNCHER`.
pub const CANDIDATE_LAUNCHER_PATH: &str = "/usr/local/bin/ratchet-candidate";
/// Runs as [`BENCH_OS_USER`], only reachable through the sudo rule below.
pub const CANDIDATE_EXEC_PATH: &str = "/usr/local/sbin/ratchet-candidate-exec";
const CANDIDATE_SUDOERS_PATH: &str = "/etc/sudoers.d/ratchet-holdout";
/// Variables sudo keeps for the launcher (set by the root-run bench scripts before the harness starts).
pub const CANDIDATE_ENV_KEEP: [&str; 4] = [
    "RATCHET_CANDIDATE_DIR",
    "RATCHET_CANDIDATE_HOME",
    "RATCHET_CANDIDATE_TMP",
    "RATCHET_CANDIDATE_PATH",
];

/// `/usr/local/sbin/ratchet-candidate-exec`: runs ONE command as `ratchet-bench` in the candidate's checkout with a clean
/// environment (`env -i`): no `RATCHET_TRACE`, no `AWS_*`, nothing the harness (a different user) holds. Leading
/// `NAME=value` arguments become the candidate's environment, except the holdout and credentials, which are refused.
/// stdin, stdout and stderr are the caller's.
pub const CANDIDATE_EXEC_SCRIPT: &str = r##"#!/bin/bash
set -eu
[ "$(id -un)" = ratchet-bench ] || { echo "ratchet-candidate-exec must run as ratchet-bench" >&2; exit 77; }
: "${RATCHET_CANDIDATE_DIR:?}" "${RATCHET_CANDIDATE_HOME:?}" "${RATCHET_CANDIDATE_TMP:?}" "${RATCHET_CANDIDATE_PATH:?}"
for a in "$@"; do
  case "$a" in
    [A-Za-z_]*=*)
      case "$a" in
        RATCHET_TRACE=*|AWS_*=*) echo "refusing to pass ${a%%=*} to candidate code" >&2; exit 78 ;;
      esac ;;
    *) break ;;
  esac
done
cd "$RATCHET_CANDIDATE_DIR"
exec env -i PATH="$RATCHET_CANDIDATE_PATH" HOME="$RATCHET_CANDIDATE_HOME" TMPDIR="$RATCHET_CANDIDATE_TMP" USER=ratchet-bench LOGNAME=ratchet-bench LANG=C.UTF-8 "$@"
"##;

/// `/usr/local/bin/ratchet-candidate`: what the harness calls (`$RATCHET_CANDIDATE_EXEC`).
pub const CANDIDATE_LAUNCHER_SCRIPT: &str = r##"#!/bin/sh
exec sudo -n -u ratchet-bench /usr/local/sbin/ratchet-candidate-exec "$@"
"##;

/// The only privilege the harness user has: run the launcher script as the bench user. `!use_pty` so stdin/stdout pipes
/// between harness and candidate stay pipes; `env_keep` carries the job's checkout location through sudo.
pub const CANDIDATE_SUDOERS: &str = r##"Defaults:ratchet-holdout !requiretty, !use_pty, !lecture, env_keep += "RATCHET_CANDIDATE_DIR RATCHET_CANDIDATE_HOME RATCHET_CANDIDATE_TMP RATCHET_CANDIDATE_PATH"
ratchet-holdout ALL=(ratchet-bench) NOPASSWD: /usr/local/sbin/ratchet-candidate-exec
"##;

/// Two unprivileged users (the bench runner's checkout step verifies both, the IMDS block for both, and the launcher,
/// and refuses to run the candidate otherwise, so a failure here cannot silently weaken the bench):
///
///  - `ratchet-bench` runs setup, guardrails and every candidate process; it can never open the holdout.
///  - `ratchet-holdout` runs the evaluator harness (trusted code from bench.ref) and owns the holdout file (mode 0600).
///    Its one privilege is a sudo rule that starts a candidate process as `ratchet-bench`.
///
/// Neither user may read instance metadata: an iptables owner match rejects 169.254.169.254 for both uids, so neither
/// can fetch the instance-profile credentials (IMDSv2 with hop limit 1 is also set at launch). `hidepid=2` hides each
/// user's processes from the other. The launcher and the sudo rule are installed here, root-owned, before the bench
/// agent starts.
pub fn bench_user_data(spec: &BenchLaunchSpec, max_lifetime_hours: f64) -> Result<String> {
    let users = [BENCH_OS_USER, HOLDOUT_OS_USER];
    let mut out = String::new();
    out.push_str(COMMON_SCRIPT_HEAD);
    out.push_str(&lifetime_guard(max_lifetime_hours));
    for u in users {
        out.push_str(&format!(
            "id -u {u} >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin {u}\n"
        ));
    }
    for u in users {
        out.push_str(&format!(
            "iptables -I OUTPUT 1 -m owner --uid-owner {u} -d 169.254.169.254 -j REJECT\n"
        ));
    }
    for u in users {
        out.push_str(&format!(
            "command -v ip6tables >/dev/null 2>&1 && ip6tables -I OUTPUT 1 -m owner --uid-owner {u} -d fd00:ec2::254 -j REJECT || true\n"
        ));
    }
    out.push_str("mount -o remount,hidepid=2 /proc >/dev/null 2>&1 || true\n");
    // The repo's container (`environment.image`) runs on this engine, loaded offline, with --network none. Images without
    // Docker (the legacy host-toolchain path) simply skip it.
    out.push_str(DOCKER_START);
    out.push_str(&heredoc_install(
        CANDIDATE_EXEC_PATH,
        CANDIDATE_EXEC_SCRIPT,
        "755",
        "RATCHET_CANDIDATE_EXEC_EOF",
    )?);
    out.push_str(&heredoc_install(
        CANDIDATE_LAUNCHER_PATH,
        CANDIDATE_LAUNCHER_SCRIPT,
        "755",
        "RATCHET_CANDIDATE_LAUNCHER_EOF",
    )?);
    out.push_str(&heredoc_install(
        &format!("{CANDIDATE_SUDOERS_PATH}.tmp"),
        CANDIDATE_SUDOERS,
        "440",
        "RATCHET_SUDOERS_EOF",
    )?);
    out.push_str(&format!(
        "visudo -cf {CANDIDATE_SUDOERS_PATH}.tmp >/dev/null || {{ rm -f {CANDIDATE_SUDOERS_PATH}.tmp; echo \"invalid sudoers rule for {HOLDOUT_OS_USER}\" >&2; exit 1; }}\n"
    ));
    out.push_str(&format!(
        "mv {CANDIDATE_SUDOERS_PATH}.tmp {CANDIDATE_SUDOERS_PATH}\n"
    ));
    out.push_str(&env_file(&[
        kv("RATCHET_OPTIMIZATION_ID", spec.optimization_id.clone()),
        kv("RATCHET_BENCH_PURPOSE", spec.purpose.as_str()),
    ]));
    out.push_str("set -a; . /etc/ratchet/runner.env; set +a\n");
    out.push_str("mkdir -p /run/ratchet && touch /run/ratchet/user-data.done\n");
    Ok(out)
}

/// Unprivileged OS user that runs the package manager's fetch step in a dependency-snapshot job. Must match remote-scripts `DEPS_USER`.
pub const DEPS_OS_USER: &str = "ratchet-deps";

/// Prepares a dependency-snapshot instance (`purpose: deps`). It runs in the AGENT subnet, i.e. WITH egress (through the
/// allow-list proxy when it is enabled: the proxy variables are written to `/etc/ratchet/runner.env`, which the job
/// script sources as root and passes on explicitly), and executes no candidate code: only the package manager's fetch
/// step on the trusted base ref's lockfile, as the unprivileged `ratchet-deps` user, which has no route to instance
/// metadata (the same owner-match rule as the bench users, checked by the job script before it runs anything). Its
/// instance profile can write `bench-deps/` and nothing else. The lifetime guard is shorter than a bench's: a snapshot
/// build is minutes.
pub fn deps_user_data(
    spec: &BenchLaunchSpec,
    max_lifetime_hours: f64,
    net: Option<&RunnerNetworkOptions>,
) -> Result<String> {
    let u = DEPS_OS_USER;
    let mut vars = vec![
        kv("RATCHET_OPTIMIZATION_ID", spec.optimization_id.clone()),
        kv("RATCHET_BENCH_PURPOSE", spec.purpose.as_str()),
    ];
    vars.extend(proxy_env(net)?);
    let mut out = String::new();
    out.push_str(COMMON_SCRIPT_HEAD);
    out.push_str(&lifetime_guard(max_lifetime_hours.min(1.0)));
    out.push_str(&format!("id -u {u} >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin {u}\n"));
    out.push_str(&format!(
        "iptables -I OUTPUT 1 -m owner --uid-owner {u} -d 169.254.169.254 -j REJECT\n"
    ));
    out.push_str(&format!(
        "command -v ip6tables >/dev/null 2>&1 && ip6tables -I OUTPUT 1 -m owner --uid-owner {u} -d fd00:ec2::254 -j REJECT || true\n"
    ));
    out.push_str("mount -o remount,hidepid=2 /proc >/dev/null 2>&1 || true\n");
    out.push_str(&env_file(&vars));
    out.push_str(&docker_builder_setup(net)?);
    out.push_str("set -a; . /etc/ratchet/runner.env; set +a\n");
    out.push_str("mkdir -p /run/ratchet && touch /run/ratchet/user-data.done\n");
    Ok(out)
}

/// Starts the Docker engine when the image has one (the generic bench image does; the legacy host-toolchain image does not).
const DOCKER_START: &str =
    "command -v docker >/dev/null 2>&1 && systemctl start docker >/dev/null 2>&1 || true\n";

/// The image / dependency job builds the repo's container and runs the package fetch in it, on THIS instance (egress).
/// Containers reach the network through the host, where the owner-match rule on OUTPUT does not apply, so metadata is
/// blocked for forwarded traffic too (the DOCKER-USER chain Docker consults first, and FORWARD as a second net; IMDSv2 hop
/// limit 1 is the third). The daemon's own registry pulls need the allow-list proxy settings in its systemd environment
/// (the client environment does not reach the daemon). Images without Docker skip all of it.
fn docker_builder_setup(net: Option<&RunnerNetworkOptions>) -> Result<String> {
    let proxy = proxy_env(net)?;
    let drop_in = if proxy.is_empty() {
        String::new()
    } else {
        let get = |k: &str| {
            proxy
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        let mut s = String::from(
            "install -d -m 755 /etc/systemd/system/docker.service.d\ncat > /etc/systemd/system/docker.service.d/ratchet-proxy.conf <<'RATCHET_DOCKER_PROXY_EOF'\n[Service]\n",
        );
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"] {
            s.push_str(&format!("Environment=\"{k}={}\"\n", get(k)));
        }
        s.push_str("RATCHET_DOCKER_PROXY_EOF\nsystemctl daemon-reload\n");
        s
    };
    Ok(format!(
        "if command -v docker >/dev/null 2>&1; then\n  iptables -N DOCKER-USER >/dev/null 2>&1 || true\n  iptables -I DOCKER-USER 1 -d 169.254.169.254 -j REJECT\n  iptables -I FORWARD 1 -d 169.254.169.254 -j REJECT\n  command -v ip6tables >/dev/null 2>&1 && ip6tables -I FORWARD 1 -d fd00:ec2::254 -j REJECT || true\n{drop_in}  systemctl start docker\nfi\n"
    ))
}

fn heredoc_install(path: &str, body: &str, mode: &str, delimiter: &str) -> Result<String> {
    if body.contains(delimiter) {
        return Err(Error::other("script contains its heredoc delimiter"));
    }
    Ok(format!(
        "install -D -m {mode} -o root -g root /dev/null {path}\ncat > {path} <<'{delimiter}'\n{body}{delimiter}\n"
    ))
}
