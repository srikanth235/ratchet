//! Shell for the repo's container (`environment.image`, see `image`). Port of `image-scripts.ts`; the generated text is
//! pinned byte for byte against the TypeScript output (`tests/golden`).
//!
//!  - [`image_build_script`]: the IMAGE JOB. Runs through `CloudCompute::run_command` on an instance launched with purpose
//!    `deps` (agent subnet, egress, an identity that can write `bench-images/` and nothing else). It downloads the staged
//!    BASE tree, builds the image from the protected ref only, saves it (`docker save | zstd`), uploads the archive and THEN
//!    the manifest.
//!  - [`image_restore_fragment`]: part of the bench checkout script. Root loads the saved image OFFLINE after verifying its
//!    sha256 and its image id, then probes the container exactly as the steps will run it.
//!  - [`container_run`]: the `docker run` command line of a bench step. Setup, guardrails and the evaluator run in the
//!    container with `--network none`.
//!
//! Isolation, host path vs container path (the same two uids in both):
//!
//! ```text
//! property                           host (no `environment.image`)        container
//! --------------------------------   ----------------------------------   --------------------------------------------
//! candidate uid                      ratchet-bench via setpriv            same numeric uid via `--user`
//! harness uid / holdout owner        ratchet-holdout via setpriv          same numeric uid via `--user`
//! candidate no_new_privs             setpriv --no-new-privs               --security-opt no-new-privileges (and the launcher)
//! harness -> candidate               sudo rule + launcher script          setuid-root static launcher (CANDIDATE_HELPER) bind-
//!                                                                         mounted read-only; allowed caller / target in a
//!                                                                         root-owned file; bounding set {SETUID, SETGID}
//! network                            none (no route; endpoints only)      --network none: loopback only, not even the endpoints
//! instance metadata                  iptables owner rule + hop limit 1    unreachable: no interface; the owner rule still applies
//! process visibility                 hidepid=2 on /proc                   own PID namespace: the build's processes are invisible;
//!                                                                         harness and candidate share it (different uids, no ptrace)
//! capabilities                       the users have none                  --cap-drop ALL (harness: only SETUID, SETGID for the launcher)
//! filesystem                         whole instance disk                  read-only root, tmpfs /tmp, only the checkout, homes and
//!                                                                         out dir bind-mounted; the holdout directory only into
//!                                                                         the confirm harness, read-only
//! docker socket                      n/a                                  never mounted; the users are not in group docker
//! seccomp                            n/a                                  Docker's default profile (checked in `docker info`)
//! ```

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::deps::{Compression, FileHashes};
use crate::errors::{BenchError, BenchResult};
use crate::image::{IMAGE_PREFIX, ImageKind, ImageSpec};
use crate::shell::{SENTINEL, assert_safe_ref, fill, json_str, reject_nul, sq};

/// Setuid-root launcher the harness uses INSIDE the container to start candidate processes
/// (`crates/ratchet-bench/src/bin/ratchet-candidate-launcher.rs`, formerly `infra/images/scripts/ratchet-candidate-ctr.c`).
pub const CANDIDATE_HELPER: &str = "/usr/local/libexec/ratchet/ratchet-candidate-ctr";
/// Where the container sees it, and the root-owned file that says who may call it and who it becomes.
pub const HELPER_IN_CONTAINER: &str = "/ratchet/candidate";
pub const HELPER_CONF_IN_CONTAINER: &str = "/ratchet/launcher.conf";

pub mod restore_exit {
    pub const DAEMON: u8 = 60;
    pub const SECCOMP: u8 = 61;
    pub const DOWNLOAD: u8 = 62;
    pub const DIGEST: u8 = 63;
    pub const LOAD: u8 = 64;
    pub const IMAGE_ID: u8 = 65;
    pub const GROUPS: u8 = 66;
    pub const PROBE: u8 = 67;
    pub const PROBE_HARNESS: u8 = 68;
    pub const HELPER: u8 = 69;
}

pub mod build_exit {
    pub const PREPARE: u8 = 10;
    pub const DOWNLOAD: u8 = 20;
    pub const DIGEST: u8 = 21;
    pub const UNPACK: u8 = 22;
    pub const DAEMON: u8 = 70;
    pub const BUILD: u8 = 71;
    pub const INSPECT: u8 = 72;
    pub const PACK: u8 = 73;
    pub const UPLOAD_ARCHIVE: u8 = 74;
    pub const UPLOAD_MANIFEST: u8 = 75;
}

