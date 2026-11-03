//! Advanced: `infra: <path>` in ratchet.yaml points at a custom OpenTofu module instead of the one that ships with the
//! app (infra/aws). OFF BY DEFAULT. The module is read from the PROTECTED ref only (agents edit the repo, and a module
//! runs with the user's cloud credentials), refused when it can run local commands, and its outputs are validated
//! against the sandbox contract (infra/contract.md) after every apply, so a module that does not satisfy the contract
//! fails loudly instead of weakening the isolation silently. (TS `sandbox/override.ts`)

use std::fmt;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::util::json_str;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfraOverrideError(pub String);

impl fmt::Display for InfraOverrideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InfraOverrideError {}

fn ov(msg: impl Into<String>) -> Error {
    InfraOverrideError(msg.into()).into()
}

/// POSIX `path.normalize` for a relative path: collapses `.`, empty segments and `..` where possible.
pub(crate) fn posix_normalize(p: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    if out.is_empty() {
        ".".to_string()
    } else {
        out.join("/")
    }
}

/// `config.infra` (defensive: any other shape is refused, absence is `None`). A repo-relative path otherwise.
///
/// TODO(schema): take the typed `config.infra` once ratchet-core declares it.
pub fn read_infra_override(config: &Value) -> Result<Option<String>> {
    let v = match config.get("infra") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.is_empty() => return Ok(None),
        Some(Value::String(s)) => s,
        Some(_) => {
            return Err(ov(
                "infra must be a path to an OpenTofu module inside the repository",
            ));
        }
    };
    let drive_letter =
        v.len() >= 2 && v.as_bytes()[0].is_ascii_alphabetic() && v.as_bytes()[1] == b':';
    if v.contains('\0') || v.starts_with('/') || drive_letter {
        return Err(ov(format!(
            "infra must be a path inside the repository, not {}",
            json_str(v)
        )));
    }
    let n = posix_normalize(&v.replace('\\', "/"));
    if n == ".." || n.starts_with("../") {
        return Err(ov(format!(
            "infra must stay inside the repository ({})",
            json_str(v)
        )));
    }
    if n == "." || n.is_empty() {
        return Err(ov(
            "infra must name a directory of the repository, not the repository root",
        ));
    }
    Ok(Some(n))
}

pub(crate) static LOCAL_EXEC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"provisioner\s+"(local-exec|remote-exec)"|data\s+"external"|\bexternal\s*=\s*\{|program\s*=\s*\["#).expect("static regex")
});

/// The two git operations the override needs.
#[async_trait]
pub trait GitSource: Send + Sync {
    /// Resolve a ref (branch, tag, sha) to a full commit sha.
    async fn resolve_ref(&self, git_ref: &str) -> Result<String>;
    /// Materialize `sha` (code) into `dir`.
    async fn checkout(&self, sha: &str, dir: &Path) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedInfra {
    pub module_dir: PathBuf,
    pub commit: String,
}

/// Checks `git_ref` out (the protected ref, resolved to a commit) and returns the module directory. Refuses a module that
/// is not a real directory inside the checkout (symlinks), has no .tf files, or can run commands on this machine during
/// plan / apply.
pub async fn materialize_infra_override(
    git: &dyn GitSource,
    git_ref: &str,
    path: &str,
    dest_root: &Path,
) -> Result<MaterializedInfra> {
    let commit = git.resolve_ref(git_ref).await?;
    // The commit names a directory: it must be a plain hex sha, never something that could climb out of `dest_root`.
    if commit.len() < 7 || commit.len() > 64 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ov(format!(
            "ref {git_ref} did not resolve to a commit sha ({})",
            json_str(&commit)
        )));
    }
    tokio::fs::create_dir_all(dest_root).await?;
    let root = dest_root.join(format!("infra-{}", &commit[..12.min(commit.len())]));
    git.checkout(&commit, &root).await?;
    let mut module_dir = root.clone();
    module_dir.extend(path.split('/'));
    // No symlink anywhere on the way: the module must be what the protected ref contains.
    let mut acc = root.clone();
    for part in path.split('/') {
        acc.push(part);
        let st = tokio::fs::symlink_metadata(&acc).await.ok();
        let Some(st) = st else {
            return Err(ov(format!("infra {path} does not exist at {git_ref}")));
        };
        if st.file_type().is_symlink() || !st.is_dir() {
            return Err(ov(format!(
                "infra {path} must be a plain directory at {git_ref} (no symlinks)"
            )));
        }
    }
    let real = tokio::fs::canonicalize(&module_dir).await?;
    let real_root = tokio::fs::canonicalize(&root).await?;
    let with_sep = |p: &Path| format!("{}{MAIN_SEPARATOR}", p.to_string_lossy());
    if !with_sep(&real).starts_with(&with_sep(&real_root)) {
        return Err(ov(format!("infra {path} resolves outside the repository")));
    }
    let mut tf: Vec<String> = vec![];
    let mut rd = tokio::fs::read_dir(&module_dir).await?;
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tf") {
            tf.push(name);
        }
    }
    tf.sort();
    if tf.is_empty() {
        return Err(ov(format!(
            "infra {path} at {git_ref} contains no .tf files"
        )));
    }
    for f in &tf {
        let text =
            String::from_utf8_lossy(&tokio::fs::read(module_dir.join(f)).await?).into_owned();
        if LOCAL_EXEC.is_match(&text) {
            return Err(ov(format!(
                "infra {path}/{f} can run commands on this machine (local-exec, remote-exec or an external data source); a custom sandbox module may only declare cloud resources"
            )));
        }
    }
    Ok(MaterializedInfra { module_dir, commit })
}

