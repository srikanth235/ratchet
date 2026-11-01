//! The repo's container (`environment.image`) as the bench's toolchain (port of `image.ts`).
//!
//! The bench instances are offline and generic (Docker engine, SSM, zstd, the users). The toolchain a repo needs (node,
//! python, k6, a compiler, ...) therefore arrives as a CONTAINER IMAGE that the repo describes with a Dockerfile or a
//! `devcontainer.json`. It is handled exactly like the dependency snapshot:
//!
//!  - built ONCE from the protected ref (`bench.ref`) by a job on an instance that has egress and applies no candidate
//!    input;
//!  - saved (`docker save | zstd`) to the artifacts bucket under a CONTENT-ADDRESSED key and described by a manifest that is
//!    written last (a manifest means a complete archive);
//!  - reused by every candidate and every optimization with the same key;
//!  - loaded offline on bench / confirm instances by root, after verifying its sha256 and its image id.
//!
//! A candidate that changes the Dockerfile / devcontainer or a file the build copies in is BLOCKED ("environment changes
//! are not allowed in v0"), consistent with the lockfile rule.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use indexmap::IndexMap;
use ratchet_core::{GuardrailResult, RatchetConfig};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::deps::{
    Compression, FileHashes, hash_named_files, is_sha256_hex, same_hashes, sha256_str, sorted_map,
};
use crate::errors::{BenchError, BenchResult};
use crate::posix::{self, js_trim, js_trim_end, js_trim_start};
use crate::shell::{js_cmp, js_sort, json_str};

/// Prefix in the artifacts bucket for saved images. Only the deps identity writes here; the bench identities read.
pub const IMAGE_PREFIX: &str = "bench-images/";
/// Bumped when build / save / restore change in a way that makes old images wrong: part of every key.
pub const IMAGE_REV: u32 = 1;

static IMAGE_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^img-[0-9a-f]{40}$").expect("static regex"));

/// Is `key` a plain image key (`img-<40 hex>`)?
pub fn is_image_key(key: &str) -> bool {
    IMAGE_KEY_RE.is_match(key)
}

/// Tag the image gets in the docker daemon of the job's instance. Content-addressed, so a stale image can never be
/// mistaken for the current one.
pub fn image_tag(key: &str) -> String {
    format!("ratchet-env:{key}")
}

/// `bench-images/<key>.json`: written LAST by the image job, so its presence means the archive is complete.
pub fn image_manifest_key(key: &str) -> String {
    format!("{IMAGE_PREFIX}{key}.json")
}

/// The part of `environment:` the bench reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentSettings {
    /// Repo-relative path of a Dockerfile or a devcontainer.json.
    pub image: Option<String>,
    pub machine: Option<String>,
    pub region: Option<String>,
    pub network: Option<Vec<String>>,
}

/// Defensive read of an `environment:` value (JSON): anything that is not the documented shape is ignored, except an
/// `image` that is present but not a safe repo-relative path, which is a loud configuration error because silently
/// falling back to the bare host would benchmark the wrong toolchain. (`read_environment` is the typed variant.)
pub fn read_environment_value(config: &Value) -> BenchResult<EnvironmentSettings> {
    let Some(env) = config.get("environment").and_then(Value::as_object) else {
        return Ok(EnvironmentSettings::default());
    };
    let mut out = EnvironmentSettings::default();
    match env.get("image") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if !js_trim(s).is_empty() => {
            out.image = Some(normalize_repo_relative(s, "environment.image")?);
        }
        Some(_) => {
            return Err(BenchError::infra(
                "environment.image must be a path (a Dockerfile or a devcontainer.json) inside the repository",
            ));
        }
    }
    if let Some(Value::String(m)) = env.get("machine")
        && !m.is_empty()
    {
        out.machine = Some(m.clone());
    }
    if let Some(Value::String(r)) = env.get("region")
        && !r.is_empty()
    {
        out.region = Some(r.clone());
    }
    if let Some(Value::Array(n)) = env.get("network") {
        out.network = Some(
            n.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
        );
    }
    Ok(out)
}

/// `readEnvironment(config)` for a parsed `ratchet.yaml`.
pub fn read_environment(config: &RatchetConfig) -> BenchResult<EnvironmentSettings> {
    let env = serde_json::to_value(&config.environment).unwrap_or(Value::Null);
    read_environment_value(&json!({ "environment": env }))
}

