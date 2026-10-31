//! Dependencies for a bench that has no network (port of `deps.ts`).
//!
//! Candidate setup (`pnpm install --frozen-lockfile && pnpm build`) cannot download anything on a bench instance, so the
//! packages arrive as a SNAPSHOT: the package manager's own cache/store, built once per lockfile by a short-lived job on a
//! machine that does have egress (`remote`, `remote_deps`), stored in the artifacts bucket and restored read-only before
//! setup. Setup then installs from it with the network switched off (`npm_config_offline`).
//!
//! The snapshot is built from the lockfile at `bench.ref` only, never from the candidate. A candidate whose lockfile
//! differs is BLOCKED (see [`lockfile_changes`] / [`dependency_change_guardrail`]): dependency changes are not allowed
//! in v0.
//!
//! A [`DepsStrategy`] is the small, package-manager-specific part. Everything else (keys, manifests, the job, the
//! restore) is shared.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use indexmap::IndexMap;
use ratchet_core::{DepsMode, GuardrailResult};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::errors::{BenchError, BenchResult};
use crate::shell::js_cmp;

/// Prefix in the artifacts bucket for snapshots. Only the deps identity writes here; the bench identities read.
pub const DEPS_PREFIX: &str = "bench-deps/";
/// Name of the staged base-tree archive the snapshot job downloads (`bench-inputs/<opt>/<job>/deps-base.tgz`). The deps
/// identity may read exactly this basename under `bench-inputs/`, never the candidate's `code.tgz`.
pub const DEPS_BASE_ARCHIVE: &str = "deps-base.tgz";
/// Unprivileged OS user of the snapshot job (created and IMDS-blocked by the deps user-data; `DEPS_OS_USER`).
pub const DEPS_USER: &str = "ratchet-deps";

/// `name -> sha256` of root-level files, in the order they were hashed (the order is part of generated scripts).
pub type FileHashes = IndexMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepsStrategyId {
    Pnpm,
    Npm,
    None,
}

impl DepsStrategyId {
    pub const fn as_str(self) -> &'static str {
        match self {
            DepsStrategyId::Pnpm => "pnpm",
            DepsStrategyId::Npm => "npm",
            DepsStrategyId::None => "none",
        }
    }
}

impl std::fmt::Display for DepsStrategyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The package-manager-specific part of a dependency snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepsStrategy {
    pub id: DepsStrategyId,
    /// Bumped when fetch/prune/restore change in a way that makes old snapshots wrong: it is part of the snapshot key.
    pub rev: u32,
    /// Root-relative lockfile names. Whichever exist in a tree are hashed into the key; all of them are pinned to
    /// bench.ref's version on the bench (they count as protected paths whether or not the user listed them) and a
    /// candidate that changes one is blocked.
    pub lockfiles: &'static [&'static str],
    /// Root-relative files that steer how the package manager installs (registry, hooks, workspace settings). Pinned,
    /// hashed and blocked on change exactly like the lockfiles, but they never select the strategy.
    pub config_files: &'static [&'static str],
    /// Shell, run by the snapshot job as the unprivileged user in the base tree. It must fill the store/cache named by
    /// `fetch_env` WITHOUT running lifecycle scripts of the packages (`pnpm fetch` only populates the store;
    /// `npm ci --ignore-scripts`).
    pub fetch: &'static str,
    /// Shell that prints the package manager's version (recorded in the manifest and compared on restore).
    pub version: &'static str,
    /// Root shell run on the store before it is packed (`$STORE`): drop per-project registrations and logs.
    pub prune: &'static str,
    /// Root shell run on the restored store (`$STORE`, `$SCRATCH`): wire the few paths the package manager insists on
    /// writing.
    pub restore: &'static str,
}

type Env = Vec<(String, String)>;

const QUIET: [(&str, &str); 3] = [
    ("npm_config_update_notifier", "false"),
    ("npm_config_fund", "false"),
    ("npm_config_audit", "false"),
];

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

impl DepsStrategy {
    /// Environment of the fetch (store location, no update checks, no package-manager self-update).
    pub fn fetch_env(&self, store: &str) -> Env {
        match self.id {
            DepsStrategyId::Pnpm => {
                let mut e = env(&[
                    ("npm_config_store_dir", store),
                    ("PNPM_STORE_DIR", store),
                    ("npm_config_ignore_scripts", "true"),
                    // Never download another pnpm because `packageManager` names a different version: the bench has no
                    // network and the image's pinned pnpm is the one that was verified.
                    ("npm_config_manage_package_manager_versions", "false"),
                ]);
                e.extend(env(&QUIET));
                e
            }
            DepsStrategyId::Npm => {
                let mut e = env(&[
                    ("npm_config_cache", store),
                    ("npm_config_ignore_scripts", "true"),
                ]);
                e.extend(env(&QUIET));
                e
            }
            DepsStrategyId::None => Vec::new(),
        }
    }