/// The options of the container every step runs in, minus the image tag (which comes from the saved image).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerOptions {
    /// `--read-only` root filesystem (default true). Switch off for toolchains that insist on writing outside the mounted
    /// directories.
    #[serde(default)]
    pub read_only_root: Option<bool>,
    /// `--pids-limit` (default 4096).
    #[serde(default)]
    pub pids_limit: Option<u32>,
    /// Size of the tmpfs on /tmp (default 2g).
    #[serde(default)]
    pub tmpfs_size: Option<String>,
    /// Host path of the setuid launcher. Default [`CANDIDATE_HELPER`]. Test seam.
    #[serde(default)]
    pub helper_path: Option<String>,
    /// Verify that the helper is a setuid root-owned file (default true). Only a test on a developer machine turns it off.
    #[serde(default)]
    pub verify_helper_owner: Option<bool>,
    /// How many times (2 s apart) the checkout waits for the docker daemon to answer (default 30).
    #[serde(default)]
    pub daemon_wait_attempts: Option<u32>,
}

impl ContainerOptions {
    pub fn runtime(&self, tag: impl Into<String>) -> ContainerRuntime {
        ContainerRuntime {
            tag: tag.into(),
            options: self.clone(),
        }
    }
}

/// [`ContainerOptions`] plus the tag of the loaded image (`ratchet-env:<key>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRuntime {
    pub tag: String,
    #[serde(flatten)]
    pub options: ContainerOptions,
}

impl ContainerRuntime {
    pub fn new(tag: impl Into<String>) -> Self {
        ContainerRuntime {
            tag: tag.into(),
            options: ContainerOptions::default(),
        }
    }
}

struct RtOpts {
    read_only_root: bool,
    pids_limit: u32,
    tmpfs_size: String,
    helper: String,
    verify_owner: bool,
    daemon_attempts: u32,
}

fn rt_opts(r: &ContainerRuntime) -> RtOpts {
    let o = &r.options;
    RtOpts {
        read_only_root: o.read_only_root.unwrap_or(true),
        pids_limit: o.pids_limit.unwrap_or(4096),
        tmpfs_size: o.tmpfs_size.clone().unwrap_or_else(|| "2g".into()),
        helper: o
            .helper_path
            .clone()
            .unwrap_or_else(|| CANDIDATE_HELPER.into()),
        verify_owner: o.verify_helper_owner.unwrap_or(true),
        daemon_attempts: o.daemon_wait_attempts.unwrap_or(30),
    }
}

static WORK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/[A-Za-z0-9._/-]+$").expect("static regex"));
static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ratchet-env:img-[0-9a-f]{40}$").expect("static regex"));
static SIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{1,6}[kmg]$").expect("static regex"));
static BUCKET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$").expect("static regex"));

pub(crate) fn is_bucket_name(b: &str) -> bool {
    BUCKET_RE.is_match(b)
}

pub fn assert_container_runtime(r: &ContainerRuntime, work: &str) -> BenchResult<()> {
    if !TAG_RE.is_match(&r.tag) {
        return Err(BenchError::infra(format!(
            "container tag {} is not a ratchet-env tag",
            json_str(&r.tag)
        )));
    }
    if !WORK_RE.is_match(work) || work.contains("..") {
        return Err(BenchError::infra(format!(
            "the work directory {} must be a plain absolute path to be bind-mounted into the container",
            json_str(work)
        )));
    }
    let o = rt_opts(r);
    if !SIZE_RE.is_match(&o.tmpfs_size) {
        return Err(BenchError::infra(format!(
            "tmpfs size {} is not like 512m or 2g",
            json_str(&o.tmpfs_size)
        )));
    }
    if o.pids_limit < 64 {
        return Err(BenchError::infra("pidsLimit must be an integer >= 64"));
    }
    if !WORK_RE.is_match(&o.helper) {
        return Err(BenchError::infra(format!(
            "helper path {} is not a plain absolute path",
            json_str(&o.helper)
        )));
    }
    Ok(())
}