/// `a/./b//c` -> `a/b/c`; rejects absolute paths, `..` and NUL. The result never starts with `./`.
pub fn normalize_repo_relative(p: &str, what: &str) -> BenchResult<String> {
    if p.contains('\0') {
        return Err(BenchError::infra(format!("{what} contains a NUL byte")));
    }
    let slashed = p.replace('\\', "/");
    if slashed.starts_with('/') {
        return Err(BenchError::infra(format!(
            "{what} must be a path inside the repository, not an absolute path ({})",
            json_str(p)
        )));
    }
    let mut n = posix::normalize(&slashed);
    while let Some(rest) = n.strip_prefix("./") {
        n = rest.to_owned();
    }
    let n = n.trim_end_matches('/').to_owned();
    if n == ".." || n.starts_with("../") {
        return Err(BenchError::infra(format!(
            "{what} must stay inside the repository ({})",
            json_str(p)
        )));
    }
    Ok(if n.is_empty() { ".".into() } else { n })
}

// ---- JSONC and Dockerfile parsing -------------------------------------------------------------------------------------

/// Parses JSON with comments and trailing commas (devcontainer.json is JSONC).
pub fn parse_jsonc(text: &str) -> Result<Value, serde_json::Error> {
    let chars: Vec<char> = text.chars().collect();
    let mut no_comments = String::new();
    let mut in_str = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            no_comments.push(c);
            if c == '\\' {
                i += 1;
                if let Some(n) = chars.get(i) {
                    no_comments.push(*n);
                }
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            no_comments.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            no_comments.push('\n');
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            // find "*/" from i + 2
            let mut j = i + 2;
            let mut end = None;
            while j + 1 < chars.len() {
                if chars[j] == '*' && chars[j + 1] == '/' {
                    end = Some(j);
                    break;
                }
                j += 1;
            }
            i = match end {
                Some(e) => e + 1,
                None => chars.len(),
            };
        } else {
            no_comments.push(c);
        }
        i += 1;
    }
    let nc: Vec<char> = no_comments.chars().collect();
    let mut out = String::new();
    in_str = false;
    let mut i = 0;
    while i < nc.len() {
        let c = nc[i];
        if in_str {
            out.push(c);
            if c == '\\' {
                i += 1;
                if let Some(n) = nc.get(i) {
                    out.push(*n);
                }
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == ',' {
            let rest: String = nc[i + 1..].iter().collect();
            let rest = js_trim_start(&rest);
            if !(rest.starts_with('}') || rest.starts_with(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    serde_json::from_str(&out)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DockerfileInfo {
    /// Base images of every FROM that names a registry image (not `scratch`, not an earlier stage), in order.
    pub base_images: Vec<String>,
    /// Sources of COPY / ADD instructions that read the BUILD CONTEXT (not `--from=`, not URLs, not heredocs).
    pub copy_sources: Vec<String>,
}

/// Splits an instruction into words like a shell would for COPY (quotes grouped, no expansion).
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        if let Some(quote) = q {
            if c == quote {
                q = None;
            } else {
                cur.push(c);
            }
        } else if c == '"' || c == '\'' {
            q = Some(c);
            any = true;
        } else if posix::is_js_space(c) {
            if !cur.is_empty() || any {
                out.push(std::mem::take(&mut cur));
            }
            any = false;
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

/// The two things the bench needs from a Dockerfile: which images it builds on, and which context files it copies in.
pub fn parse_dockerfile(text: &str) -> DockerfileInfo {
    let text = text.replace("\r\n", "\n");
    let mut instructions: Vec<String> = Vec::new();
    let mut cur = String::new();
    for raw in text.split('\n') {
        let line = js_trim_end(raw);
        if cur.is_empty() && (js_trim_start(line).starts_with('#') || js_trim(line).is_empty()) {
            continue;
        }
        if !cur.is_empty() && js_trim_start(line).starts_with('#') {
            continue; // a comment line inside a continued instruction
        }
        if let Some(stripped) = line.strip_suffix('\\') {
            cur.push_str(stripped);
            cur.push(' ');
        } else {
            instructions.push(js_trim(&format!("{cur}{line}")).to_owned());
            cur.clear();
        }
    }
    if !js_trim(&cur).is_empty() {
        instructions.push(js_trim(&cur).to_owned());
    }

    let mut stages: BTreeSet<String> = BTreeSet::new();
    let mut info = DockerfileInfo::default();
    for ins in &instructions {
        // /^(\S+)\s*(.*)$/s
        let split = ins.find(posix::is_js_space).unwrap_or(ins.len());
        if split == 0 {
            continue;
        }
        let op = ins[..split].to_uppercase();
        let rest = js_trim_start(&ins[split..]);
        if op == "FROM" {
            let w: Vec<String> = words(rest)
                .into_iter()
                .filter(|x| !x.starts_with("--"))
                .collect();
            if let Some(image) = w.first()
                && image.to_lowercase() != "scratch"
                && !stages.contains(&image.to_lowercase())
            {
                info.base_images.push(image.clone());
            }
            if let Some(as_at) = w.iter().position(|x| x.to_uppercase() == "AS")
                && let Some(name) = w.get(as_at + 1)
                && !name.is_empty()
            {
                stages.insert(name.to_lowercase());
            }
        } else if op == "COPY" || op == "ADD" {
            let flags: Vec<String> = words(rest)
                .into_iter()
                .filter(|x| x.starts_with("--"))
                .collect();
            if flags
                .iter()
                .any(|f| f.to_lowercase().starts_with("--from="))
            {
                continue;
            }
            let plain = || -> Vec<String> {
                words(rest)
                    .into_iter()
                    .filter(|x| !x.starts_with("--"))
                    .collect()
            };
            let args: Vec<String> = if js_trim_start(rest).starts_with('[') {
                // JSON.parse(rest.trimStart().replace(/^(--\S+\s+)+/, ''))
                let stripped = strip_leading_flags(js_trim_start(rest));
                match serde_json::from_str::<Value>(stripped) {
                    Ok(Value::Array(a)) => a.iter().map(js_string).collect(),
                    Ok(_) => Vec::new(),
                    Err(_) => plain(),
                }
            } else {
                plain()
            };
            if args.iter().any(|a| a.starts_with("<<")) {
                continue; // heredoc: the content is in the Dockerfile itself
            }
            for src in args.iter().take(args.len().saturating_sub(1)) {
                if is_url_like(src) || src.starts_with("git@") {
                    continue; // ADD <url>: fetched at build time
                }
                info.copy_sources.push(src.clone());
            }
        }
    }
    info
}

/// `s.replace(/^(--\S+\s+)+/, '')`.
fn strip_leading_flags(s: &str) -> &str {
    let mut rest = s;
    loop {
        if !rest.starts_with("--") {
            return rest;
        }
        let after_flag = rest
            .find(posix::is_js_space)
            .map(|i| &rest[i..])
            .unwrap_or("");
        let trimmed = js_trim_start(after_flag);
        // the group needs at least one whitespace after `--\S+`
        if after_flag.len() == trimmed.len() {
            return rest;
        }
        rest = trimmed;
    }
}

/// `String(x)` for a JSON value.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            ratchet_core::numfmt::js_number_to_string(n.as_f64().unwrap_or(f64::NAN))
        }
        other => other.to_string(),
    }
}

static URL_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://").expect("static regex"));

fn is_url_like(s: &str) -> bool {
    URL_LIKE.is_match(s)
}

static DIGEST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@(sha256:[0-9a-f]{64})$").expect("static regex"));

/// Image references pinned by digest (`name@sha256:...`).
pub fn pinned_digest(r: &str) -> Option<String> {
    DIGEST_RE
        .captures(r)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
}

// ---- resolving `environment.image` against a tree -------------------------------------------------------------------------

/// Cap on the number of files that make up the "environment" of one repo. A Dockerfile that copies the whole repo in is
/// refused long before this.
pub const MAX_ENV_FILES: usize = 2000;

static BUILD_ARG_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("static regex"));
static BUILD_ARG_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._/:@+=,~-]{0,200}$").expect("static regex"));
static IMAGE_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/:@-]{0,255}$").expect("static regex"));
static TARGET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]{1,64}$").expect("static regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageKind {
    Dockerfile,
    Image,
}