// ---- the sandbox contract, checked on the outputs ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractCheck {
    pub ok: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractCheckOptions {
    /// `deps_identity_ref` is optional in the contract (sandboxes from before dependency snapshots), but without it the
    /// app can neither build dependency snapshots nor container images. Default true: a custom module must provide it.
    pub require_deps: bool,
}

impl Default for ContractCheckOptions {
    fn default() -> Self {
        Self { require_deps: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractViolationError {
    pub errors: Vec<String>,
}

impl fmt::Display for ContractViolationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the custom sandbox module does not satisfy the sandbox contract (infra/contract.md):\n - {}\nFix the module so its outputs match the contract, or remove `infra:` from ratchet.yaml to use the module that ships with the app.",
            self.errors.join("\n - ")
        )
    }
}

impl std::error::Error for ContractViolationError {}

static BUCKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$").expect("static regex"));
static ROLE_ARN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^arn:aws[a-z-]*:iam::?[0-9]{12}:role/.+").expect("static regex"));
static PROFILE_ARN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^arn:aws[a-z-]*:iam::?[0-9]{12}:instance-profile/.+").expect("static regex")
});
static POLICY_ARN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^arn:aws[a-z-]*:iam::?[0-9]{12}:policy/.+").expect("static regex")
});
static VPC_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^vpc-[0-9a-f]+$").expect("static regex"));
static SUBNET_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^subnet-[0-9a-f]+$").expect("static regex"));
static SG_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^sg-[0-9a-f]+$").expect("static regex"));

struct Checker<'a> {
    doc: &'a Map<String, Value>,
    errors: Vec<String>,
}