/// The fixed shell that runs a command in the container: bash when the image has it, else sh. The command is the last
/// argument.
const STEP_SHELL: &str =
    "if command -v bash >/dev/null 2>&1; then exec bash -c \"$1\"; else exec sh -c \"$1\"; fi";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerNetwork {
    /// For everything on the bench.
    None,
    /// The image job's package fetch needs the default bridge.
    Bridge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRunSpec {
    pub rt: ContainerRuntime,
    /// Shell expression of the container name (a variable, e.g. `"$CN"`).
    pub name: String,
    /// Who runs inside: shell expression `uid:gid`.
    pub user: String,
    /// Capabilities added back after `--cap-drop ALL`.
    #[serde(default)]
    pub cap_add: Vec<String>,
    /// `--security-opt no-new-privileges` (false only for the harness: its launcher is setuid).
    pub no_new_privileges: bool,
    pub network: ContainerNetwork,
    /// Shell expression of the working directory.
    pub workdir: String,
    /// `src:dst[:ro]`, shell expressions, double quoted by this function.
    #[serde(default)]
    pub mounts: Vec<String>,
    /// Variables whose VALUE is already exported in the script (`-e NAME`).
    #[serde(default)]
    pub pass_env: Vec<String>,
    /// Variables with values: `NAME=shell expression` (double quoted).
    #[serde(default)]
    pub env: Vec<String>,
    /// Extra raw words (already valid shell), e.g. `"${DENV[@]}"`.
    #[serde(default)]
    pub extra: Vec<String>,
    /// The command line for the container's shell (a shell expression: quoted by the caller).
    pub command: String,
}

/// `docker run ...` as one line. Everything security-relevant is spelled out here, in one place, and probed after the image
/// is loaded.
pub fn container_run(s: &ContainerRunSpec) -> String {
    let o = rt_opts(&s.rt);
    let mut w: Vec<String> = vec![
        "docker run --rm".into(),
        format!("--name {}", s.name),
        "--pull never".into(), // the bench has no network: never try to pull
        "--init".into(),
        if s.network == ContainerNetwork::None {
            "--network none".into()
        } else {
            String::new()
        },
        "--cap-drop ALL".into(),
    ];
    w.extend(s.cap_add.iter().map(|c| format!("--cap-add {c}")));
    w.push(if s.no_new_privileges {
        "--security-opt no-new-privileges".into()
    } else {
        String::new()
    });
    // seccomp: Docker's default profile (no --security-opt seccomp=unconfined anywhere; checked against `docker info` at
    // restore)
    w.push(if o.read_only_root {
        "--read-only".into()
    } else {
        String::new()
    });
    w.push(format!(
        "--tmpfs /tmp:rw,exec,nosuid,nodev,size={}",
        o.tmpfs_size
    ));
    w.push(format!("--pids-limit {}", o.pids_limit));
    w.push(format!("--user {}", s.user));
    w.push(format!("--workdir {}", s.workdir));
    w.extend(s.mounts.iter().map(|m| format!("-v \"{m}\"")));
    w.extend(s.pass_env.iter().map(|e| format!("-e {e}")));
    w.extend(s.env.iter().map(|e| format!("-e \"{e}\"")));
    w.extend(s.extra.iter().cloned());
    w.push("--entrypoint sh".into());
    w.push(sq(&s.rt.tag));
    w.push(format!(
        "-c {} ratchet-step {}",
        sq(STEP_SHELL),
        s.command
    ));
    w.into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The ids the container runs as, resolved on the instance. In `none` isolation (tests on a developer machine) the current
/// user.
pub fn ids_fragment(isolated: bool) -> &'static str {
    if isolated {
        "BUID=$(id -u \"$BU\"); BGID=$(id -g \"$BU\"); HUID=$(id -u \"$HU\"); HGID=$(id -g \"$HU\")\n"
    } else {
        "BUID=$(id -u); BGID=$(id -g); HUID=$BUID; HGID=$BGID\n"
    }
}

/// Root-owned file the launcher reads: caller uid, target uid, target gid. Written before every harness container.
pub fn launcher_conf_fragment() -> String {
    format!(
        "printf '%s %s %s\\n' \"$HUID\" \"$BUID\" \"$BGID\" > \"$W/launcher.conf\" && chown root:root \"$W/launcher.conf\" 2>/dev/null; chmod 644 \"$W/launcher.conf\" || exit {}\n",
        restore_exit::HELPER
    )
}

/// Mounts of the harness container for the helper.
pub fn helper_mounts(rt: &ContainerRuntime) -> Vec<String> {
    vec![
        format!("{}:{HELPER_IN_CONTAINER}:ro", rt_opts(rt).helper),
        format!("$W/launcher.conf:{HELPER_CONF_IN_CONTAINER}:ro"),
    ]
}

// ---- restore (bench / confirm checkout) -------------------------------------------------------------------------------------

/// A saved image to load before setup (see `image::ImageManifest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRestore {
    /// `s3://<artifacts bucket>/bench-images/<key>/<sha256>.<ext>`
    pub uri: String,
    pub sha256: String,
    pub compression: Compression,
    /// `sha256:<64 hex>`: what `docker image inspect` must report after `docker load`.
    pub image_id: String,
    pub rt: ContainerRuntime,
}