    /// Environment of setup and command guardrails on the bench: the restored store and offline semantics.
    pub fn setup_env(&self, store: &str, scratch: &str) -> Env {
        match self.id {
            DepsStrategyId::Pnpm => {
                let mut e = env(&[
                    ("npm_config_store_dir", store),
                    ("PNPM_STORE_DIR", store),
                    ("npm_config_offline", "true"),
                    ("npm_config_manage_package_manager_versions", "false"),
                    // Never hardlink out of the shared store (a hardlink would make a node_modules file and a store
                    // file one inode).
                    ("npm_config_package_import_method", "clone-or-copy"),
                ]);
                e.extend(env(&QUIET));
                e
            }
            DepsStrategyId::Npm => {
                let logs = format!("{scratch}/npm-logs");
                let mut e = env(&[
                    ("npm_config_cache", store),
                    ("npm_config_offline", "true"),
                    ("npm_config_logs_dir", &logs),
                ]);
                e.extend(env(&QUIET));
                e
            }
            DepsStrategyId::None => Vec::new(),
        }
    }

    /// Every file pinned to bench.ref and hashed into the snapshot key: the lockfiles plus the install-steering config.
    pub fn pinned_files(&self) -> Vec<String> {
        self.lockfiles
            .iter()
            .chain(self.config_files.iter())
            .map(|s| (*s).to_owned())
            .collect()
    }
}

/// Every file pinned to bench.ref and hashed into the snapshot key (`pinnedDepsFiles`).
pub fn pinned_deps_files(s: &DepsStrategy) -> Vec<String> {
    s.pinned_files()
}

pub const PNPM: DepsStrategy = DepsStrategy {
    id: DepsStrategyId::Pnpm,
    rev: 1,
    lockfiles: &["pnpm-lock.yaml"],
    config_files: &["pnpm-workspace.yaml", ".npmrc", ".pnpmfile.cjs"],
    // `pnpm fetch` is lockfile-only (the manifests are ignored) and, unlike `pnpm install`, populates just the
    // content-addressable store: no node_modules, no lifecycle scripts. It has no --frozen-lockfile flag because it never
    // resolves anything.
    fetch: "pnpm fetch",
    version: "pnpm --version",
    // pnpm registers every project that uses a store under `<store>/vN/projects/`; the job's tree must not leak into the
    // snapshot.
    prune: "rm -rf \"$STORE\"/v*/projects \"$STORE\"/v*/tmp",
    // `pnpm install` insists on creating `<store>/vN/projects/<hash>`. The store itself stays read-only: `projects`
    // becomes a symlink to a directory the candidate user owns (verified: a fully read-only store fails with EACCES, this
    // works).
    restore: "for v in \"$STORE\"/v*; do [ -d \"$v\" ] || continue; rm -rf \"$v/projects\"; ln -s \"$SCRATCH/pnpm-projects\" \"$v/projects\"; done; mkdir -p \"$SCRATCH/pnpm-projects\"",
};

pub const NPM: DepsStrategy = DepsStrategy {
    id: DepsStrategyId::Npm,
    rev: 1,
    lockfiles: &["package-lock.json", "npm-shrinkwrap.json"],
    config_files: &[".npmrc"],
    // Installs into a throwaway node_modules (discarded with the tree); what is kept is the download cache.
    fetch: "npm ci --ignore-scripts --no-audit --no-fund",
    version: "npm --version",
    prune: "rm -rf \"$STORE/_logs\" \"$STORE/_update-notifier-last-checked\"",
    restore: "mkdir -p \"$SCRATCH/npm-logs\"",
};

/// No lockfile, or the evaluator declared `deps: none`: nothing to snapshot, nothing pinned, nothing blocked.
pub const NO_DEPS: DepsStrategy = DepsStrategy {
    id: DepsStrategyId::None,
    rev: 1,
    lockfiles: &[],
    config_files: &[],
    fetch: "true",
    version: "true",
    prune: "true",
    restore: "true",
};

pub fn deps_strategy(id: DepsStrategyId) -> DepsStrategy {
    match id {
        DepsStrategyId::Pnpm => PNPM,
        DepsStrategyId::Npm => NPM,
        DepsStrategyId::None => NO_DEPS,
    }
}

