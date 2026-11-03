//! The dependency-snapshot job (port of `deps-scripts.ts`): ONE script, run through `CloudCompute::run_command` on an
//! instance launched with purpose `deps` (agent subnet, egress through the proxy when enabled, an identity that can write
//! `bench-deps/` and nothing else).
//!
//! What it does, in order:
//!   1. root: download the staged BASE tree (`bench.ref`, `deps-base.tgz`; the candidate is never staged for this job),
//!      verify the digest, unpack it;
//!   2. root: verify the unprivileged user exists and cannot reach instance metadata (same checks as the bench, exit 12-14);
//!   3. as the unprivileged `ratchet-deps` user, `--no-new-privs`, `env -i` plus the proxy variables only (no AWS_*): the
//!      strategy's fetch step (`pnpm fetch` / `npm ci --ignore-scripts`). It fills the store; it installs no node_modules
//!      and runs no lifecycle script of any package;
//!   4. root: prune per-project state, refuse symlinks, pack the store (zstd if the image has it, else gzip), digest it;
//!   5. root: upload the archive to `bench-deps/<key>/<sha256>.<ext>` and THEN the manifest `bench-deps/<key>.json`.
//!
//! The user that touches the network and downloaded packages never holds a credential, and the user that holds the
//! credential (root, via the instance profile) never executes anything from the network.

use serde_json::json;

use crate::deps::{DEPS_PREFIX, DEPS_USER, DepsStrategy, DepsStrategyId, FileHashes};
use crate::errors::{BenchError, BenchResult};
use crate::image_scripts::{
    ContainerNetwork, ContainerRunSpec, ImageRestore, assert_base_archive,
    assert_container_runtime, container_run, ctr_function, ensure_image_loaded, is_bucket_name,
};
use crate::remote_scripts::{Isolation, Layout, StagedArchive};
use crate::shell::{SENTINEL, fill, json_str, reject_nul, sq};

/// Exit codes worth recognising (the executor reports the stderr tail with them).
pub mod exit {
    pub const PREPARE: u8 = 10;
    pub const DOWNLOAD: u8 = 20;
    pub const DIGEST: u8 = 21;
    pub const UNPACK: u8 = 22;
    pub const FETCH: u8 = 50;
    pub const SYMLINK: u8 = 51;
    pub const PACK: u8 = 52;
    pub const UPLOAD_ARCHIVE: u8 = 53;
    pub const UPLOAD_MANIFEST: u8 = 54;
}

#[derive(Debug, Clone)]
pub struct DepsBuildInput {
    pub layout: Layout,
    pub strategy: DepsStrategy,
    /// The staged BASE tree (`deps-base.tgz`).
    pub base: StagedArchive,
    pub key: String,
    /// Hashes of the base tree's lockfiles (as the controller computed them).
    pub lockfiles: FileHashes,
    pub image: String,
    pub arch: String,
    /// Artifacts bucket the archive and the manifest are uploaded to.
    pub bucket: String,
    /// Limit for the fetch step.
    pub timeout_ms: u64,
    /// `none` runs everything as the current user (tests only).
    pub isolation: Isolation,
    /// The repo's container image (`environment.image`): the fetch runs INSIDE it (the bench instances have no node or
    /// package manager of their own), as the same unprivileged user, with all capabilities dropped and a read-only root,
    /// but WITH network (this instance is the one with egress). The image is loaded from the artifacts bucket when this
    /// instance does not hold it yet.
    pub container: Option<ImageRestore>,
}