impl<'a> Checker<'a> {
    /// `doc[name].value`, when `doc[name]` is an object.
    fn val(&self, name: &str) -> Option<&'a Value> {
        self.doc
            .get(name)
            .filter(|o| o.is_object())
            .and_then(|o| o.get("value"))
    }

    fn has(&self, name: &str) -> bool {
        self.doc
            .get(name)
            .is_some_and(|o| o.as_object().is_some_and(|m| m.contains_key("value")))
    }

    fn need(&mut self, name: &str) -> Option<&'a Value> {
        if !self.has(name) {
            self.errors.push(format!("output \"{name}\" is missing"));
        }
        self.val(name)
    }

    /// A non-empty string (matching `re`); problems are recorded. A missing top-level output was already reported by
    /// `need`; a missing field inside one is reported here.
    fn str(
        &mut self,
        name: &str,
        v: Option<&Value>,
        re: Option<&Regex>,
        what: &str,
    ) -> Option<String> {
        let Some(sv) = v.and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            if v.is_some() || name.contains('.') {
                self.errors
                    .push(format!("output \"{name}\" must be {what}"));
            }
            return None;
        };
        if let Some(re) = re
            && !re.is_match(sv)
        {
            self.errors.push(format!(
                "output \"{name}\" must be {what} (got {})",
                json_str(sv)
            ));
            return None;
        }
        Some(sv.to_string())
    }

    fn identity(&mut self, name: &str, v: Option<&Value>) -> Option<(String, String)> {
        let v = v?;
        let Some(obj) = v.as_object() else {
            self.errors.push(format!(
                "output \"{name}\" must be an object with role_arn, role_name, instance_profile_arn and instance_profile_name"
            ));
            return None;
        };
        let role = self.str(
            &format!("{name}.role_arn"),
            obj.get("role_arn"),
            Some(&ROLE_ARN),
            "an IAM role ARN",
        );
        self.str(
            &format!("{name}.role_name"),
            obj.get("role_name"),
            None,
            "a string",
        );
        let profile = self.str(
            &format!("{name}.instance_profile_arn"),
            obj.get("instance_profile_arn"),
            Some(&PROFILE_ARN),
            "an IAM instance profile ARN",
        );
        self.str(
            &format!("{name}.instance_profile_name"),
            obj.get("instance_profile_name"),
            None,
            "a string",
        );
        role.zip(profile)
    }
}