/// `DEPS_STRATEGIES`.
pub fn deps_strategies() -> [DepsStrategy; 3] {
    [PNPM, NPM, NO_DEPS]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDeps {
    pub strategy: DepsStrategy,
    /// Human-readable note when the choice is worth a log line (e.g. an unsupported lockfile).
    pub note: Option<String>,
}

/// Picks the strategy for `evaluator.deps` given the root-level file names of the BASE tree (`bench.ref`): `auto` detects
/// by lockfile (pnpm first), `pnpm` / `npm` demand their lockfile, `none` is none.
pub fn resolve_deps_strategy<S: AsRef<str>>(
    mode: DepsMode,
    root_files: impl IntoIterator<Item = S>,
) -> BenchResult<ResolvedDeps> {
    let files: BTreeSet<String> = root_files
        .into_iter()
        .map(|s| s.as_ref().to_owned())
        .collect();
    let has = |s: &DepsStrategy| s.lockfiles.iter().any(|f| files.contains(*f));
    let plain = |strategy: DepsStrategy| ResolvedDeps {
        strategy,
        note: None,
    };
    match mode {
        DepsMode::None => Ok(plain(NO_DEPS)),
        DepsMode::Pnpm | DepsMode::Npm => {
            let strategy = if mode == DepsMode::Pnpm { PNPM } else { NPM };
            if !has(&strategy) {
                return Err(BenchError::infra(format!(
                    "evaluator.deps is \"{mode}\" but bench.ref has no {} at its root; add the lockfile or set evaluator.deps to auto or none",
                    strategy.lockfiles.join(" or ")
                )));
            }
            Ok(plain(strategy))
        }
        DepsMode::Auto => {
            if has(&PNPM) {
                return Ok(plain(PNPM));
            }
            if has(&NPM) {
                return Ok(plain(NPM));
            }
            Ok(ResolvedDeps {
                strategy: NO_DEPS,
                note: files.contains("yarn.lock").then(|| {
                    "yarn.lock found: dependency snapshots support pnpm and npm only, so nothing is installed for the bench"
                        .to_owned()
                }),
            })
        }
    }
}

fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub(crate) fn sha256_str(s: &str) -> String {
    sha256_hex(s.as_bytes())
}

/// sha256 of each existing file in `names` (root-relative) under `dir`. A symlink hashes its target text under a different
/// prefix, so a lockfile swapped for a link never equals a real one. Missing files are absent from the result.
pub fn hash_named_files<S: AsRef<str>>(dir: &Path, names: &[S]) -> BenchResult<FileHashes> {
    use std::os::unix::fs::MetadataExt;
    let mut out = FileHashes::new();
    for name in names {
        let name = name.as_ref();
        let p = dir.join(name);
        let Ok(st) = fs::symlink_metadata(&p) else {
            continue;
        };
        let value = if st.file_type().is_symlink() {
            let target = fs::read_link(&p).map_err(io_err(&p))?;
            format!("link:{}", sha256_hex(target.as_os_str().as_encoded_bytes()))
        } else if st.is_file() {
            sha256_hex(&fs::read(&p).map_err(io_err(&p))?)
        } else {
            format!("other:{:o}", st.mode())
        };
        out.insert(name.to_owned(), value);
    }
    Ok(out)
}

pub(crate) fn io_err(p: &Path) -> impl Fn(std::io::Error) -> BenchError + '_ {
    move |e| BenchError::infra(format!("{}: {e}", p.display()))
}

/// Names of the entries directly under `dir` (used to detect the strategy from the base tree's root).
pub fn root_entries(dir: &Path) -> BenchResult<Vec<String>> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).map_err(io_err(dir))? {
        let e = e.map_err(io_err(dir))?;
        out.push(e.file_name().to_string_lossy().into_owned());
    }
    Ok(out)
}

/// Lockfiles whose hash differs between the base tree and the candidate (a lockfile present on only one side counts).
/// Sorted.
pub fn lockfile_changes(base: &FileHashes, candidate: &FileHashes) -> Vec<String> {
    let names: BTreeSet<&String> = base.keys().chain(candidate.keys()).collect();
    let mut out: Vec<String> = names
        .into_iter()
        .filter(|n| base.get(*n) != candidate.get(*n))
        .cloned()
        .collect();
    crate::shell::js_sort(&mut out);
    out
}

