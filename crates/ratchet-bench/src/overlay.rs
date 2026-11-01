//! Protected-path overlay (local runner). Port of `overlay.ts`.
//!
//! Rule: a path is *protected* iff it matches one of the config's `protected` globs (core's `matches_glob`). Protected paths
//! ALWAYS come from `bench.ref`, never from the candidate:
//!
//!  1. every file / symlink in the bench.ref tree that is protected is copied over the candidate checkout, replacing
//!     whatever the candidate has at that path (files, symlinks, directories);
//!  2. with `prune` (default), every protected file / symlink the candidate has that bench.ref does not is deleted (so a
//!     candidate cannot add `bench/extra.js` for a directory-scanning bench). This assumes bench.ref is a full snapshot (the
//!     baseline plus the trusted bench).
//!
//! The copy never follows symlinks: any ancestor of a target that is a symlink or a regular file in the candidate tree is
//! removed and replaced by a real directory first. `.git` and `node_modules` are never walked. Mode bits are preserved,
//! ownership is not.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use ratchet_core::matches_glob;

use crate::errors::{BenchError, BenchResult};
use crate::shell::js_sort;

const SKIP_DIRS: [&str; 2] = [".git", "node_modules"];

pub fn is_protected<S: AsRef<str>>(path: &str, globs: &[S]) -> bool {
    globs.iter().any(|g| matches_glob(path, g.as_ref()))
}

fn io_err(what: &str, p: &Path, e: io::Error) -> BenchError {
    BenchError::infra(format!("{what} {}: {e}", p.display()))
}

/// Relative POSIX paths of every file and symlink under `root` (directories are not listed, symlinks are not followed).
pub fn list_files(root: &Path) -> BenchResult<Vec<String>> {
    let mut out = Vec::new();
    walk(root, "", &mut out)?;
    js_sort(&mut out);
    Ok(out)
}

fn walk(root: &Path, rel: &str, out: &mut Vec<String>) -> BenchResult<()> {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    for e in fs::read_dir(&dir).map_err(|e| io_err("cannot read", &dir, e))? {
        let e = e.map_err(|e| io_err("cannot read", &dir, e))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(root, &r, out)?;
            }
        } else {
            out.push(r);
        }
    }
    Ok(())
}

/// `rm -rf` of whatever is at `p` (a file, a symlink, or a directory), without following symlinks.
fn remove_any(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn ensure_real_parents(root: &Path, rel: &str) -> BenchResult<()> {
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    let mut cur = root.to_path_buf();
    for part in parts {
        cur.push(part);
        let st = fs::symlink_metadata(&cur).ok();
        if let Some(st) = &st
            && !st.is_dir()
        {
            remove_any(&cur).map_err(|e| io_err("cannot remove", &cur, e))?; // symlink or file in the way
        }
        if st.as_ref().is_none_or(|s| !s.is_dir()) {
            fs::create_dir(&cur).map_err(|e| io_err("cannot create", &cur, e))?;
        }
    }
    Ok(())
}

fn copy_entry(src_root: &Path, dst_root: &Path, rel: &str) -> BenchResult<()> {
    let src = src_root.join(rel);
    let dst = dst_root.join(rel);
    ensure_real_parents(dst_root, rel)?;
    remove_any(&dst).map_err(|e| io_err("cannot remove", &dst, e))?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err("cannot create", parent, e))?;
    }
    let st = fs::symlink_metadata(&src).map_err(|e| io_err("cannot stat", &src, e))?;
    if st.file_type().is_symlink() {
        let target = fs::read_link(&src).map_err(|e| io_err("cannot read link", &src, e))?;
        symlink(target, &dst).map_err(|e| io_err("cannot link", &dst, e))?;
    } else {
        fs::copy(&src, &dst).map_err(|e| io_err("cannot copy", &src, e))?;
        fs::set_permissions(&dst, fs::Permissions::from_mode(st.mode() & 0o777))
            .map_err(|e| io_err("cannot chmod", &dst, e))?;
    }
    Ok(())
}

fn same_entry(a: &Path, b: &Path) -> bool {
    let (Ok(sa), Ok(sb)) = (fs::symlink_metadata(a), fs::symlink_metadata(b)) else {
        return false;
    };
    if sa.file_type().is_symlink() || sb.file_type().is_symlink() {
        return sa.file_type().is_symlink()
            && sb.file_type().is_symlink()
            && fs::read_link(a).ok() == fs::read_link(b).ok();
    }
    if !sb.is_file() || sa.len() != sb.len() {
        return false;
    }
    match (fs::read(a), fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayResult {
    /// Protected paths taken from bench.ref (sorted).
    pub copied: Vec<String>,
    /// Candidate-only protected paths that were deleted (sorted).
    pub removed: Vec<String>,
}

pub fn apply_overlay<S: AsRef<str>>(
    code_dir: &Path,
    bench_dir: &Path,
    globs: &[S],
    prune: bool,
) -> BenchResult<OverlayResult> {
    let copied: Vec<String> = list_files(bench_dir)?
        .into_iter()
        .filter(|p| is_protected(p, globs))
        .collect();
    let mut removed = Vec::new();
    if prune {
        let in_bench: BTreeSet<&String> = copied.iter().collect();
        for p in list_files(code_dir)?
            .into_iter()
            .filter(|p| is_protected(p, globs))
        {
            let path = code_dir.join(&p);
            remove_any(&path).map_err(|e| io_err("cannot remove", &path, e))?;
            if !in_bench.contains(&p) {
                removed.push(p);
            }
        }
    }
    for p in &copied {
        copy_entry(bench_dir, code_dir, p)?;
    }
    Ok(OverlayResult { copied, removed })
}

/// Compare the protected files of the code tree with bench.ref's, restore any that differ and return their paths. Non-empty
/// means an earlier step (build, tests, the bench itself) modified trusted files.
pub fn restore_protected(
    code_dir: &Path,
    bench_dir: &Path,
    files: &[String],
) -> BenchResult<Vec<String>> {
    let mut restored = Vec::new();
    for p in files {
        // an ancestor may have been swapped for a symlink; lstat of the final path would follow it
        if !same_entry_through_real_parents(bench_dir, code_dir, p) {
            copy_entry(bench_dir, code_dir, p)?;
            restored.push(p.clone());
        }
    }
    Ok(restored)
}

fn same_entry_through_real_parents(src_root: &Path, dst_root: &Path, rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    let mut cur = dst_root.to_path_buf();
    for part in parts {
        cur.push(part);
        match fs::symlink_metadata(&cur) {
            Ok(st) if st.is_dir() => {}
            _ => return false,
        }
    }
    same_entry(&src_root.join(rel), &dst_root.join(rel))
}