fn assert_restore(r: &ImageRestore) -> BenchResult<()> {
    static URI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^s3://[a-z0-9][a-z0-9.-]*/bench-images/[A-Za-z0-9._/-]+$").expect("static regex")
    });
    static ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^sha256:[0-9a-f]{64}$").expect("static regex"));
    if !URI.is_match(&r.uri) || r.uri.contains("..") {
        return Err(BenchError::infra(format!(
            "image uri {} is not a plain s3:// uri under bench-images/",
            json_str(&r.uri)
        )));
    }
    if !crate::deps::is_sha256_hex(&r.sha256) {
        return Err(BenchError::infra("image digest is not a sha256 hex string"));
    }
    if !ID.is_match(&r.image_id) {
        return Err(BenchError::infra("image id is not sha256:<hex>"));
    }
    Ok(())
}

fn daemon_ready(code: u8, attempts: u32) -> String {
    fill(
        r#"command -v docker >/dev/null 2>&1 || { echo "docker is missing on this image (rebuild the bench image)" >&2; exit {{code}}; }
for _i in $(seq 1 {{n}}); do docker info >/dev/null 2>&1 && break; sleep 2; done
docker info >/dev/null 2>&1 || { echo "the docker daemon is not running" >&2; exit {{code}}; }
"#,
        &[("code", &code.to_string()), ("n", &attempts.max(1).to_string())],
    )
}

/// Download, verify and `docker load` (also used by the image job when an image exists but the dependency snapshot does not).
fn load_image(r: &ImageRestore, dir: &str) -> String {
    let extract = match r.compression {
        Compression::Zstd => "zstd -dc -- \"$IMG/in/image\" | docker load",
        Compression::Gzip => "gzip -dc -- \"$IMG/in/image\" | docker load",
    };
    fill(
        r#"IMG={{dir}}; mkdir -p "$IMG/in" || exit {{download}}
aws s3 cp --only-show-errors {{uri_q}} "$IMG/in/image" || { echo "cannot download the container image {{uri}}" >&2; exit {{download}}; }
[ "$(sha256sum -- "$IMG/in/image" | cut -d' ' -f1)" = {{sha_q}} ] || { echo "sha256 mismatch for the container image" >&2; exit {{digest}}; }
{{extract}} >"$IMG/load.out" 2>&1 || { tail -c 600 "$IMG/load.out" >&2; echo "docker load failed" >&2; exit {{load}}; }
rm -f "$IMG/in/image"
[ "$(docker image inspect --format '{{.Id}}' {{tag_q}} 2>/dev/null)" = {{id_q}} ] || { echo "the loaded image is not {{id}} (tag {{tag}})" >&2; exit {{imageid}}; }
"#,
        &[
            ("dir", dir),
            ("download", &restore_exit::DOWNLOAD.to_string()),
            ("uri_q", &sq(&r.uri)),
            ("uri", &r.uri),
            ("sha_q", &sq(&r.sha256)),
            ("digest", &restore_exit::DIGEST.to_string()),
            ("extract", extract),
            ("load", &restore_exit::LOAD.to_string()),
            ("tag_q", &sq(&r.rt.tag)),
            ("id_q", &sq(&r.image_id)),
            ("id", &r.image_id),
            ("tag", &r.rt.tag),
            ("imageid", &restore_exit::IMAGE_ID.to_string()),
        ],
    )
}

/// Runs a short command in the loaded image the way a setup step would, but as root-free, offline and read-only:
/// `ctr 'node --version'`.
pub fn ctr_function(rt: &ContainerRuntime) -> String {
    format!(
        "ctr() {{ docker run --rm --pull never --network none --cap-drop ALL --security-opt no-new-privileges --read-only --user \"$BUID:$BGID\" --entrypoint sh {} -c \"$1\"; }}\n",
        sq(&rt.tag)
    )
}

const PROBE_SETUP: &str = r#"[ "$(id -u)" = "$EXPECT_UID" ] || { echo "runs as uid $(id -u), expected $EXPECT_UID"; exit 1; }
{ [ ! -e /var/run/docker.sock ] && [ ! -e /run/docker.sock ]; } || { echo "a docker socket is visible"; exit 1; }
grep -Eq '^CapEff:[[:space:]]*0+$' /proc/self/status || { echo "capabilities are not dropped"; exit 1; }
grep -Eq '^NoNewPrivs:[[:space:]]*1' /proc/self/status || { echo "no_new_privs is not set"; exit 1; }
n=$(ls /sys/class/net 2>/dev/null | tr '\n' ' '); [ "$n" = "lo " ] || [ -z "$n" ] || { echo "network interfaces visible: $n"; exit 1; }
echo ok"#;

