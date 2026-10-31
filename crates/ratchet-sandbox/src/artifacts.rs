//! Artifact stores: S3 for live runs, a directory for simulation. (TS `artifacts.ts`)
//!
//! `FsArtifactStore` writes are atomic like S3 PUTs: a temp file in a `.put-tmp` staging directory inside the root (same
//! filesystem, so the rename is atomic), then `rename` into place. Readers that poll event and inbox batches must never
//! see a half-written object, or they would skip it. The staging directory is never listed.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::ServerSideEncryption;

use crate::error::{Error, Result, sdk};
use crate::ports::ArtifactStore;
use crate::util::json_str;

fn assert_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.starts_with('/')
        || key.contains('\0')
        || key.split('/').any(|s| s == "..")
    {
        return Err(Error::invalid(format!(
            "invalid artifact key {}",
            json_str(key)
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct S3ArtifactStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    /// Prepended to every key (hidden from callers).
    prefix: String,
}

impl S3ArtifactStore {
    pub fn new(client: aws_sdk_s3::Client, bucket: impl Into<String>) -> Self {
        Self {
            client,
            bucket: bucket.into(),
            prefix: String::new(),
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// A store on the default credential chain, or the named profile in `~/.aws/config`.
    pub async fn connect(bucket: impl Into<String>, region: &str, profile: Option<&str>) -> Self {
        let cfg = crate::compute::aws::load_sdk_config(region, profile).await;
        Self::new(aws_sdk_s3::Client::new(&cfg), bucket)
    }
}

#[async_trait]
impl ArtifactStore for S3ArtifactStore {
    async fn put(&self, key: &str, body: &[u8]) -> Result<()> {
        assert_key(key)?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(format!("{}{key}", self.prefix))
            .body(ByteStream::from(body.to_vec()))
            .server_side_encryption(ServerSideEncryption::Aes256)
            .send()
            .await
            .map_err(sdk)?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Option<String>> {
        assert_key(key)?;
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(format!("{}{key}", self.prefix))
            .send()
            .await
        {
            Ok(r) => {
                let bytes = r
                    .body
                    .collect()
                    .await
                    .map_err(|e| Error::other(e.to_string()))?
                    .into_bytes();
                Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
            }
            Err(e) => {
                let missing = e.as_service_error().is_some_and(|s| s.is_no_such_key());
                let e = sdk(e);
                if missing || e.code() == "NoSuchKey" {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = vec![];
        let mut token: Option<String> = None;
        loop {
            let r = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(format!("{}{prefix}", self.prefix))
                .set_continuation_token(token.clone())
                .send()
                .await
                .map_err(sdk)?;
            for o in r.contents() {
                if let Some(k) = o.key() {
                    keys.push(k.strip_prefix(&self.prefix).unwrap_or(k).to_string());
                }
            }
            token = if r.is_truncated().unwrap_or(false) {
                r.next_continuation_token().map(str::to_string)
            } else {
                None
            };
            if token.is_none() {
                break;
            }
        }
        keys.sort();
        Ok(keys)
    }
}

/// Staging directory for atomic writes, inside the root (same filesystem, so rename is atomic); never listed.
const FS_TMP: &str = ".put-tmp";

/// Local-directory `ArtifactStore` for simulation. Keys map to files under `root`.
#[derive(Debug, Clone)]
pub struct FsArtifactStore {
    root: PathBuf,
}

impl FsArtifactStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            root: std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()),
        }
    }

    fn path(&self, key: &str) -> Result<PathBuf> {
        assert_key(key)?;
        // Lexical normalization like Node's `resolve`: `.` and empty segments vanish; `..` was already refused.
        let mut p = self.root.clone();
        let mut any = false;
        for seg in key.split('/').filter(|s| !s.is_empty() && *s != ".") {
            p.push(seg);
            any = true;
        }
        if !any {
            return Err(Error::other(format!(
                "artifact key escapes store root: {key}"
            )));
        }
        Ok(p)
    }
}

#[async_trait]
impl ArtifactStore for FsArtifactStore {
    async fn put(&self, key: &str, body: &[u8]) -> Result<()> {
        let p = self.path(key)?;
        let tmp_dir = self.root.join(FS_TMP);
        let parent = p.parent().unwrap_or(&self.root).to_path_buf();
        let (a, b) = tokio::join!(
            tokio::fs::create_dir_all(&parent),
            tokio::fs::create_dir_all(&tmp_dir)
        );
        a?;
        b?;
        let tmp = tmp_dir.join(format!("{:016x}", rand::random::<u64>()));
        let written = async {
            tokio::fs::write(&tmp, body).await?;
            tokio::fs::rename(&tmp, &p).await
        }
        .await;
        if let Err(e) = written {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e.into());
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Option<String>> {
        match tokio::fs::read(self.path(key)?).await {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::IsADirectory
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut out = vec![];
        // Iterative walk; an unreadable directory is skipped, like the TS `try { readdir } catch { return }`.
        let mut stack: Vec<(PathBuf, String)> = vec![(self.root.clone(), String::new())];
        while let Some((dir, rel)) = stack.pop() {
            let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                if rel.is_empty() && name == FS_TMP {
                    continue;
                }
                let k = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                let Ok(ft) = entry.file_type().await else {
                    continue;
                };
                if ft.is_dir() {
                    stack.push((dir.join(&name), k));
                } else if k.starts_with(prefix) {
                    out.push(k);
                }
            }
        }
        out.sort();
        Ok(out)
    }
}
