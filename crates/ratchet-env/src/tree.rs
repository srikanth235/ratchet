//! Reading an environment's files from a checkout of the protected ref (`environment-files.ts`).
//!
//! The functions are synchronous (a few small files); async callers run them in `spawn_blocking`.

use std::fs;
use std::io;
use std::path::Path;

use ratchet_core::config::{DEFAULT_ENVIRONMENT_INFRA_DIR, RatchetConfig};
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::ports::{SandboxEnvironment, SandboxEnvironmentFile, SandboxEnvironmentKind};
use ratchet_core::protected::normalize_repo_path;

use crate::environment::{
    EnvironmentConfigInput, EnvironmentModuleRef, environment_module_of, hash_environment_files,
};
use crate::error::{EnvError, EnvResult};
use crate::types::EnvironmentFile;
use crate::validate::{
    MAX_ENVIRONMENT_FILE_BYTES, MAX_ENVIRONMENT_TOTAL_BYTES, ValidateEnvironmentInput,
    validate_environment,
};

/// A directory of a checked-out tree as a flat, sorted list of `{ path (relative to dir), content }`. Symlinks and
/// binaries are refused. Errors are plain messages (they are wrapped by the caller).
pub fn read_environment_tree(dir: &Path) -> Result<Vec<SandboxEnvironmentFile>, TreeError> {
    let mut acc = Vec::new();
    let mut total = 0usize;
    walk(dir, "", &mut acc, &mut total)?;
    Ok(acc)
}

#[derive(Debug)]
pub enum TreeError {
    /// The directory does not exist.
    NotFound,
    /// Anything else, as a message.
    Other(String),
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("directory not found"),
            Self::Other(m) => f.write_str(m),
        }
    }
}

impl From<io::Error> for TreeError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            Self::NotFound
        } else {
            Self::Other(e.to_string())
        }
    }
}

fn walk(
    dir: &Path,
    rel: &str,
    acc: &mut Vec<SandboxEnvironmentFile>,
    total: &mut usize,
) -> Result<(), TreeError> {
    let here = if rel.is_empty() {
        dir.to_path_buf()
    } else {
        dir.join(rel)
    };
    let mut names: Vec<String> = fs::read_dir(&here)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    // JavaScript sorts strings by UTF-16 code unit; for the ASCII names of a module this is byte order
    names.sort();
    for name in names {
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let path = dir.join(&r);
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(TreeError::Other(format!(
                "{r} is a symbolic link; an environment must be plain files"
            )));
        }
        if meta.is_dir() {
            walk(dir, &r, acc, total)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let size = meta.len() as usize;
        if size > MAX_ENVIRONMENT_FILE_BYTES {
            return Err(TreeError::Other(format!(
                "{r} is {} KB (limit {} KB)",
                (size as f64 / 1024.0).round(),
                MAX_ENVIRONMENT_FILE_BYTES / 1024
            )));
        }
        *total += size;
        if *total > MAX_ENVIRONMENT_TOTAL_BYTES {
            return Err(TreeError::Other(
                "the environment is larger than 2 MB".into(),
            ));
        }
        let bytes = fs::read(&path)?;
        if bytes.contains(&0) {
            return Err(TreeError::Other(format!("{r} is not a text file")));
        }
        acc.push(SandboxEnvironmentFile {
            path: r,
            content: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }
    Ok(())
}

/// The environment of an optimization, read from a checkout of the PROTECTED ref (`root` is that checkout; a
/// candidate's tree is never used). `None` for scope `none`, for `single` without a drafted module, and when the
/// config names nothing.
///
/// A drafted module is validated here with the same allow-list the user saw when approving (module-only mode); a
/// custom (`infra:`) module is checked for the shape only, the sandbox provider scans it for commands and validates
/// its outputs.
pub fn read_environment(
    root: &Path,
    config: &RatchetConfig,
    commit: &str,
    git_ref: &str,
) -> EnvResult<Option<SandboxEnvironment>> {
    let Some(module) = environment_module_of(&EnvironmentConfigInput::from(config)) else {
        return Ok(None);
    };
    let EnvironmentModuleRef {
        kind,
        path,
        required,
    } = &module;
    let rel = normalize_repo_path(path);
    let dir = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    let files = match fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(EnvError::invalid_config(format!(
                "cannot read {path} at {git_ref}: {path} is not a plain directory"
            )));
        }
        Ok(_) => match read_environment_tree(&dir) {
            Ok(f) => f,
            Err(TreeError::NotFound) => return missing(config, &module, git_ref),
            Err(TreeError::Other(m)) => {
                return Err(EnvError::invalid_config(format!(
                    "cannot read {path} at {git_ref}: {m}"
                )));
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => return missing(config, &module, git_ref),
        Err(e) => {
            return Err(EnvError::invalid_config(format!(
                "cannot read {path} at {git_ref}: {e}"
            )));
        }
    };
    if files.is_empty() || !files.iter().any(|f| f.path.ends_with(".tf")) {
        if !required {
            return Ok(None);
        }
        return Err(EnvError::invalid_config(format!(
            "{path}/ at {git_ref} has no OpenTofu (.tf) files"
        )));
    }
    let scope = config
        .environment
        .as_ref()
        .and_then(|e| e.scope)
        .unwrap_or(EnvironmentScope::Full);
    if *kind == SandboxEnvironmentKind::Drafted {
        let mut input = ValidateEnvironmentInput::new(
            scope,
            files
                .iter()
                .map(|f| EnvironmentFile::new(format!("{path}/{}", f.path), f.content.clone()))
                .collect(),
        );
        input.module_only = true;
        let v = validate_environment(&input);
        let failed: Vec<_> = v
            .checks
            .iter()
            .filter(|c| !c.ok && c.id != "cost" && c.id != "scope")
            .collect();
        if !failed.is_empty() {
            let list: Vec<String> = failed
                .iter()
                .map(|c| format!("  - {}: {}", c.label, c.detail))
                .collect();
            return Err(EnvError::invalid_config(format!(
                "the environment at {path}/ ({git_ref}) does not satisfy the sandbox contract:\n{}",
                list.join("\n")
            )));
        }
    }
    let mut hashed: Vec<(&str, &str)> = files
        .iter()
        .map(|f| (f.path.as_str(), f.content.as_str()))
        .collect();
    let scope_str = scope.as_str();
    hashed.push(("#scope", scope_str));
    let hash = hash_environment_files(hashed);
    Ok(Some(SandboxEnvironment {
        kind: *kind,
        scope,
        path: path.clone(),
        commit: commit.to_owned(),
        hash,
        files,
    }))
}

fn missing(
    config: &RatchetConfig,
    module: &EnvironmentModuleRef,
    git_ref: &str,
) -> EnvResult<Option<SandboxEnvironment>> {
    if !module.required {
        return Ok(None);
    }
    let what = if module.kind == SandboxEnvironmentKind::Custom {
        "infra".to_owned()
    } else {
        format!(
            "environment.scope {}",
            config
                .environment
                .as_ref()
                .and_then(|e| e.scope)
                .map(|s| s.to_string())
                .unwrap_or_default()
        )
    };
    Err(EnvError::invalid_config(format!(
        "{what} needs {}/ on {git_ref}, which does not exist there. Approve the environment (commit or merge its pull request) first.",
        module.path
    )))
}

#[allow(dead_code)]
const _: &str = DEFAULT_ENVIRONMENT_INFRA_DIR;