const PROBE_ROOTFS: &str = r#"touch /ratchet-rootfs-probe 2>/dev/null && { echo "the root filesystem is writable"; exit 1; }; echo ok"#;

fn probe_harness() -> String {
    fill(
        r#"[ "$(id -u)" = "$HARNESS_UID" ] || { echo "harness runs as uid $(id -u), expected $HARNESS_UID"; exit 1; }
cat "$PROBE_FILE" >/dev/null 2>&1 || { echo "the harness cannot read the holdout directory"; exit 1; }
out=$({{helper}} sh -c 'id -u; grep -E "^NoNewPrivs" /proc/self/status; if cat "$1" >/dev/null 2>&1; then echo LEAK; fi' x "$PROBE_FILE" 2>&1) || { echo "the launcher failed: $out"; exit 1; }
[ "$(printf '%s\n' "$out" | sed -n 1p)" = "$EXPECT_UID" ] || { echo "the launcher did not run the candidate as uid $EXPECT_UID: $out"; exit 1; }
printf '%s\n' "$out" | grep -Eq 'NoNewPrivs:[[:space:]]*1' || { echo "candidate processes have no no_new_privs"; exit 1; }
printf '%s\n' "$out" | grep -q LEAK && { echo "a candidate process can read the holdout"; exit 1; }
echo ok"#,
        &[("helper", HELPER_IN_CONTAINER)],
    )
}

/// Root, in the bench checkout, after the isolation checks: load the saved image offline and prove the container is what the
/// steps assume. Exit 60 to 69.
pub fn image_restore_fragment(r: &ImageRestore, work: &str, isolated: bool) -> BenchResult<String> {
    assert_restore(r)?;
    assert_container_runtime(&r.rt, work)?;
    let rt = rt_opts(&r.rt);
    let helper_check = if isolated {
        format!(
            "[ -x {h} ] && [ -u {h} ]{owner} || {{ echo \"the candidate launcher {raw} is missing or is not a setuid root file (rebuild the bench image)\" >&2; exit {code}; }}\n",
            h = sq(&rt.helper),
            owner = if rt.verify_owner {
                format!(" && [ \"$(stat -c %u {})\" = 0 ]", sq(&rt.helper))
            } else {
                String::new()
            },
            raw = rt.helper,
            code = restore_exit::HELPER
        )
    } else {
        String::new()
    };
    let groups = if isolated {
        format!(
            "for u in \"$BU\" \"$HU\"; do\n  if id -nG \"$u\" 2>/dev/null | tr ' ' '\\n' | grep -qxE 'docker|root|wheel|sudo'; then echo \"$u is in a privileged group (docker/root/wheel/sudo): it could reach the docker socket\" >&2; exit {}; fi\ndone\n",
            restore_exit::GROUPS
        )
    } else {
        String::new()
    };
    let probe_run = |script: &str| -> String {
        container_run(&ContainerRunSpec {
            rt: r.rt.clone(),
            name: "\"$CN\"".into(),
            user: "\"$BUID:$BGID\"".into(),
            cap_add: vec![],
            no_new_privileges: true,
            network: ContainerNetwork::None,
            workdir: "/".into(),
            mounts: vec![],
            pass_env: vec![],
            env: vec![],
            extra: vec![],
            command: format!("\"EXPECT_UID=$BUID;\"{}", sq(script)),
        })
    };
    let mut harness_env = vec![
        "RATCHET_CANDIDATE_DIR=/".to_owned(),
        "RATCHET_CANDIDATE_HOME=/tmp".to_owned(),
        "RATCHET_CANDIDATE_TMP=/tmp".to_owned(),
    ];
    let mut mounts = vec!["$W/holdout:$W/holdout:ro".to_owned()];
    mounts.extend(helper_mounts(&r.rt));
    let harness_probe_run = container_run(&ContainerRunSpec {
        rt: r.rt.clone(),
        name: "\"$CN\"".into(),
        user: "\"$HUID:$HGID\"".into(),
        cap_add: vec!["SETUID".into(), "SETGID".into()],
        no_new_privileges: false,
        network: ContainerNetwork::None,
        workdir: "/".into(),
        mounts,
        pass_env: vec![],
        env: std::mem::take(&mut harness_env),
        extra: vec![],
        command: format!(
            "\"HARNESS_UID=$HUID; EXPECT_UID=$BUID; PROBE_FILE=$W/holdout/.cprobe;\"{}",
            sq(&probe_harness())
        ),
    });
    let isolated_conf = if isolated {
        format!(
            "{}printf x > \"$W/holdout/.cprobe\" && chown \"$HU:\" \"$W/holdout/.cprobe\" && chmod 600 \"$W/holdout/.cprobe\" || exit {}\n",
            launcher_conf_fragment(),
            restore_exit::PROBE
        )
    } else {
        String::new()
    };
    let rootfs = if rt.read_only_root {
        format!(
            "pr=$({} 2>&1) || {{ echo \"container probe failed: $pr\" >&2; exit {}; }}\n",
            probe_run(PROBE_ROOTFS),
            restore_exit::PROBE
        )
    } else {
        String::new()
    };
    let harness = if isolated {
        format!(
            "docker rm -f \"$CN\" >/dev/null 2>&1 || true\npr=$({harness_probe_run} 2>&1) || {{ echo \"harness container probe failed: $pr\" >&2; rm -f \"$W/holdout/.cprobe\"; exit {}; }}\nrm -f \"$W/holdout/.cprobe\"\n",
            restore_exit::PROBE_HARNESS
        )
    } else {
        String::new()
    };
    Ok(format!(
        "# ---- container image (root): loaded offline, verified, probed ----\n{daemon}docker info --format '{{{{json .SecurityOptions}}}}' 2>/dev/null | grep -q seccomp || {{ echo \"the docker daemon runs without its default seccomp profile\" >&2; exit {seccomp}; }}\n{ids}{groups}{load}{ctr}CN=\"ratchet-$(basename -- \"$W\")-probe\"\n{helper_check}{isolated_conf}docker rm -f \"$CN\" >/dev/null 2>&1 || true\npr=$({probe} 2>&1) || {{ echo \"container probe failed: $pr\" >&2; exit {probe_code}; }}\n{rootfs}{harness}emit container \"{{\\\"tag\\\":\\\"{tag}\\\",\\\"imageId\\\":\\\"{id}\\\",\\\"readOnlyRoot\\\":{ro},\\\"network\\\":\\\"none\\\"}}\"\n",
        daemon = daemon_ready(restore_exit::DAEMON, rt.daemon_attempts),
        seccomp = restore_exit::SECCOMP,
        ids = ids_fragment(isolated),
        load = load_image(r, "\"$W/image\""),
        ctr = ctr_function(&r.rt),
        probe = probe_run(PROBE_SETUP),
        probe_code = restore_exit::PROBE,
        tag = r.rt.tag,
        id = r.image_id,
        ro = rt.read_only_root,
    ))
}