fn is_env_name(k: &str) -> bool {
    let mut chars = k.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn deps_build_script(o: &DepsBuildInput) -> BenchResult<String> {
    if o.strategy.id == DepsStrategyId::None {
        return Err(BenchError::infra(
            "there is nothing to snapshot for a strategy without a lockfile",
        ));
    }
    if !crate::deps::is_deps_key(&o.key) {
        return Err(BenchError::infra(format!(
            "snapshot key {} is not a plain key",
            json_str(&o.key)
        )));
    }
    if !is_bucket_name(&o.bucket) {
        return Err(BenchError::infra(format!(
            "artifacts bucket {} is not a valid bucket name",
            json_str(&o.bucket)
        )));
    }
    if let Some(c) = &o.container {
        assert_container_runtime(&c.rt, &o.layout.work)?;
    }
    assert_base_archive(&o.base)?;
    let isolated = o.isolation == Isolation::User;
    let store = format!("{}/store", o.layout.work);
    let fetch_env = o.strategy.fetch_env(&store);
    for (k, _) in &fetch_env {
        if !is_env_name(k) {
            return Err(BenchError::infra(format!(
                "invalid environment variable name {k}"
            )));
        }
    }
    reject_nul(
        [
            o.layout.work.as_str(),
            o.strategy.fetch,
            o.image.as_str(),
            o.arch.as_str(),
        ]
        .into_iter()
        .chain(
            o.lockfiles
                .iter()
                .flat_map(|(k, v)| [k.as_str(), v.as_str()]),
        ),
    )?;
    let env_args = fetch_env
        .iter()
        .map(|(k, v)| format!("{k}={}", sq(v)))
        .collect::<Vec<_>>()
        .join(" ");
    let timeout_s = o.timeout_ms.div_ceil(1000).max(1);
    let head = serde_json::to_string(&json!({
        "v": 1,
        "key": o.key,
        "strategy": o.strategy.id.as_str(),
        "rev": o.strategy.rev,
        "lockfiles": o.lockfiles,
        "image": o.image,
        "arch": o.arch,
    }))
    .unwrap_or_default();
    let head_open = &head[..head.len() - 1];
    let archive_prefix = format!("{DEPS_PREFIX}{}", o.key);

    // The fetch command, as the unprivileged user: a clean environment plus only the proxy variables the user data wrote.
    let clean = format!(
        "env -i PATH=\"$PATH\" HOME=\"$W/home\" TMPDIR=\"$W/tmp\" LANG=C.UTF-8 USER=\"$DU\" LOGNAME=\"$DU\" ${{PX[@]+\"${{PX[@]}}\"}} {env_args} bash -c {}",
        sq(o.strategy.fetch)
    );
    let run = if isolated {
        format!("setpriv --reuid=\"$DU\" --regid=\"$DU\" --clear-groups --no-new-privs -- {clean}")
    } else {
        clean
    };
    let fetch_command = match &o.container {
        Some(c) => {
            let mut extra: Vec<String> = fetch_env
                .iter()
                .flat_map(|(k, v)| ["-e".to_owned(), sq(&format!("{k}={v}"))])
                .collect();
            extra.push("${DENV[@]+\"${DENV[@]}\"}".into());
            container_run(&ContainerRunSpec {
                rt: c.rt.clone(),
                name: "\"$CN\"".into(),
                user: "\"$BUID:$BGID\"".into(),
                cap_add: vec![],
                no_new_privileges: true,
                network: ContainerNetwork::Bridge,
                workdir: "\"$TREE\"".into(),
                mounts: vec![
                    "$TREE:$TREE".into(),
                    "$STORE:$STORE".into(),
                    "$W/home:$W/home".into(),
                    "$W/tmp:$W/tmp".into(),
                ],
                pass_env: vec![],
                env: vec!["HOME=$W/home".into(), "TMPDIR=$W/tmp".into()],
                extra,
                command: sq(o.strategy.fetch),
            })
        }
        None => run,
    };
    let in_container = o.container.is_some();
    let tool = |cmd: &str| -> String {
        if in_container {
            format!("ctr {}", sq(cmd))
        } else {
            cmd.to_owned()
        }
    };

    let isolation_checks = if isolated {
        "id -u \"$DU\" >/dev/null 2>&1 || { echo \"snapshot user $DU does not exist on this image\" >&2; exit 12; }\n\
iptables -C OUTPUT -m owner --uid-owner \"$DU\" -d 169.254.169.254 -j REJECT >/dev/null 2>&1 || { echo \"IMDS is not blocked for $DU (iptables owner rule missing)\" >&2; exit 13; }\n\
if setpriv --reuid=\"$DU\" --regid=\"$DU\" --clear-groups --no-new-privs -- timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then echo \"instance metadata is reachable as $DU\" >&2; exit 14; fi\n\
chown -R \"$DU:\" \"$TREE\" \"$STORE\" \"$W/home\" \"$W/tmp\" || exit 16"
            .to_owned()
    } else {
        String::new()
    };
    let container_setup = match &o.container {
        Some(c) => format!(
            "DENV=(); for kv in ${{PX[@]+\"${{PX[@]}}\"}}; do DENV+=(-e \"$kv\"); done\n{}\nCN=\"ratchet-$(basename -- \"$W\")-fetch\"\n{}{}docker rm -f \"$CN\" >/dev/null 2>&1 || true\n",
            if isolated {
                "BUID=$(id -u \"$DU\"); BGID=$(id -g \"$DU\")"
            } else {
                "BUID=$(id -u); BGID=$(id -g)"
            },
            ensure_image_loaded(c)?,
            ctr_function(&c.rt),
        ),
        None => String::new(),
    };

    Ok(fill(
        r#"set -uo pipefail
W={{work}}
TREE="$W/tree"; STORE={{store}}; OUT="$W/out"
emit() { printf '%s %s %s\n' '{{sentinel}}' "$1" "$2"; }
rm -rf -- "${W:?}"; mkdir -p "$TREE" "$STORE" "$OUT" "$W/in" "$W/home" "$W/tmp" || exit {{prepare}}
chmod 755 "$W"
aws s3 cp --only-show-errors {{base_uri}} "$W/in/base.tgz" || { echo "cannot download the base tree {{base_uri_raw}}" >&2; exit {{download}}; }
[ "$(sha256sum -- "$W/in/base.tgz" | cut -d' ' -f1)" = {{base_sha}} ] || { echo "sha256 mismatch for the base tree" >&2; exit {{digest}}; }
tar -xzf "$W/in/base.tgz" -C "$TREE" --no-same-owner || exit {{unpack}}
rm -f "$W/in/base.tgz"
DU={{du}}
{{isolation_checks}}
[ -r /etc/ratchet/runner.env ] && { set -a; . /etc/ratchet/runner.env; set +a; }
PX=(); for v in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy NO_PROXY no_proxy NODE_USE_ENV_PROXY; do [ -n "${!v:-}" ] && PX+=("$v=${!v}"); done
{{container_setup}}cd "$TREE" || exit {{prepare}}
s=$(date +%s%N)
timeout -k 10 {{timeout_s}} {{fetch_command}} </dev/null >"$W/fetch.out" 2>&1; rc=$?
{{container_rm}}e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
echo "--- fetch output tail ---"; tail -c 4000 "$W/fetch.out"
[ $rc -eq 0 ] || { echo "the package fetch ({{strategy}}) exited with status $rc" >&2; exit {{fetch_code}}; }

{{prune}}
if [ -n "$(find "$STORE" -type l -print -quit)" ]; then echo "the fetched store contains symlinks; refusing to pack it" >&2; exit {{symlink}}; fi
if command -v zstd >/dev/null 2>&1; then
  EXT=tar.zst; COMP=zstd
  tar -C "$STORE" --sort=name --owner=0 --group=0 --numeric-owner -cf - . | zstd -T0 -3 -q -o "$OUT/snapshot" || exit {{pack}}
else
  EXT=tgz; COMP=gzip
  tar -C "$STORE" --sort=name --owner=0 --group=0 --numeric-owner -cf - . | gzip -3 > "$OUT/snapshot" || exit {{pack}}
fi
SHA=$(sha256sum -- "$OUT/snapshot" | cut -d' ' -f1); BYTES=$(stat -c %s -- "$OUT/snapshot")
NODEV=$({{tool_node}} 2>/dev/null | tr -cd 'A-Za-z0-9._+-'); PMV=$({{tool_pm}} 2>/dev/null | tr -cd 'A-Za-z0-9._+-')
[ -n "$NODEV" ] && [ -n "$PMV" ] || { echo "cannot determine the node and {{strategy}} versions" >&2; exit {{pack}}; }
ARCHIVE={{archive_prefix_q}}/"$SHA.$EXT"
printf '%s,"node":"%s","pm":"%s","compression":"%s","archive":"%s","sha256":"%s","bytes":%s,"createdAt":%s}\n' {{head_q}} "$NODEV" "$PMV" "$COMP" "$ARCHIVE" "$SHA" "$BYTES" "$(date +%s%3N)" > "$OUT/manifest.json"
aws s3 cp --only-show-errors "$OUT/snapshot" "s3://{{bucket}}/$ARCHIVE" || { echo "cannot upload the snapshot" >&2; exit {{up_archive}}; }
aws s3 cp --only-show-errors "$OUT/manifest.json" {{manifest_dst_q}} || { echo "cannot upload the manifest" >&2; exit {{up_manifest}}; }
emit deps "{\"key\":\"{{key}}\",\"sha256\":\"$SHA\",\"bytes\":$BYTES,\"compression\":\"$COMP\",\"ms\":$ms}"
exit 0
"#,
        &[
            ("work", &sq(&o.layout.work)),
            ("store", &sq(&store)),
            ("sentinel", SENTINEL),
            ("prepare", &exit::PREPARE.to_string()),
            ("base_uri", &sq(&o.base.uri)),
            ("base_uri_raw", &o.base.uri),
            ("download", &exit::DOWNLOAD.to_string()),
            ("base_sha", &sq(&o.base.sha256)),
            ("digest", &exit::DIGEST.to_string()),
            ("unpack", &exit::UNPACK.to_string()),
            ("du", &sq(if isolated { DEPS_USER } else { "deps" })),
            ("isolation_checks", &isolation_checks),
            ("container_setup", &container_setup),
            ("timeout_s", &timeout_s.to_string()),
            ("fetch_command", &fetch_command),
            (
                "container_rm",
                if in_container {
                    "docker rm -f \"$CN\" >/dev/null 2>&1 || true\n"
                } else {
                    ""
                },
            ),
            ("strategy", o.strategy.id.as_str()),
            ("fetch_code", &exit::FETCH.to_string()),
            ("prune", o.strategy.prune),
            ("symlink", &exit::SYMLINK.to_string()),
            ("pack", &exit::PACK.to_string()),
            ("tool_node", &tool("node --version")),
            ("tool_pm", &tool(o.strategy.version)),
            ("archive_prefix_q", &sq(&archive_prefix)),
            ("head_q", &sq(head_open)),
            ("bucket", &o.bucket),
            ("up_archive", &exit::UPLOAD_ARCHIVE.to_string()),
            (
                "manifest_dst_q",
                &sq(&format!("s3://{}/{archive_prefix}.json", o.bucket)),
            ),
            ("up_manifest", &exit::UPLOAD_MANIFEST.to_string()),
            ("key", &o.key),
        ],
    ))
}