/// The reason a candidate is blocked when its lockfile differs. Same text for the local and the remote runner.
pub fn dependency_change_guardrail(changed: &[String], bench_ref: &str) -> GuardrailResult {
    GuardrailResult {
        name: "dependencies unchanged".into(),
        passed: false,
        detail: Some(format!(
            "dependency changes are not allowed in v0: the bench has no network ({} differs from {bench_ref}). Revert the lockfile (and the dependency changes in package.json); the dependencies are those of the base ref.",
            changed.join(", ")
        )),
    }
}

/// Hint appended to a failed setup when the bench installs from a snapshot (the usual cause: package.json asks for
/// something the lockfile does not have).
pub fn offline_setup_hint(strategy: &DepsStrategy, bench_ref: &str) -> String {
    format!(
        "the bench installs from a snapshot of {} at {bench_ref} with no network; a setup that needs anything else (changed package.json dependencies, another registry, git or url dependencies) fails here, and dependency changes are not allowed in v0",
        strategy.lockfiles.join(" / ")
    )
}

static ARM_FAMILY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(a1|[a-z]+\d+[a-z]*g[a-z]*)$").expect("static regex"));

/// Graviton instance families (`c7g`, `m6gd`, `t4g`, `im4gn`, `is4gen`, `hpc7g`, `a1`) are arm64; everything else is
/// treated as x86_64.
pub fn arch_from_instance_type(instance_type: &str) -> &'static str {
    let family = instance_type.split('.').next().unwrap_or("");
    if ARM_FAMILY.is_match(family) {
        "arm64"
    } else {
        "x86_64"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepsKeyInput {
    pub strategy: DepsStrategy,
    /// Hashes of the BASE tree's lockfiles.
    pub lockfiles: FileHashes,
    /// The evaluator image (AMI id, or the container image key): it pins node, the package manager and the OS.
    pub image: String,
    /// `arch_from_instance_type` of the bench hardware: native optional dependencies differ per architecture.
    pub arch: String,
}

pub(crate) fn sorted_map(m: &FileHashes) -> Map<String, Value> {
    let mut entries: Vec<(&String, &String)> = m.iter().collect();
    entries.sort_by(|a, b| js_cmp(a.0, b.0));
    entries
        .into_iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect()
}

/// One snapshot per key: strategy + its revision, the lockfile hashes at the base ref, the image and the architecture.
/// Reused by every candidate and every optimization that shares them.
pub fn deps_key(i: &DepsKeyInput) -> String {
    let canon = json!({
        "v": 1,
        "strategy": i.strategy.id.as_str(),
        "rev": i.strategy.rev,
        "lockfiles": Value::Object(sorted_map(&i.lockfiles)),
        "image": i.image,
        "arch": i.arch,
    });
    let text = serde_json::to_string(&canon).unwrap_or_default();
    format!("{}-{}", i.strategy.id, &sha256_str(&text)[..40])
}

static DEPS_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(pnpm|npm)-[0-9a-f]{40}$").expect("static regex"));

/// Is `key` a plain snapshot key (`pnpm-<40 hex>`)?
pub fn is_deps_key(key: &str) -> bool {
    DEPS_KEY_RE.is_match(key)
}

/// `bench-deps/<key>.json`: written LAST by the snapshot job, so its presence means the archive is complete.
pub fn manifest_key(key: &str) -> String {
    format!("{DEPS_PREFIX}{key}.json")
}

/// Compression of a saved archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    Zstd,
    Gzip,
}

impl Compression {
    pub const fn as_str(self) -> &'static str {
        match self {
            Compression::Zstd => "zstd",
            Compression::Gzip => "gzip",
        }
    }
}

/// What the snapshot job records about its archive. `archive` is content-addressed (`bench-deps/<key>/<sha256>.<ext>`),
/// so two racing builders can never leave a manifest pointing at someone else's bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepsManifest {
    pub v: u8,
    pub key: String,
    pub strategy: DepsStrategyId,
    pub rev: u32,
    pub lockfiles: FileHashes,
    pub image: String,
    pub arch: String,
    /// `node --version` and the package manager's version on the builder; the bench refuses to restore on a mismatch.
    pub node: String,
    pub pm: String,
    pub compression: Compression,
    pub archive: String,
    pub sha256: String,
    pub bytes: u64,
    pub created_at: i64,
}

pub type ManifestCheck = Result<DepsManifest, String>;

static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._+-]{1,64}$").expect("static regex"));
static SHA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").expect("static regex"));

pub(crate) fn is_sha256_hex(s: &str) -> bool {
    SHA_RE.is_match(s)
}