/// Same as `image_restore_fragment`'s load, for the deps job on an instance that may already hold the image: a no-op when
/// it is loaded.
pub fn ensure_image_loaded(r: &ImageRestore) -> BenchResult<String> {
    assert_restore(r)?;
    Ok(format!(
        "{}if [ \"$(docker image inspect --format '{{{{.Id}}}}' {} 2>/dev/null)\" != {} ]; then\n{}fi\n",
        daemon_ready(build_exit::DAEMON, 30),
        sq(&r.rt.tag),
        sq(&r.image_id),
        load_image(r, "\"$W/image\"")
    ))
}

// ---- the image job ------------------------------------------------------------------------------------------------------------

/// The staged BASE tree (`deps-base.tgz`) the image / snapshot job downloads.
pub use crate::remote_scripts::StagedArchive as BaseArchive;

#[derive(Debug, Clone)]
pub struct ImageBuildInput {
    pub work: String,
    /// The staged BASE tree (`deps-base.tgz`).
    pub base: BaseArchive,
    pub key: String,
    pub tag: String,
    pub spec: ImageSpec,
    /// Hashes of the environment files as the controller computed them.
    pub files: FileHashes,
    pub arch: String,
    pub bucket: String,
    /// Limit for the build.
    pub timeout_ms: u64,
}

/// A staged-base uri must be a plain `s3://` uri ending in `deps-base.tgz`.
pub(crate) fn assert_base_archive(base: &BaseArchive) -> BenchResult<()> {
    static URI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^s3://[a-z0-9][a-z0-9.-]*/[A-Za-z0-9._/-]+/deps-base\.tgz$").expect("static regex")
    });
    assert_safe_ref(&base.commit, "base commit")?;
    if !URI.is_match(&base.uri) || base.uri.contains("..") {
        return Err(BenchError::infra(format!(
            "staged base tree uri {} is not a plain s3:// uri ending in deps-base.tgz",
            json_str(&base.uri)
        )));
    }
    if !crate::deps::is_sha256_hex(&base.sha256) {
        return Err(BenchError::infra(
            "staged base tree digest is not a sha256 hex string",
        ));
    }
    Ok(())
}