impl ImageKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ImageKind::Dockerfile => "dockerfile",
            ImageKind::Image => "image",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSpec {
    pub kind: ImageKind,
    /// `environment.image` as the repo wrote it (normalized).
    pub path: String,
    /// Root-relative Dockerfile (kind `dockerfile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
    /// Root-relative build context directory (kind `dockerfile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub args: IndexMap<String, String>,
    /// A prebuilt registry image to pull instead of building (devcontainer `image`, kind `image`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Registry images the build starts from, with the digest-pinned ones identified.
    #[serde(default)]
    pub base_images: Vec<String>,
    #[serde(default)]
    pub base_digests: Vec<String>,
    /// Plain-language remarks for the log: unpinned base images, ignored devcontainer properties.
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImage {
    pub spec: ImageSpec,
    /// sha256 per root-relative file that can influence the built image: the Dockerfile, the devcontainer.json, the
    /// context's .dockerignore and every context file the Dockerfile COPYs / ADDs. A candidate whose map differs is
    /// blocked.
    pub files: FileHashes,
}

fn read_regular(tree: &Path, rel: &str, what: &str) -> BenchResult<String> {
    let p = tree.join(rel);
    let st = fs::symlink_metadata(&p)
        .map_err(|_| BenchError::infra(format!("{what} {rel} does not exist in the repository")))?;
    if !st.is_file() {
        return Err(BenchError::infra(format!(
            "{what} {rel} must be a regular file (not a symlink or directory)"
        )));
    }
    let bytes = fs::read(&p).map_err(|e| BenchError::infra(format!("{what} {rel}: {e}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// All regular files and symlinks under `rel` (a file, or a directory walked without following links), `.git` excluded.
fn files_under(tree: &Path, rel: &str, budget: &mut i64) -> BenchResult<Vec<String>> {
    let Ok(st) = fs::symlink_metadata(tree.join(rel)) else {
        return Ok(Vec::new());
    };
    if !st.is_dir() {
        return Ok(vec![rel.to_owned()]);
    }
    let mut out = Vec::new();
    walk_env(tree, rel, budget, &mut out)?;
    Ok(out)
}

fn walk_env(tree: &Path, dir: &str, budget: &mut i64, out: &mut Vec<String>) -> BenchResult<()> {
    let rd = fs::read_dir(tree.join(dir)).map_err(|e| BenchError::infra(format!("{dir}: {e}")))?;
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == ".git" && dir == "." {
            continue;
        }
        let r = if dir == "." {
            name
        } else {
            format!("{dir}/{name}")
        };
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            walk_env(tree, &r, budget, out)?;
        } else {
            *budget -= 1;
            if *budget < 0 {
                return Err(BenchError::infra(format!(
                    "the Dockerfile copies more than {MAX_ENV_FILES} files from the build context; keep the environment small (a toolchain-only context such as .devcontainer/)"
                )));
            }
            out.push(r);
        }
    }
    Ok(())
}

fn build_string_map(raw: Option<&Value>, what: &str) -> BenchResult<IndexMap<String, String>> {
    let mut out = IndexMap::new();
    let Some(raw) = raw else { return Ok(out) };
    let Value::Object(map) = raw else {
        return Err(BenchError::infra(format!(
            "{what} must be an object of strings"
        )));
    };
    for (k, v) in map {
        let ok =
            BUILD_ARG_NAME.is_match(k) && v.as_str().is_some_and(|s| BUILD_ARG_VALUE.is_match(s));
        if !ok {
            return Err(BenchError::infra(format!(
                "{what}.{k} is not a plain build argument (names [A-Za-z_][A-Za-z0-9_]*, values of letters, digits and . _ / : @ + = , ~ -)"
            )));
        }
        out.insert(k.clone(), v.as_str().unwrap_or_default().to_owned());
    }
    Ok(out)
}

/// Reads `environment.image` from a checked-out tree (the base ref on the controller, or the candidate's tree to compare)
/// and returns what to build and the hashes of everything that influences the result. Fails with a `BenchError` for
/// configurations the bench cannot honor.
pub fn resolve_image_source(tree: &Path, image_path: &str) -> BenchResult<ResolvedImage> {
    let path = normalize_repo_relative(image_path, "environment.image")?;
    let base = posix::basename(&path);
    let is_devcontainer = base.to_lowercase().ends_with(".json");
    let mut notes: Vec<String> = Vec::new();
    let mut args: IndexMap<String, String> = IndexMap::new();
    let dockerfile: String;
    let context: String;
    let mut target: Option<String> = None;
    let mut hash_names: Vec<String> = vec![path.clone()];

    if is_devcontainer {
        let text = read_regular(tree, &path, "environment.image")?;
        let dc = match parse_jsonc(&text) {
            Ok(Value::Object(m)) => m,
            Ok(_) => {
                return Err(BenchError::infra(format!(
                    "{path} is not valid JSON (comments and trailing commas are allowed): not an object"
                )));
            }
            Err(e) => {
                return Err(BenchError::infra(format!(
                    "{path} is not valid JSON (comments and trailing commas are allowed): {e}"
                )));
            }
        };
        if dc.get("dockerComposeFile").is_some() {
            return Err(BenchError::infra(format!(
                "{path}: dockerComposeFile is not supported; point environment.image at a Dockerfile or a devcontainer with \"build\" or \"image\""
            )));
        }
        let dir = posix::dirname(&path);
        let join_ = |p: &str, what: &str| -> BenchResult<String> {
            let d = if dir == "." { "" } else { dir.as_str() };
            normalize_repo_relative(&posix::join(&[d, p]), what)
        };
        let empty = Map::new();
        let build = match dc.get("build") {
            Some(Value::Object(m)) => m,
            _ => &empty,
        };
        let df = build
            .get("dockerfile")
            .and_then(Value::as_str)
            .or_else(|| dc.get("dockerFile").and_then(Value::as_str));
        let Some(df) = df else {
            let image = dc.get("image").and_then(Value::as_str);
            let Some(image) = image.filter(|i| IMAGE_REF.is_match(i)) else {
                return Err(BenchError::infra(format!(
                    "{path} needs \"build.dockerfile\" (a Dockerfile in the repository) or \"image\" (a registry image) to describe the toolchain"
                )));
            };
            let digest = pinned_digest(image);
            if digest.is_none() {
                notes.push(format!(
                    "image {image} is not pinned by digest: the saved image is that tag as pulled when it was built (rebuilt only when {path} changes)"
                ));
            }
            ignored_note(&dc, &mut notes);
            return Ok(ResolvedImage {
                spec: ImageSpec {
                    kind: ImageKind::Image,
                    path,
                    dockerfile: None,
                    context: None,
                    target: None,
                    args: IndexMap::new(),
                    image: Some(image.to_owned()),
                    base_images: vec![image.to_owned()],
                    base_digests: digest.into_iter().collect(),
                    notes,
                },
                files: hash_named_files(tree, &hash_names)?,
            });
        };
        dockerfile = join_(df, "build.dockerfile")?;
        context = join_(
            build.get("context").and_then(Value::as_str).unwrap_or("."),
            "build.context",
        )?;
        args.extend(build_string_map(build.get("args"), "build.args")?);
        if let Some(t) = build.get("target") {
            match t.as_str() {
                Some(s) if TARGET_RE.is_match(s) => target = Some(s.to_owned()),
                _ => return Err(BenchError::infra("build.target must be a plain stage name")),
            }
        }
        ignored_note(&dc, &mut notes);
    } else {
        dockerfile = path.clone();
        // Same as `docker build -f <path> <dir of the Dockerfile>`; use a devcontainer.json to choose another context.
        context = posix::dirname(&path);
    }

    let df = read_regular(tree, &dockerfile, "the Dockerfile")?;
    if dockerfile != path {
        hash_names.push(dockerfile.clone());
    }
    let info = parse_dockerfile(&df);
    if info.base_images.is_empty() && !has_from_scratch(&df) {
        return Err(BenchError::infra(format!(
            "{dockerfile} has no FROM instruction"
        )));
    }
    for r in &info.base_images {
        // `FROM ${BASE}` and friends are resolved by the build; they cannot be pinned or checked here.
        if !r.contains('$') && !IMAGE_REF.is_match(r) {
            return Err(BenchError::infra(format!(
                "{dockerfile}: base image {} is not a plain image reference",
                json_str(r)
            )));
        }
    }
    let base_digests: Vec<String> = info
        .base_images
        .iter()
        .filter_map(|r| pinned_digest(r))
        .collect();
    let unpinned: Vec<&String> = info
        .base_images
        .iter()
        .filter(|r| pinned_digest(r).is_none())
        .collect();
    if !unpinned.is_empty() {
        notes.push(format!(
            "base image {} is not pinned by digest: rebuilds are keyed on the Dockerfile only, so the saved image can lag behind upstream (pin with @sha256:... for reproducibility)",
            unpinned.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }

    // .dockerignore changes what COPY sees.
    hash_names.push(format!("{dockerfile}.dockerignore"));
    hash_names.push(if context == "." {
        ".dockerignore".into()
    } else {
        format!("{context}/.dockerignore")
    });

    // Every context file the Dockerfile copies in. `COPY . /app` would put the candidate's own source into the image:
    // refused.
    let mut budget = MAX_ENV_FILES as i64;
    let whole_context = info.copy_sources.iter().any(|s| s.contains('$'));
    let sources: Vec<String> = if whole_context {
        vec![".".into()]
    } else {
        info.copy_sources.clone()
    };
    if whole_context {
        notes.push(
            "a COPY source uses a variable, so every file of the build context is treated as part of the environment"
                .into(),
        );
    }
    let ctx_prefix = if context == "." {
        String::new()
    } else {
        format!("{context}/")
    };
    let mut listing: Option<Vec<String>> = None;
    for src in &sources {
        let rel = normalize_repo_relative(
            src.trim_start_matches('/'),
            &format!("COPY source {}", json_str(src)),
        )?;
        let joined = format!("{ctx_prefix}{}", if rel == "." { "" } else { rel.as_str() });
        let full =
            normalize_repo_relative(if joined.is_empty() { "." } else { &joined }, "COPY source")?;
        if !rel.contains(['*', '?', '[', '{']) {
            // A literal path: a file, or a directory (all of it). A missing source fails the build, which reports it.
            hash_names.extend(files_under(tree, &full, &mut budget)?);
        } else {
            if listing.is_none() {
                let mut big = 200_000_i64;
                listing = Some(files_under(tree, &context, &mut big)?);
            }
            let re = ratchet_core::protected::glob_to_regexp(&rel);
            let mut hits = 0;
            for f in listing.iter().flatten() {
                let local = if ctx_prefix.is_empty() {
                    f.as_str()
                } else {
                    f.get(ctx_prefix.len()..).unwrap_or("")
                };
                let parts: Vec<&str> = local.split('/').collect();
                for n in 1..=parts.len() {
                    if re.is_match(&parts[..n].join("/")) {
                        hash_names.push(f.clone());
                        hits += 1;
                        break;
                    }
                }
            }
            if hits > MAX_ENV_FILES {
                return Err(BenchError::infra(format!(
                    "COPY {src} matches more than {MAX_ENV_FILES} files; keep the environment small"
                )));
            }
        }
    }
    let mut names: Vec<String> = {
        let mut seen = BTreeSet::new();
        hash_names
            .into_iter()
            .filter(|n| seen.insert(n.clone()))
            .collect()
    };
    js_sort(&mut names);
    if names.iter().any(|n| n == "ratchet.yaml") {
        return Err(BenchError::infra(format!(
            "{dockerfile} copies ratchet.yaml (or the whole repository) into the image; the environment must not contain the code under optimization. Keep the build context to toolchain files (for example .devcontainer/), or COPY only a manifest such as a lockfile"
        )));
    }
    if names.len() > MAX_ENV_FILES {
        return Err(BenchError::infra(format!(
            "the environment would consist of more than {MAX_ENV_FILES} files; keep the build context small"
        )));
    }
    let files = hash_named_files(tree, &names)?;
    if !files.contains_key(&dockerfile) {
        return Err(BenchError::infra(format!(
            "{dockerfile} is not a readable file"
        )));
    }
    Ok(ResolvedImage {
        spec: ImageSpec {
            kind: ImageKind::Dockerfile,
            path,
            dockerfile: Some(dockerfile),
            context: Some(context),
            target,
            args,
            image: None,
            base_images: info.base_images,
            base_digests,
            notes,
        },
        files,
    })
}

/// `/^\s*FROM\s+scratch/im`.
fn has_from_scratch(df: &str) -> bool {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?im)^\s*FROM\s+scratch").expect("static regex"));
    RE.is_match(df)
}

/// devcontainer.json properties the bench does not honor (the image is built, nothing else runs in it).
fn ignored_note(dc: &Map<String, Value>, notes: &mut Vec<String>) {
    const KEYS: [&str; 17] = [
        "features",
        "postCreateCommand",
        "onCreateCommand",
        "updateContentCommand",
        "postStartCommand",
        "postAttachCommand",
        "initializeCommand",
        "containerEnv",
        "remoteEnv",
        "mounts",
        "runArgs",
        "privileged",
        "capAdd",
        "securityOpt",
        "forwardPorts",
        "remoteUser",
        "containerUser",
    ];
    // (workspaceMount, workspaceFolder follow)
    let all = KEYS
        .iter()
        .chain(["workspaceMount", "workspaceFolder"].iter());
    let present: Vec<&str> = all.copied().filter(|k| dc.get(*k).is_some()).collect();
    if !present.is_empty() {
        notes.push(format!(
            "devcontainer properties ignored by the bench (it builds the image only, runs offline and unprivileged): {}",
            present.join(", ")
        ));
    }
}

// ---- keys, manifests ------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageKeyInput {
    pub spec: ImageSpec,
    pub files: FileHashes,
    /// `arch_from_instance_type` of the bench hardware.
    pub arch: String,
}

fn sorted_args(m: &IndexMap<String, String>) -> Map<String, Value> {
    let mut entries: Vec<(&String, &String)> = m.iter().collect();
    entries.sort_by(|a, b| js_cmp(a.0, b.0));
    entries
        .into_iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect()
}

fn opt(s: &Option<String>) -> Value {
    s.as_ref().map_or(Value::Null, |v| Value::String(v.clone()))
}

/// One image per key: what is built (Dockerfile / devcontainer fields), the hashes of everything it copies in, the pinned
/// base digests and the architecture. Reused by every candidate and every optimization that shares them.
pub fn image_key(i: &ImageKeyInput) -> String {
    let s = &i.spec;
    let mut digests = s.base_digests.clone();
    js_sort(&mut digests);
    let canon = json!({
        "v": 1,
        "rev": IMAGE_REV,
        "kind": s.kind.as_str(),
        "dockerfile": opt(&s.dockerfile),
        "context": opt(&s.context),
        "target": opt(&s.target),
        "args": Value::Object(sorted_args(&s.args)),
        "image": opt(&s.image),
        "files": Value::Object(sorted_map(&i.files)),
        "baseDigests": digests,
        "arch": i.arch,
    });
    let text = serde_json::to_string(&canon).unwrap_or_default();
    format!("img-{}", &sha256_str(&text)[..40])
}

/// What the image job records about its archive. `archive` is content-addressed, like the dependency snapshots'.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageManifest {
    pub v: u8,
    pub key: String,
    pub rev: u32,
    pub kind: ImageKind,
    pub path: String,
    pub arch: String,
    pub files: FileHashes,
    pub base_digests: Vec<String>,
    pub tag: String,
    /// `docker image inspect --format '{{.Id}}'` on the builder; the bench compares it after `docker load`.
    pub image_id: String,
    pub compression: Compression,
    pub archive: String,
    pub sha256: String,
    pub bytes: u64,
    pub created_at: i64,
}

pub type ImageManifestCheck = Result<ImageManifest, String>;

static IMAGE_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^sha256:[0-9a-f]{64}$").expect("static regex"));

/// Validates a manifest read from the artifacts bucket against what the controller computed itself from the base ref.
pub fn parse_image_manifest(text: &str, expect: &ImageKeyInput, key: &str) -> ImageManifestCheck {
    let raw: Value = serde_json::from_str(text).map_err(|_| "manifest is not JSON".to_owned())?;
    let empty = Map::new();
    let m = match &raw {
        Value::Object(m) => m,
        Value::Array(_) => &empty,
        _ => return Err("manifest is not an object".into()),
    };
    let bad = |what: &str| Err(format!("manifest {what}"));
    if m.get("v").and_then(Value::as_f64) != Some(1.0) {
        return bad("has an unknown version");
    }
    if m.get("key").and_then(Value::as_str) != Some(key) {
        return bad("is for another key");
    }
    if m.get("rev").and_then(Value::as_f64) != Some(f64::from(IMAGE_REV)) {
        return bad("was built by another image-job revision");
    }
    if m.get("arch").and_then(Value::as_str) != Some(expect.arch.as_str()) {
        return bad("was built for another architecture");
    }
    let files_ok = match m.get("files") {
        None | Some(Value::Null) => expect.files.is_empty(),
        other => same_hashes(other, &expect.files),
    };
    if !files_ok {
        return bad("has different environment file hashes");
    }
    let tag = image_tag(key);
    if m.get("tag").and_then(Value::as_str) != Some(tag.as_str()) {
        return bad("has an unexpected tag");
    }
    let Some(image_id) = m
        .get("imageId")
        .and_then(Value::as_str)
        .filter(|s| IMAGE_ID_RE.is_match(s))
    else {
        return bad("has no valid image id");
    };
    let compression = match m.get("compression").and_then(Value::as_str) {
        Some("zstd") => Compression::Zstd,
        Some("gzip") => Compression::Gzip,
        _ => return bad("has an unknown compression"),
    };
    let Some(sha) = m
        .get("sha256")
        .and_then(Value::as_str)
        .filter(|s| is_sha256_hex(s))
    else {
        return bad("has no valid sha256");
    };
    let ext = match compression {
        Compression::Zstd => "tar.zst",
        Compression::Gzip => "tar.gz",
    };
    let want = format!("{IMAGE_PREFIX}{key}/{sha}.{ext}");
    if m.get("archive").and_then(Value::as_str) != Some(want.as_str()) {
        return bad("points outside its own prefix");
    }
    let bytes = match m.get("bytes").and_then(Value::as_f64) {
        Some(b) if b.is_finite() && b >= 0.0 => b,
        _ => return bad("has no valid size"),
    };
    Ok(ImageManifest {
        v: 1,
        key: key.to_owned(),
        rev: IMAGE_REV,
        kind: expect.spec.kind,
        path: expect.spec.path.clone(),
        arch: expect.arch.clone(),
        files: expect.files.clone(),
        base_digests: expect.spec.base_digests.clone(),
        tag,
        image_id: image_id.to_owned(),
        compression,
        archive: want,
        sha256: sha.to_owned(),
        bytes: bytes as u64,
        created_at: m
            .get("createdAt")
            .and_then(Value::as_f64)
            .map_or(0, |n| n as i64),
    })
}

/// What a bench job needs to load the image and what it compares the candidate against.
#[derive(Debug, Clone)]
pub struct ImagePlan {
    pub key: String,
    pub tag: String,
    pub spec: ImageSpec,
    /// Hashes of the environment files at the BASE commit.
    pub files: FileHashes,
    /// Root-relative paths that are pinned to bench.ref on the bench (the environment files).
    pub pinned: Vec<String>,
    pub base_commit: String,
    pub manifest: ImageManifest,
    /// True when the image already existed (no instance was launched for it).
    pub reused: bool,
}

// ---- the candidate may not change the environment ------------------------------------------------------------------------

const UNRESOLVABLE: &str = "!environment";

/// Hashes the environment of a candidate tree the way the base was hashed. A tree whose `environment.image` can no longer
/// be resolved (Dockerfile deleted or turned into a symlink) yields a marker entry, so it never equals the base.
pub fn environment_files_of(tree: &Path, image_path: &str) -> FileHashes {
    match resolve_image_source(tree, image_path) {
        Ok(r) => r.files,
        Err(e) => {
            let mut m = FileHashes::new();
            m.insert(
                UNRESOLVABLE.into(),
                format!("unresolvable:{}", sha256_str(&e.to_string())),
            );
            m
        }
    }
}

/// Same comparison as the lockfiles: files whose hash differs, or that exist on one side only. Sorted.
pub fn environment_changes(base: &FileHashes, candidate: &FileHashes) -> Vec<String> {
    let names: BTreeSet<&String> = base.keys().chain(candidate.keys()).collect();
    let mut out: Vec<String> = names
        .into_iter()
        .filter(|n| base.get(*n) != candidate.get(*n))
        .map(|n| {
            if n == UNRESOLVABLE {
                "the environment (Dockerfile / devcontainer) no longer resolves".to_owned()
            } else {
                n.clone()
            }
        })
        .collect();
    js_sort(&mut out);
    out
}

/// The reason a candidate is blocked when it changes the environment. Same text for every runner.
pub fn environment_change_guardrail(changed: &[String], bench_ref: &str) -> GuardrailResult {
    let shown = changed
        .iter()
        .take(10)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = if changed.len() > 10 { ", ..." } else { "" };
    GuardrailResult {
        name: "environment unchanged".into(),
        passed: false,
        detail: Some(format!(
            "environment changes are not allowed in v0: the container image is built once from {bench_ref} and the bench has no network ({shown}{more} differs from {bench_ref}). Revert the Dockerfile / devcontainer and the files it copies; the toolchain is that of the base ref."
        )),
    }
}

/// Hint appended to a failed setup when it ran in the repo's container.
pub fn container_setup_hint(bench_ref: &str) -> String {
    format!(
        "setup ran inside the container built from {bench_ref} with no network and a read-only root filesystem; tools missing from that image, writes outside the checkout, home and /tmp, and downloads fail here, and environment changes are not allowed in v0"
    )
}