pub(crate) fn is_version(s: &str) -> bool {
    VERSION_RE.is_match(s)
}

/// `Number(m.x) === n` for a JSON value that is a number.
fn num_eq(v: Option<&Value>, n: f64) -> bool {
    v.and_then(Value::as_f64) == Some(n)
}

/// Does JSON value `v` (an object of strings) equal `expect` as a set of entries?
pub(crate) fn same_hashes(v: Option<&Value>, expect: &FileHashes) -> bool {
    match v {
        Some(Value::Object(m)) => {
            m.len() == expect.len()
                && expect
                    .iter()
                    .all(|(k, h)| m.get(k).and_then(Value::as_str) == Some(h.as_str()))
        }
        _ => false,
    }
}

/// Parses and validates a manifest read from the artifacts bucket against what the controller computed itself from the base
/// ref. Anything that does not match is rejected (the caller rebuilds): the controller trusts the lockfile hashes it
/// computed, not the ones written to the bucket.
pub fn parse_deps_manifest(text: &str, expect: &DepsKeyInput, key: &str) -> ManifestCheck {
    let raw: Value = serde_json::from_str(text).map_err(|_| "manifest is not JSON".to_owned())?;
    let empty = Map::new();
    let m = match &raw {
        Value::Object(m) => m,
        Value::Array(_) => &empty,
        _ => return Err("manifest is not an object".into()),
    };
    let bad = |what: &str| Err(format!("manifest {what}"));
    if !num_eq(m.get("v"), 1.0) {
        return bad("has an unknown version");
    }
    if m.get("key").and_then(Value::as_str) != Some(key) {
        return bad("is for another key");
    }
    if m.get("strategy").and_then(Value::as_str) != Some(expect.strategy.id.as_str())
        || !num_eq(m.get("rev"), f64::from(expect.strategy.rev))
    {
        return bad("was built by another strategy revision");
    }
    if m.get("image").and_then(Value::as_str) != Some(expect.image.as_str())
        || m.get("arch").and_then(Value::as_str) != Some(expect.arch.as_str())
    {
        return bad("was built for another image or architecture");
    }
    if !same_hashes(m.get("lockfiles"), &expect.lockfiles) {
        return bad("has different lockfile hashes");
    }
    let node = m.get("node").and_then(Value::as_str).filter(|s| is_version(s));
    let pm = m.get("pm").and_then(Value::as_str).filter(|s| is_version(s));
    let (Some(node), Some(pm)) = (node, pm) else {
        return bad("has no valid tool versions");
    };
    let compression = match m.get("compression").and_then(Value::as_str) {
        Some("zstd") => Compression::Zstd,
        Some("gzip") => Compression::Gzip,
        _ => return bad("has an unknown compression"),
    };
    let Some(sha) = m.get("sha256").and_then(Value::as_str).filter(|s| is_sha256_hex(s)) else {
        return bad("has no valid sha256");
    };
    let ext = match compression {
        Compression::Zstd => "tar.zst",
        Compression::Gzip => "tgz",
    };
    let want = format!("{DEPS_PREFIX}{key}/{sha}.{ext}");
    let archive = m.get("archive").and_then(Value::as_str);
    if archive != Some(want.as_str()) {
        return bad("points outside its own prefix");
    }
    let bytes = match m.get("bytes").and_then(Value::as_f64) {
        Some(b) if b.is_finite() && b >= 0.0 => b,
        _ => return bad("has no valid size"),
    };
    Ok(DepsManifest {
        v: 1,
        key: key.to_owned(),
        strategy: expect.strategy.id,
        rev: expect.strategy.rev,
        lockfiles: expect.lockfiles.clone(),
        image: expect.image.clone(),
        arch: expect.arch.clone(),
        node: node.to_owned(),
        pm: pm.to_owned(),
        compression,
        archive: want,
        sha256: sha.to_owned(),
        bytes: bytes as u64,
        created_at: m.get("createdAt").and_then(Value::as_f64).map_or(0, |n| n as i64),
    })
}

/// What a bench job needs to restore a snapshot and what it compares the candidate against.
#[derive(Debug, Clone)]
pub struct DepsPlan {
    pub strategy: DepsStrategy,
    pub key: String,
    /// Commit of bench.ref the lockfiles (and the snapshot) are from; the job stages exactly this commit.
    pub base_commit: String,
    pub lockfiles: FileHashes,
    pub manifest: DepsManifest,
    /// True when the snapshot already existed (no deps instance was launched for this job).
    pub reused: bool,
}