/// Validates `tofu output -json` (name -> {value}) against infra/contract.md: every output the app reads is present and
/// has the shape it expects, and the separations the invariants depend on hold (the three bench identities differ, the
/// sealed store is a different bucket from the artifacts store, bench machines are not in the agent subnet or group).
pub fn check_sandbox_contract(doc: &Value, opts: ContractCheckOptions) -> ContractCheck {
    let Some(doc) = doc.as_object() else {
        return ContractCheck {
            ok: false,
            errors: vec![
                "the module produced no outputs (tofu output -json is not an object)".to_string(),
            ],
        };
    };
    let mut c = Checker {
        doc,
        errors: vec![],
    };

    let v = c.need("artifacts_bucket");
    let artifacts = c.str("artifacts_bucket", v, Some(&BUCKET), "an S3 bucket name");
    let v = c.need("sealed_bucket");
    let sealed = c.str("sealed_bucket", v, Some(&BUCKET), "an S3 bucket name");
    if let (Some(a), Some(s)) = (&artifacts, &sealed)
        && a == s
    {
        c.errors.push(
            "artifacts_bucket and sealed_bucket must be different buckets (sealed isolation)"
                .to_string(),
        );
    }
    let v = c.need("agent_boundary_ref");
    c.str(
        "agent_boundary_ref",
        v,
        Some(&POLICY_ARN),
        "an IAM policy ARN",
    );
    let v = c.need("agent_role_path");
    if let Some(role_path) = c.str("agent_role_path", v, None, "a string")
        && !(role_path.starts_with('/') && role_path.ends_with('/'))
    {
        c.errors.push(format!(
            "output \"agent_role_path\" must start and end with \"/\" (got {})",
            json_str(&role_path)
        ));
    }
    let v = c.need("sealed_confirm_prefix");
    if let Some(prefix) = c.str("sealed_confirm_prefix", v, None, "a string")
        && (prefix.starts_with('/') || !prefix.ends_with('/'))
    {
        c.errors.push(
                "output \"sealed_confirm_prefix\" must be a key prefix ending in \"/\" (the only part of the sealed store the confirm identity reads)"
                    .to_string(),
            );
    }

    let v = c.need("eval_identity_ref");
    let eval_id = c.identity("eval_identity_ref", v);
    let v = c.need("confirm_identity_ref");
    let confirm_id = c.identity("confirm_identity_ref", v);
    let deps_raw = if c.has("deps_identity_ref") {
        c.val("deps_identity_ref")
    } else if opts.require_deps {
        c.need("deps_identity_ref")
    } else {
        None
    };
    let deps_id = c.identity("deps_identity_ref", deps_raw);
    let ids: Vec<(String, String)> = [eval_id, confirm_id, deps_id]
        .into_iter()
        .flatten()
        .collect();
    let distinct = |f: fn(&(String, String)) -> &String| {
        let mut seen: Vec<&String> = vec![];
        for i in &ids {
            if !seen.contains(&f(i)) {
                seen.push(f(i));
            }
        }
        seen.len() == ids.len()
    };
    if !(distinct(|i| &i.0) && distinct(|i| &i.1)) {
        c.errors.push(
            "eval_identity_ref, confirm_identity_ref and deps_identity_ref must be three different identities (least identity per bench phase)"
                .to_string(),
        );
    }

    if let Some(net) = c.need("network_ref") {
        match net.as_object() {
            None => c
                .errors
                .push("output \"network_ref\" must be an object".to_string()),
            Some(net) => {
                c.str(
                    "network_ref.vpc_id",
                    net.get("vpc_id"),
                    Some(&VPC_ID),
                    "a VPC id",
                );
                let sub = c.str(
                    "network_ref.subnet_id",
                    net.get("subnet_id"),
                    Some(&SUBNET_ID),
                    "a subnet id",
                );
                let bench = c.str(
                    "network_ref.bench_subnet_id",
                    net.get("bench_subnet_id"),
                    Some(&SUBNET_ID),
                    "a subnet id",
                );
                if let (Some(s), Some(b)) = (&sub, &bench)
                    && s == b
                {
                    c.errors.push(
                            "network_ref.bench_subnet_id must differ from subnet_id: bench machines must not share the agent subnet (they have no route out)"
                                .to_string(),
                        );
                }
                match net.get("security_group_ids").and_then(Value::as_object) {
                    None => c.errors.push("output \"network_ref.security_group_ids\" must be an object with agent_runner and bench".to_string()),
                    Some(sg) => {
                        let a = c.str("network_ref.security_group_ids.agent_runner", sg.get("agent_runner"), Some(&SG_ID), "a security group id");
                        let b = c.str("network_ref.security_group_ids.bench", sg.get("bench"), Some(&SG_ID), "a security group id");
                        if let (Some(a), Some(b)) = (&a, &b)
                            && a == b {
                                c.errors.push("network_ref.security_group_ids.agent_runner and .bench must be different groups".to_string());
                            }
                    }
                }
                if let Some(p) = net.get("egress_proxy_url")
                    && !p.is_null()
                    && !p.is_string()
                {
                    c.errors.push(
                        "output \"network_ref.egress_proxy_url\" must be a string or null"
                            .to_string(),
                    );
                }
                if !net.contains_key("egress_proxy_url") {
                    c.errors.push("output \"network_ref.egress_proxy_url\" is missing (use null without a proxy)".to_string());
                }
            }
        }
    }

    if let Some(limits) = c.need("limits") {
        match limits.as_object() {
            None => c
                .errors
                .push("output \"limits\" must be an object with max_instances_per_agent and max_instance_lifetime_hours".to_string()),
            Some(l) => {
                for k in ["max_instances_per_agent", "max_instance_lifetime_hours"] {
                    let ok = l.get(k).and_then(Value::as_f64).is_some_and(|v| v.is_finite() && v > 0.0);
                    if !ok {
                        c.errors.push(format!("output \"limits.{k}\" must be a positive number"));
                    }
                }
            }
        }
    }
    ContractCheck {
        ok: c.errors.is_empty(),
        errors: c.errors,
    }
}

/// Fails with a [`ContractViolationError`] listing every problem when the outputs (`tofu output -json`) do not satisfy the
/// contract.
pub fn assert_sandbox_contract(json: &str, opts: &ContractCheckOptions) -> Result<()> {
    let doc: Value = match serde_json::from_str(json) {
        Ok(d) => d,
        Err(_) => {
            return Err(ContractViolationError {
                errors: vec!["the module outputs are not JSON".to_string()],
            }
            .into());
        }
    };
    let r = check_sandbox_contract(&doc, *opts);
    if r.ok {
        Ok(())
    } else {
        Err(ContractViolationError { errors: r.errors }.into())
    }
}