/// The image job, one script: download the staged BASE tree (never the candidate), build or pull the image with the daemon of
/// this throwaway instance, save it, upload the archive and then the manifest. `docker build` runs as root on an instance
/// that has no candidate code: the Dockerfile is protected-ref input (as trusted as the lockfile), its RUN steps run in
/// build containers whose route to instance metadata is blocked by the user data (FORWARD / DOCKER-USER rule, hop limit 1)
/// and which hold no credentials (DOCKER_CONFIG points at an empty directory plus the proxy settings; AWS_* are not in the
/// environment).
pub fn image_build_script(o: &ImageBuildInput) -> BenchResult<String> {
    if !crate::image::is_image_key(&o.key) {
        return Err(BenchError::infra(format!(
            "image key {} is not a plain key",
            json_str(&o.key)
        )));
    }
    if o.tag != format!("ratchet-env:{}", o.key) {
        return Err(BenchError::infra("image tag does not match its key"));
    }
    if !BUCKET_RE.is_match(&o.bucket) {
        return Err(BenchError::infra(format!(
            "artifacts bucket {} is not a valid bucket name",
            json_str(&o.bucket)
        )));
    }
    if !WORK_RE.is_match(&o.work) {
        return Err(BenchError::infra(format!(
            "work directory {} is not a plain absolute path",
            json_str(&o.work)
        )));
    }
    assert_base_archive(&o.base)?;
    let s = &o.spec;
    reject_nul(
        [s.path.as_str(), s.image.as_deref().unwrap_or("")]
            .into_iter()
            .chain(s.args.iter().flat_map(|(k, v)| [k.as_str(), v.as_str()]))
            .chain(s.target.as_deref()),
    )?;
    let timeout_s = (o.timeout_ms.div_ceil(1000)).max(1);
    static PLAIN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._/ -]+$").expect("static regex"));
    let rel = |p: &str| -> BenchResult<String> {
        if PLAIN.is_match(p) && !p.contains("..") {
            Ok(p.to_owned())
        } else {
            Err(BenchError::infra(format!("path {} is not plain", json_str(p))))
        }
    };
    let build = if s.kind == ImageKind::Dockerfile {
        let args = s
            .args
            .iter()
            .map(|(k, v)| format!("--build-arg {}", sq(&format!("{k}={v}"))))
            .collect::<Vec<_>>()
            .join(" ");
        let dockerfile = s.dockerfile.as_deref().unwrap_or("");
        let context = s.context.as_deref().unwrap_or("");
        // The build runs in `env -i bash -c`: absolute paths, no shell variables of this script.
        format!(
            "docker build --pull --force-rm --tag {} --file {} {}{} {}",
            sq(&o.tag),
            sq(&format!("{}/tree/{}", o.work, rel(dockerfile)?)),
            s.target
                .as_ref()
                .map(|t| format!("--target {} ", sq(t)))
                .unwrap_or_default(),
            args,
            sq(&format!("{}/tree/{}", o.work, rel(context)?))
        )
    } else {
        let image = s.image.as_deref().unwrap_or("");
        format!(
            "docker pull {} && docker tag {} {}",
            sq(image),
            sq(image),
            sq(&o.tag)
        )
    };
    let head = serde_json::to_string(&json!({
        "v": 1,
        "key": o.key,
        "rev": 1,
        "kind": s.kind.as_str(),
        "path": s.path,
        "arch": o.arch,
        "files": o.files,
        "baseDigests": s.base_digests,
        "tag": o.tag,
    }))
    .unwrap_or_default();
    let head_open = &head[..head.len() - 1];
    let archive_prefix = format!("{IMAGE_PREFIX}{}", o.key);

    Ok(fill(
        r#"set -uo pipefail
W={{work}}
TREE="$W/tree"; OUT="$W/out"
emit() { printf '%s %s %s\n' '{{sentinel}}' "$1" "$2"; }
rm -rf -- "${W:?}"; mkdir -p "$TREE" "$OUT" "$W/in" "$W/home" "$W/docker-config" || exit {{prepare}}
chmod 755 "$W"
aws s3 cp --only-show-errors {{base_uri}} "$W/in/base.tgz" || { echo "cannot download the base tree {{base_uri_raw}}" >&2; exit {{download}}; }
[ "$(sha256sum -- "$W/in/base.tgz" | cut -d' ' -f1)" = {{base_sha}} ] || { echo "sha256 mismatch for the base tree" >&2; exit {{digest}}; }
tar -xzf "$W/in/base.tgz" -C "$TREE" --no-same-owner || exit {{unpack}}
rm -f "$W/in/base.tgz"
{{daemon}}[ -r /etc/ratchet/runner.env ] && { set -a; . /etc/ratchet/runner.env; set +a; }
# Proxy settings reach the build containers through the docker client config (the daemon's own pulls use its systemd drop-in).
if [ -n "${HTTPS_PROXY:-}" ]; then
  printf '{"proxies":{"default":{"httpProxy":"%s","httpsProxy":"%s","noProxy":"%s"}}}\n' "${HTTP_PROXY:-$HTTPS_PROXY}" "$HTTPS_PROXY" "${NO_PROXY:-}" > "$W/docker-config/config.json"
fi
cd "$TREE" || exit {{prepare}}
s=$(date +%s%N)
timeout -k 10 {{timeout_s}} env -i PATH="$PATH" HOME="$W/home" DOCKER_CONFIG="$W/docker-config" bash -c {{build}} </dev/null >"$W/build.out" 2>&1; rc=$?
e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
echo "--- build output tail ---"; tail -c 4000 "$W/build.out"
[ $rc -eq 0 ] || { echo "the image build exited with status $rc" >&2; exit {{build_code}}; }

IMAGEID=$(docker image inspect --format '{{.Id}}' {{tag_q}} 2>/dev/null)
case "$IMAGEID" in sha256:????????????????????????????????????????????????????????????????) ;; *) echo "cannot read the id of the built image" >&2; exit {{inspect}};; esac
if command -v zstd >/dev/null 2>&1; then
  EXT=tar.zst; COMP=zstd
  docker save {{tag_q}} | zstd -T0 -3 -q -o "$OUT/image" || exit {{pack}}
