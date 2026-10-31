//! Pre-staging of what a bench instance needs, so it never talks to GitHub (port of `stage.ts`): the candidate tree at `sha`
//! and the trusted tree at `bench.ref`, each as a gzip tarball under `bench-inputs/<opt>/<job>/` in the artifacts bucket,
//! with the sha256 the instance must see. Built on the controller from `GitHost::checkout` (trees only; the candidate's
//! history never reaches the instance).

use std::path::Path;
use std::sync::{Arc, LazyLock};

use ratchet_core::ports::{ArtifactStore, GitHost};
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::deps::{DEPS_BASE_ARCHIVE, FileHashes, hash_named_files, hex};
use crate::errors::{BenchError, BenchResult};
use crate::remote_scripts::StagedArchive;
use crate::shell::{assert_safe_ref, json_str};

/// Prefix in the artifacts bucket for staged inputs. Agents cannot touch it (boundary: `artifacts/<opt>/<agent>/` only);
/// only the bench identities read it.
pub const STAGING_PREFIX: &str = "bench-inputs/";

static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9-]{1,64}$").expect("static regex"));

#[derive(Debug, Clone)]
pub struct StagedSource {
    pub code: StagedArchive,
    pub bench: StagedArchive,
}

/// Hashes the environment (the repo's Dockerfile / devcontainer and what it copies in) of a tree before it is archived.
pub type EnvHasher = Arc<dyn Fn(&Path) -> FileHashes + Send + Sync>;

#[derive(Clone)]
pub struct StageBase {
    pub git: Arc<dyn GitHost>,
    /// Writes into the artifacts bucket (the controller's own credentials; bench instances only read).
    pub store: Arc<dyn ArtifactStore>,
    /// Name of that bucket, for the `s3://` uris the instance downloads.
    pub bucket: String,
    pub optimization_id: String,
}

#[derive(Clone)]
pub struct StageInput {
    pub base: StageBase,
    pub sha: String,
    pub bench_ref: String,
    /// Root-level file names (the dependency strategy's lockfiles) to hash in BOTH trees before archiving; the result is in
    /// `StagedArchive::files`. The controller needs the candidate's, to block a lockfile change before the instance runs
    /// anything.
    pub hash_files: Option<Vec<String>>,
    /// Hashes the environment of a tree before it is archived; the result is in `StagedArchive::env`. Used for BOTH trees:
    /// the controller compares them to block a candidate that changes the environment.
    pub hash_environment: Option<EnvHasher>,
}

fn validate(i: &StageBase) -> BenchResult<()> {
    // These end up in S3 keys and in IAM-scoped prefixes: plain ids only.
    if !ID_RE.is_match(&i.optimization_id) {
        return Err(BenchError::infra(format!(
            "optimization id {} must match [a-z0-9-] (it is used in an S3 prefix)",
            json_str(&i.optimization_id)
        )));
    }
    if !crate::image_scripts::is_bucket_name(&i.bucket) {
        return Err(BenchError::infra(format!(
            "staging bucket {} is not a valid bucket name",
            json_str(&i.bucket)
        )));
    }
    Ok(())
}