else
  EXT=tar.gz; COMP=gzip
  docker save {{tag_q}} | gzip -3 > "$OUT/image" || exit {{pack}}
fi
SHA=$(sha256sum -- "$OUT/image" | cut -d' ' -f1); BYTES=$(stat -c %s -- "$OUT/image")
ARCHIVE={{archive_prefix_q}}/"$SHA.$EXT"
printf '%s,"imageId":"%s","compression":"%s","archive":"%s","sha256":"%s","bytes":%s,"createdAt":%s}\n' {{head_q}} "$IMAGEID" "$COMP" "$ARCHIVE" "$SHA" "$BYTES" "$(date +%s%3N)" > "$OUT/manifest.json"
aws s3 cp --only-show-errors "$OUT/image" "s3://{{bucket}}/$ARCHIVE" || { echo "cannot upload the image" >&2; exit {{up_archive}}; }
aws s3 cp --only-show-errors "$OUT/manifest.json" {{manifest_dst_q}} || { echo "cannot upload the manifest" >&2; exit {{up_manifest}}; }
emit image "{\"key\":\"{{key}}\",\"imageId\":\"$IMAGEID\",\"sha256\":\"$SHA\",\"bytes\":$BYTES,\"compression\":\"$COMP\",\"ms\":$ms}"
exit 0
"#,
        &[
            ("work", &sq(&o.work)),
            ("sentinel", SENTINEL),
            ("prepare", &build_exit::PREPARE.to_string()),
            ("base_uri", &sq(&o.base.uri)),
            ("base_uri_raw", &o.base.uri),
            ("download", &build_exit::DOWNLOAD.to_string()),
            ("base_sha", &sq(&o.base.sha256)),
            ("digest", &build_exit::DIGEST.to_string()),
            ("unpack", &build_exit::UNPACK.to_string()),
            ("daemon", &daemon_ready(build_exit::DAEMON, 30)),
            ("timeout_s", &timeout_s.to_string()),
            ("build", &sq(&build)),
            ("build_code", &build_exit::BUILD.to_string()),
            ("tag_q", &sq(&o.tag)),
            ("inspect", &build_exit::INSPECT.to_string()),
            ("pack", &build_exit::PACK.to_string()),
            ("archive_prefix_q", &sq(&archive_prefix)),
            ("head_q", &sq(head_open)),
            ("bucket", &o.bucket),
            ("up_archive", &build_exit::UPLOAD_ARCHIVE.to_string()),
            (
                "manifest_dst_q",
                &sq(&format!("s3://{}/{archive_prefix}.json", o.bucket)),
            ),
            ("up_manifest", &build_exit::UPLOAD_MANIFEST.to_string()),
            ("key", &o.key),
        ],
    ))
}