/// Materialize `git_ref` with the GitHost, archive it as `<prefix><name>.tgz` and upload it. `.git` and history never leave
/// the controller.
async fn stage_one(
    i: &StageBase,
    work: &Path,
    prefix: &str,
    what: &str,
    git_ref: &str,
    name: &str,
    hash_files: Option<&[String]>,
    hash_environment: Option<&EnvHasher>,
) -> BenchResult<StagedArchive> {
    let tree = work.join(name);
    let commit = match i.git.resolve_ref(git_ref).await {
        Ok(c) => c,
        Err(e) => {
            return Err(BenchError::infra(format!(
                "cannot resolve {what} {git_ref}: {e}"
            )));
        }
    };
    assert_safe_ref(&commit, &format!("{what} commit"))?;
    i.git
        .checkout(&commit, &tree)
        .await
        .map_err(|e| BenchError::infra(format!("cannot resolve {what} {git_ref}: {e}")))?;
    let files = match hash_files {
        Some(names) => {
            let (tree, names) = (tree.clone(), names.to_vec());
            Some(
                tokio::task::spawn_blocking(move || hash_named_files(&tree, &names))
                    .await
                    .map_err(|e| BenchError::infra(e.to_string()))??,
            )
        }
        None => None,
    };
    let env = match hash_environment {
        Some(h) => {
            let (tree, h) = (tree.clone(), h.clone());
            Some(
                tokio::task::spawn_blocking(move || h(&tree))
                    .await
                    .map_err(|e| BenchError::infra(e.to_string()))?,
            )
        }
        None => None,
    };
    let file = work.join(format!("{name}.tgz"));
    let out = tokio::process::Command::new("tar")
        .arg("-czf")
        .arg(&file)
        .arg("-C")
        .arg(&tree)
        .arg(".")
        .output()
        .await
        .map_err(|e| BenchError::infra(format!("cannot archive {what} {git_ref}: {e}")))?;
    if !out.status.success() {
        return Err(BenchError::infra(format!(
            "cannot archive {what} {git_ref}: tar exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|e| BenchError::infra(format!("cannot archive {what} {git_ref}: {e}")))?;
    let key = format!("{prefix}{name}.tgz");
    i.store
        .put(&key, &bytes)
        .await
        .map_err(|e| BenchError::infra(format!("cannot upload {key}: {e}")))?;
    Ok(StagedArchive {
        uri: format!("s3://{}/{key}", i.bucket),
        sha256: hex(&Sha256::digest(&bytes)),
        commit,
        files,
        env,
    })
}

fn job_prefix(optimization_id: &str) -> String {
    format!(
        "{STAGING_PREFIX}{optimization_id}/{}/",
        uuid::Uuid::new_v4()
    )
}

/// Pre-stages what a bench instance needs, so it never talks to GitHub: the candidate tree at `sha` and the trusted tree at
/// `bench.ref`.
pub async fn stage_source(i: &StageInput) -> BenchResult<StagedSource> {
    validate(&i.base)?;
    assert_safe_ref(&i.sha, "candidate sha")?;
    assert_safe_ref(&i.bench_ref, "bench.ref")?;

    let prefix = job_prefix(&i.base.optimization_id);
    let work = tempfile::tempdir().map_err(|e| BenchError::infra(format!("cannot create a staging directory: {e}")))?;
    let code = stage_one(
        &i.base,
        work.path(),
        &prefix,
        "candidate",
        &i.sha,
        "code",
        i.hash_files.as_deref(),
        i.hash_environment.as_ref(),
    )
    .await?;
    let bench = stage_one(
        &i.base,
        work.path(),
        &prefix,
        "bench.ref",
        &i.bench_ref,
        "bench",
        i.hash_files.as_deref(),
        i.hash_environment.as_ref(),
    )
    .await?;
    Ok(StagedSource { code, bench })
}

/// Stages ONLY the trusted base tree for the dependency-snapshot job, as `bench-inputs/<opt>/<job>/deps-base.tgz` (the one
/// basename the deps identity may read). The candidate is never staged for that job.
pub async fn stage_deps_base(base: &StageBase, git_ref: &str) -> BenchResult<StagedArchive> {
    validate(base)?;
    assert_safe_ref(git_ref, "bench.ref")?;
    let prefix = job_prefix(&base.optimization_id);
    let work = tempfile::tempdir().map_err(|e| BenchError::infra(format!("cannot create a staging directory: {e}")))?;
    let name = DEPS_BASE_ARCHIVE.trim_end_matches(".tgz");
    stage_one(base, work.path(), &prefix, "bench.ref", git_ref, name, None, None).await
}
