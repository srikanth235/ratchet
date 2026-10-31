//! The validator every drafted environment goes through, before the user sees it and again when the engine applies it
//! (`validate.ts`; docs/SIMPLIFIED_MODEL.md, "Dynamic environments"; the contract is infra/contract.md plus
//! infra/modules/sandbox).
//!
//! It is a STATIC check, deliberately an allow-list: the draft may only (a) call the base module Ratchet ships, with a
//! literal map of machines, (b) add a few inert resources (`aws_s3_object` seed data under the environment prefix), and
//! (c) carry scripts and a Dockerfile as plain files. Everything else a setup agent could write (IAM, network, public
//! addresses, provisioners, providers, state backends, imports, reads from this machine) fails a named check. `tofu
//! validate` and `tofu plan` in the user's account are the second line (`plan` check, added by the drafter); this one
//! has no cloud, no tofu binary and no clock.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use ratchet_core::config::{DEFAULT_ENVIRONMENT_INFRA_DIR, parse_config};
use ratchet_core::enums::EnvironmentScope;
use ratchet_core::numfmt::{js_number_to_string, to_fixed};
use ratchet_core::protected::normalize_repo_path;
use regex::Regex;
use serde_json::{Map, Value as Json};

use crate::cost::{
    CostEstimateOptions, INSTANCE_USD_PER_HOUR, MachineSpec, default_machines,
    estimate_machines_cost, js_round,
};
use crate::hcl::{HclBlock, HclValue, literal_number, literal_string, parse_hcl, walk_blocks};
use crate::types::{
    EnvironmentCheck, EnvironmentCost, EnvironmentFile, EnvironmentResource, EnvironmentRole,
};

/// Source of the base module as the draft writes it. Ratchet copies the shipped module there when it applies the
/// environment.
pub const SANDBOX_BASE_MODULE_SOURCE: &str = "./ratchet-sandbox";
/// The module block's name; the generated outputs and the validator refer to it.
pub const SANDBOX_BASE_MODULE_NAME: &str = "sandbox";
/// Key prefix in the artifacts bucket a draft's seed objects must live under.
pub const ENVIRONMENT_OBJECT_PREFIX: &str = "env/";
/// Files Ratchet generates next to a draft when it materializes it; a draft must not ship them.
pub const RATCHET_GENERATED_FILES: [&str; 2] = ["ratchet_inputs.tf", "ratchet_outputs.tf"];

/// Check ids, stable for the UI and the tests. The first four are the headline claims.
pub const ENVIRONMENT_CHECK_IDS: [&str; 13] = [
    "isolated",
    "sealed",
    "same-hardware",
    "teardown",
    "files",
    "syntax",
    "config",
    "scope",
    "base-module",
    "local-access",
    "tagged",
    "cost",
    "plan",
];

pub const MAX_ENVIRONMENT_MACHINES: usize = 8;
pub const MAX_ENVIRONMENT_FILE_BYTES: usize = 256 * 1024;
pub const MAX_ENVIRONMENT_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_VOLUME_GB: f64 = 1000.0;

/// Keys of one `machines` entry (the base module's variable). Anything else is refused: no subnet, group, address or
/// identity.
pub const MACHINE_KEYS: [&str; 7] = [
    "role",
    "instance_type",
    "label",
    "detail",
    "user_data",
    "root_volume_gb",
    "data_volume_gb",
];

const ALLOWED_RESOURCE_TYPES: [&str; 2] = ["aws_s3_object", "random_id"];
const ALLOWED_DATA_TYPES: [&str; 3] = ["aws_caller_identity", "aws_region", "aws_partition"];
const FILE_FUNCTIONS: [&str; 13] = [
    "file",
    "filebase64",
    "templatefile",
    "fileexists",
    "fileset",
    "filemd5",
    "filesha1",
    "filesha256",
    "filesha512",
    "filebase64sha256",
    "filebase64sha512",
    "abspath",
    "pathexpand",
];

#[derive(Debug, Clone)]
pub struct ValidateEnvironmentInput {
    pub scope: EnvironmentScope,
    /// Everything the draft would commit: `ratchet.yaml` (or `config_path`) and the files under `infra/ratchet/`.
    pub files: Vec<EnvironmentFile>,
    /// ratchet.yaml as it is on the base ref: proves the draft changed `environment:` and nothing else.
    pub original_config_yaml: Option<String>,
    /// Path of the config in the repo (default `ratchet.yaml`).
    pub config_path: Option<String>,
    /// The run's time limit for the cost estimate (default 2 h).
    pub time_limit_seconds: Option<f64>,
    /// The files are the environment module only (repo paths under `infra/ratchet/`, no ratchet.yaml): what the
    /// engine and the sandbox provider re-check when they APPLY an approved environment. The `config` check is then
    /// skipped.
    pub module_only: bool,
    /// Hourly prices (default: the static table).
    pub prices: Option<HashMap<String, f64>>,
    /// Pinned bench machine for `none` / `single` without OpenTofu (default c7i.large); read from ratchet.yaml when it
    /// is part of the draft.
    pub hardware: Option<String>,
}

impl ValidateEnvironmentInput {
    pub fn new(scope: EnvironmentScope, files: Vec<EnvironmentFile>) -> Self {
        Self {
            scope,
            files,
            original_config_yaml: None,
            config_path: None,
            time_limit_seconds: None,
            module_only: false,
            prices: None,
            hardware: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentValidation {
    pub ok: bool,
    pub checks: Vec<EnvironmentCheck>,
    /// The machines the draft creates (parsed from the module call; the evaluator / pinned machine for `none` /
    /// `single` without .tf).
    pub machines: Vec<MachineSpec>,
    pub resources: Vec<EnvironmentResource>,
    pub cost: EnvironmentCost,
    /// Flat list of every failed check's problems.
    pub problems: Vec<String>,
}

const KB: usize = 1024;

fn is_tf(p: &str) -> bool {
    p.ends_with(".tf")
}

fn base_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

fn strip_ws(s: &str) -> String {
    s.chars().filter(|c| !is_js_space(*c)).collect()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn pass(id: &str, label: &str, detail: String) -> EnvironmentCheck {
    EnvironmentCheck {
        id: id.into(),
        label: label.into(),
        ok: true,
        detail,
    }
}

fn result(id: &str, label: &str, problems: &[String], ok_detail: String) -> EnvironmentCheck {
    if problems.is_empty() {
        return pass(id, label, ok_detail);
    }
    let shown = &problems[..problems.len().min(6)];
    let more = if problems.len() > shown.len() {
        format!(" (+{} more)", problems.len() - shown.len())
    } else {
        String::new()
    };
    EnvironmentCheck {
        id: id.into(),
        label: label.into(),
        ok: false,
        detail: format!("{}{more}", shown.join("; ")),
    }
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

// ---- regexes ----------------------------------------------------------------------------------------------------------

fn re(p: &str) -> Regex {
    Regex::new(p).expect("static regex")
}

static NET_RESOURCE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^aws_(internet_gateway|nat_gateway|eip|eip_association|route|route_table|route_table_association|vpc|subnet|vpc_peering|vpn|customer_gateway|ec2_transit|lb|alb|elb|globalaccelerator|cloudfront|api_gateway|apigateway|network_interface|network_acl|security_group|vpc_security_group|vpc_endpoint|instance|launch_template|autoscaling|spot)",
    )
});
static IAM_RESOURCE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^aws_(iam|organizations|sso|kms_key_policy)"));
static LOCAL_EXEC: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?-u:\b)(?:local-exec|remote-exec)(?-u:\b)"));
static DATA_EXTERNAL: LazyLock<Regex> = LazyLock::new(|| re(r#"data\s+"external""#));
static PATH_ROOT: LazyLock<Regex> = LazyLock::new(|| re(r"(?-u:\b)path\.(?:cwd|root)(?-u:\b)"));
static COMMENT_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*(?:#|//).*$"));
static SEALED_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?-u:\b)(?:sealed|holdout|bench-results|confirm_identity|eval_identity|agent_boundary)(?-u:\b)",
    )
});
static INSTANCE_PROFILE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?-u:\b)iam_instance_profile(?-u:\b)|(?-u:\b)instance_profile(?-u:\b)"));
static FILE_CALL: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"({})\s*\(", FILE_FUNCTIONS.join("|"))));
static MODULE_FILE_ARG: LazyLock<Regex> = LazyLock::new(|| re(r#"^"\$\{path\.module\}/([^"]+)""#));
static S3_SOURCE: LazyLock<Regex> = LazyLock::new(|| re(r"^\$\{path\.module\}/[^.]"));
static INSTANCE_TYPE: LazyLock<Regex> = LazyLock::new(|| re(r"^[a-z][a-z0-9]*\.[a-z0-9]+$"));
static SUPPORT_FILE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\.(md|txt|json|ya?ml|csv|sql)$"));
static COMMENT_ONLY: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*#"));
static INTERNET_IN_SCRIPT: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)(?-u:\b)(?:curl|wget)(?-u:\b)[^\n]*(?-u:\b)https?://",
        r"(?i)(?-u:\b)git\s+clone\s+https?://",
        r"(?i)(?-u:\b)(?:apt(?:-get)?|yum|dnf|apk)\s+(?:-[^\s]+\s+)*(?:install|update|upgrade)(?-u:\b)",
        r"(?i)(?-u:\b)pip3?\s+install(?-u:\b)",
        r"(?i)(?-u:\b)npm\s+(?:i|install|ci)(?-u:\b)",
        r"(?i)(?-u:\b)docker\s+pull(?-u:\b)",
        r"(?i)(?-u:\b)go\s+(?:get|install)(?-u:\b)",
    ]
    .iter()
    .map(|p| re(p))
    .collect()
});

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `/(?<![\d.])0\.0\.0\.0\/0|::\/0/`
fn has_open_cidr(text: &str) -> bool {
    if text.contains("::/0") {
        return true;
    }
    let mut from = 0;
    while let Some(i) = text[from..].find("0.0.0.0/0") {
        let at = from + i;
        let prev = text[..at].chars().next_back();
        if !prev.is_some_and(|c| c.is_ascii_digit() || c == '.') {
            return true;
        }
        from = at + 1;
    }
    false
}

/// Arguments of every call to a file function in `text`, e.g. `file("${path.module}/seed.sh")` ->
/// `"${path.module}/seed.sh"`.
fn file_calls(text: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut pos = 0;
    while let Some(m) = FILE_CALL.captures_at(text, pos) {
        let whole = m.get(0).expect("match");
        let name = m.get(1).expect("group").as_str();
        let prev = text[..whole.start()].chars().next_back();
        if prev.is_some_and(|c| is_word_char(c) || c == '.') {
            // the lookbehind failed here: the scan goes on one character further
            pos = whole.start()
                + text[whole.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            continue;
        }
        let mut depth = 1;
        let mut arg = String::new();
        for c in text[whole.end()..].chars() {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
            }
            if depth <= 0 {
                break;
            }
            arg.push(c);
        }
        out.push((name.to_owned(), crate::hcl::js_trim(&arg).to_owned()));
        pos = whole.end();
    }
    out
}

fn text_contains(v: &HclValue, needle: &str) -> bool {
    match v {
        HclValue::String { value, .. } => value.contains(needle),
        HclValue::Expr(t) => t.contains(needle),
        HclValue::List(items) => items.iter().any(|i| text_contains(i, needle)),
        HclValue::Object(e) => e
            .iter()
            .any(|(k, x)| k.contains(needle) || text_contains(x, needle)),
        _ => false,
    }
}

// ---- the ratchet.yaml comparison -----------------------------------------------------------------------------------------

fn yaml_to_json(v: &serde_norway::Value) -> Json {
    use serde_norway::Value as Y;
    match v {
        Y::Null => Json::Null,
        Y::Bool(b) => Json::Bool(*b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                Json::from(i)
            } else if let Some(u) = n.as_u64() {
                Json::from(u)
            } else {
                n.as_f64()
                    .and_then(serde_json::Number::from_f64)
                    .map_or(Json::Null, Json::Number)
            }
        }
        Y::String(s) => Json::String(s.clone()),
        Y::Sequence(s) => Json::Array(s.iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => {
            let mut o = Map::new();
            for (k, x) in m {
                let key = match k {
                    Y::String(s) => s.clone(),
                    other => match yaml_to_json(other) {
                        Json::String(s) => s,
                        Json::Null => "null".into(),
                        o => o.to_string(),
                    },
                };
                o.insert(key, yaml_to_json(x));
            }
            Json::Object(o)
        }
        Y::Tagged(t) => yaml_to_json(&t.value),
    }
}

fn stable_json(v: &Json) -> String {
    match v {
        Json::Array(a) => format!(
            "[{}]",
            a.iter().map(stable_json).collect::<Vec<_>>().join(",")
        ),
        Json::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{}:{}", json_str(k), stable_json(&o[*k])))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        other => other.to_string(),
    }
}

fn js_string_of(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Number(n) => js_number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Json::Null => "null".into(),
        Json::Bool(b) => b.to_string(),
        Json::Array(a) => a.iter().map(js_string_of).collect::<Vec<_>>().join(","),
        Json::Object(_) => "[object Object]".into(),
    }
}

fn parse_yaml_object(text: &str) -> Result<Map<String, Json>, String> {
    let v: serde_norway::Value = serde_norway::from_str(text)
        .map_err(|e| e.to_string().lines().next().unwrap_or("").to_owned())?;
    match yaml_to_json(&v) {
        Json::Object(o) => Ok(o),
        _ => Ok(Map::new()),
    }
}

fn networks(env: Option<&Json>) -> Vec<String> {
    match env.and_then(|e| e.get("network")) {
        Some(Json::Array(a)) => a.iter().map(js_string_of).collect(),
        _ => vec![],
    }
}

fn compare_configs(config_path: &str, original: &str, draft: &str, problems: &mut Vec<String>) {
    let (before, after) = match (parse_yaml_object(original), parse_yaml_object(draft)) {
        (Ok(b), Ok(a)) => (b, a),
        (Err(e), _) | (_, Err(e)) => {
            problems.push(format!(
                "cannot compare {config_path} with the base ref: {e}"
            ));
            return;
        }
    };
    let strip = |m: &Map<String, Json>| {
        let mut m = m.clone();
        m.shift_remove("environment");
        Json::Object(m)
    };
    if stable_json(&strip(&before)) != stable_json(&strip(&after)) {
        let mut keys: Vec<&String> = vec![];
        for k in before.keys().chain(after.keys()) {
            if k != "environment" && !keys.contains(&k) {
                keys.push(k);
            }
        }
        let get = |m: &Map<String, Json>, k: &str| {
            m.get(k).map_or_else(|| "null".to_owned(), stable_json)
        };
        let changed: Vec<&str> = keys
            .iter()
            .filter(|k| get(&before, k) != get(&after, k))
            .map(|k| k.as_str())
            .collect();
        problems.push(format!(
            "{config_path} changes {}: a draft may only change environment (what is measured, the guardrails and the protected paths are the user's)",
            changed.join(", ")
        ));
    }
    let nets_before = networks(before.get("environment"));
    let added: Vec<String> = networks(after.get("environment"))
        .into_iter()
        .filter(|h| !nets_before.contains(h))
        .collect();
    if !added.is_empty() {
        problems.push(format!(
            "environment.network adds {}: egress hosts are the user's to grant, not a draft's",
            added.join(", ")
        ));
    }
}

// ---- machines ----------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Problems {
    same: Vec<String>,
    sealed: Vec<String>,
    scope: Vec<String>,
    module: Vec<String>,
}

/// The literal `machines` map of the module call, as machine specs (problems are pushed to `bag`).
fn read_machines(m: &HclBlock, bag: &mut Problems) -> Vec<MachineSpec> {
    let Some(attr) = m.attr("machines") else {
        bag.module.push("the module call has no `machines`".into());
        return vec![];
    };
    let HclValue::Object(entries) = attr else {
        bag.same.push(
            "`machines` must be a literal map, not an expression: the hardware has to be readable before anything is created"
                .into(),
        );
        return vec![];
    };
    let mut out = vec![];
    for (key, v) in entries {
        let HclValue::Object(fields) = v else {
            bag.same
                .push(format!("machine \"{key}\" must be an object"));
            continue;
        };
        for (k, _) in fields {
            if !MACHINE_KEYS.contains(&k.as_str()) {
                bag.sealed.push(format!(
                    "machine \"{key}\" sets \"{k}\": machines are placed, addressed and given their identity by the base module (allowed keys: {})",
                    MACHINE_KEYS.join(", ")
                ));
            }
        }
        let field = |n: &str| v.entry(n);
        let role = literal_string(field("role")).unwrap_or_default();
        let parsed_role = EnvironmentRole::parse(&role);
        if role.is_empty() || parsed_role.is_none() {
            bag.module.push(format!(
                "machine \"{key}\" needs a role: {}",
                EnvironmentRole::ALL
                    .iter()
                    .map(|r| r.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
        let ty = literal_string(field("instance_type")).unwrap_or_default();
        if ty.is_empty() {
            bag.same.push(format!(
                "machine \"{key}\": instance_type must be a literal such as \"c7i.2xlarge\" (not a variable or expression), so every run gets the same hardware"
            ));
        } else if !INSTANCE_TYPE.is_match(&ty) {
            bag.same.push(format!(
                "machine \"{key}\": \"{ty}\" is not an instance type"
            ));
        }
        let label = literal_string(field("label")).unwrap_or_else(|| key.clone());
        let detail = literal_string(field("detail")).unwrap_or_default();
        let data = literal_number(field("data_volume_gb"));
        let root = literal_number(field("root_volume_gb"));
        for (n, x) in [("data_volume_gb", data), ("root_volume_gb", root)] {
            if let Some(x) = x
                && !(0.0..=MAX_VOLUME_GB).contains(&x)
            {
                bag.module.push(format!(
                    "machine \"{key}\": {n} must be 0-{}",
                    MAX_VOLUME_GB as u32
                ));
            }
        }
        if !ty.is_empty() && !role.is_empty() {
            out.push(MachineSpec {
                key: Some(key.clone()),
                name: label,
                role: parsed_role.unwrap_or(EnvironmentRole::Other),
                machine: ty,
                detail,
                data_volume_gb: data,
                root_volume_gb: root,
            });
        }
    }
    out
}

struct TfFile {
    path: String,
    blocks: Vec<HclBlock>,
    text: String,
}

/// Validate a draft against the sandbox contract. Pure.
pub fn validate_environment(input: &ValidateEnvironmentInput) -> EnvironmentValidation {
    let config_path = {
        let p = normalize_repo_path(input.config_path.as_deref().unwrap_or("ratchet.yaml"));
        if p.is_empty() {
            "ratchet.yaml".to_owned()
        } else {
            p
        }
    };
    let prefix = format!("{DEFAULT_ENVIRONMENT_INFRA_DIR}/");
    let mut checks: Vec<EnvironmentCheck> = vec![];
    let time_limit_seconds = input.time_limit_seconds.unwrap_or(2.0 * 3600.0);

    // ---- files -----------------------------------------------------------------------------------------------------
    let mut file_problems: Vec<String> = vec![];
    let mut seen: HashSet<String> = HashSet::new();
    let mut total = 0usize;
    let mut files: Vec<EnvironmentFile> = vec![];
    for f in &input.files {
        let p = normalize_repo_path(&f.path);
        let bytes = f.content.len();
        total += bytes;
        if p.is_empty()
            || f.path.contains('\0')
            || p.split('/').any(|s| s == "..")
            || p.split('/').any(|s| s.to_lowercase() == ".git")
        {
            file_problems.push(format!("unsafe path {}", json_str(&f.path)));
            continue;
        }
        if !seen.insert(p.clone()) {
            file_problems.push(format!("{p} appears twice"));
            continue;
        }
        if p != config_path && !p.starts_with(&prefix) {
            file_problems.push(format!(
                "{p}: a drafted environment may only change {config_path} and files under {prefix}"
            ));
        }
        let name = base_name(&p);
        if p.starts_with(&prefix)
            && (name.ends_with(".tf.json")
                || name.ends_with(".tfvars")
                || name.ends_with(".tfvars.json")
                || name.ends_with(".tfstate")
                || name.starts_with(".terraform")
                || name == "override.tf"
                || name.ends_with("_override.tf")
                || RATCHET_GENERATED_FILES.contains(&name)
                || p.starts_with(&format!("{prefix}ratchet-sandbox/")))
        {
            file_problems.push(format!(
                "{p}: this file type is not allowed (variable files, overrides, state, JSON configuration and Ratchet's own files are not part of a draft)"
            ));
        }
        if bytes > MAX_ENVIRONMENT_FILE_BYTES {
            file_problems.push(format!(
                "{p} is {} KB (limit {} KB)",
                js_round(bytes as f64 / KB as f64),
                MAX_ENVIRONMENT_FILE_BYTES / KB
            ));
        }
        files.push(EnvironmentFile::new(p, f.content.clone()));
    }
    if total > MAX_ENVIRONMENT_TOTAL_BYTES {
        file_problems.push(format!(
            "the draft is {} KB (limit {} KB)",
            js_round(total as f64 / KB as f64),
            MAX_ENVIRONMENT_TOTAL_BYTES / KB
        ));
    }
    checks.push(result(
        "files",
        "Only the environment is touched",
        &file_problems,
        format!(
            "{} file{}, all under {prefix} or {config_path}",
            files.len(),
            plural(files.len())
        ),
    ));

    // ---- parse the OpenTofu ----------------------------------------------------------------------------------------
    let mut tf: Vec<TfFile> = vec![];
    let mut syntax: Vec<String> = vec![];
    for f in files.iter().filter(|x| is_tf(&x.path)) {
        let parsed = parse_hcl(&f.content);
        for e in &parsed.errors {
            syntax.push(format!("{}: {e}", f.path));
        }
        tf.push(TfFile {
            path: f.path.clone(),
            blocks: parsed.blocks,
            text: f.content.clone(),
        });
    }
    checks.push(result(
        "syntax",
        "OpenTofu is readable",
        &syntax,
        if tf.is_empty() {
            "no OpenTofu in this draft".into()
        } else {
            format!("{} .tf file{} read", tf.len(), plural(tf.len()))
        },
    ));

    let all: Vec<(&str, &HclBlock)> = tf
        .iter()
        .flat_map(|f| {
            walk_blocks(&f.blocks)
                .into_iter()
                .map(move |b| (f.path.as_str(), b))
        })
        .collect();
    let where_ = |file: &str, b: &HclBlock| format!("{file}:{}", b.line);

    // ---- config ----------------------------------------------------------------------------------------------------
    let mut config_problems: Vec<String> = vec![];
    let config_file = files.iter().find(|f| f.path == config_path);
    let mut hardware = input.hardware.clone().unwrap_or_else(|| "c7i.large".into());
    if input.module_only {
        // the module alone is re-checked at apply time: there is no ratchet.yaml to compare
    } else if let Some(cf) = config_file {
        match parse_config(&cf.content) {
            Err(errors) => {
                for e in &errors {
                    config_problems.push(format!("{config_path}: {e}"));
                }
            }
            Ok(cfg) => {
                hardware = cfg.evaluator.hardware.clone();
                let declared = cfg.environment.as_ref().and_then(|e| e.scope);
                if declared != Some(input.scope) {
                    config_problems.push(format!(
                        "{config_path} declares environment.scope {}, the draft is {}",
                        declared.map_or("(none)".to_owned(), |s| s.to_string()),
                        input.scope
                    ));
                }
                if let Some(orig) = &input.original_config_yaml {
                    compare_configs(&config_path, orig, &cf.content, &mut config_problems);
                }
            }
        }
    } else {
        config_problems.push(format!(
            "{config_path} must be part of the draft (it records environment.scope)"
        ));
    }
    checks.push(result(
        "config",
        "Only the environment section of ratchet.yaml changes",
        &config_problems,
        "what is measured, the guardrails and the protected paths are untouched".into(),
    ));

    // ---- the module call + machines --------------------------------------------------------------------------------
    let mut bag = Problems::default();
    let modules: Vec<&(&str, &HclBlock)> =
        all.iter().filter(|x| x.1.block_type == "module").collect();
    let mut machines: Vec<MachineSpec> = vec![];
    let is_base = |b: &HclBlock| {
        b.labels.first().map(String::as_str) == Some(SANDBOX_BASE_MODULE_NAME)
            && literal_string(b.attr("source")).as_deref() == Some(SANDBOX_BASE_MODULE_SOURCE)
    };

    for (file, block) in &modules {
        let name = block.labels.first();
        let source = literal_string(block.attr("source"));
        if name.map(String::as_str) != Some(SANDBOX_BASE_MODULE_NAME)
            || source.as_deref() != Some(SANDBOX_BASE_MODULE_SOURCE)
        {
            bag.module.push(format!(
                "{}: module \"{}\" (source {}) is not allowed: the only module a draft may call is module \"{SANDBOX_BASE_MODULE_NAME}\" with source \"{SANDBOX_BASE_MODULE_SOURCE}\"",
                where_(file, block),
                name.map_or("?", String::as_str),
                match &source {
                    Some(s) if !s.is_empty() => json_str(s),
                    _ => "is not a literal".to_owned(),
                }
            ));
        }
    }
    let base_calls: Vec<&(&str, &HclBlock)> =
        modules.iter().copied().filter(|x| is_base(x.1)).collect();
    if !tf.is_empty() && base_calls.is_empty() {
        bag.module.push(format!(
            "missing the base module: call module \"{SANDBOX_BASE_MODULE_NAME}\" with source \"{SANDBOX_BASE_MODULE_SOURCE}\" (it brings the network isolation, the evaluation identity, the sealed results and the budget tags)"
        ));
    }
    if base_calls.len() > 1 {
        bag.module
            .push("the base module is called more than once".into());
    }
    for (file, block) in &base_calls {
        let ok_ratchet = matches!(block.attr("ratchet"), Some(HclValue::Expr(t)) if strip_ws(t) == "var.ratchet");
        if !ok_ratchet {
            bag.module.push(format!(
                "{}: pass the sandbox through with `ratchet = var.ratchet`",
                where_(file, block)
            ));
        }
        for k in ["count", "for_each", "providers", "depends_on"] {
            if block.attr(k).is_some() {
                bag.module.push(format!(
                    "{}: the module call may not set {k}",
                    where_(file, block)
                ));
            }
        }
        for (k, _) in &block.attrs {
            if !["source", "ratchet", "machines"].contains(&k.as_str()) {
                bag.module.push(format!(
                    "{}: unknown module argument \"{k}\"",
                    where_(file, block)
                ));
            }
        }
        machines = read_machines(block, &mut bag);
        let mut seen_labels: HashSet<&str> = HashSet::new();
        for m in &machines {
            if !seen_labels.insert(&m.name) {
                bag.module
                    .push(format!("two machines are called \"{}\"", m.name));
            }
        }
    }
    if machines.len() > MAX_ENVIRONMENT_MACHINES {
        bag.module.push(format!(
            "{} machines (limit {MAX_ENVIRONMENT_MACHINES})",
            machines.len()
        ));
    }
    for (file, block) in all.iter().filter(|x| x.1.block_type == "variable") {
        if block.labels.first().map(String::as_str) == Some("ratchet") {
            bag.module.push(format!(
                "{}: variable \"ratchet\" is provided by Ratchet; a draft must not declare it",
                where_(file, block)
            ));
        }
    }
    for (file, block) in all.iter().filter(|x| x.1.block_type == "output") {
        if block.labels.first().map(String::as_str) == Some("environment") {
            bag.module.push(format!(
                "{}: output \"environment\" is generated by Ratchet",
                where_(file, block)
            ));
        }
    }
    for (file, block) in all.iter().filter(|x| x.1.block_type == "terraform") {
        if block
            .blocks
            .iter()
            .any(|b| b.block_type == "backend" || b.block_type == "cloud")
        {
            bag.module.push(format!(
                "{}: a backend would move the state out of Ratchet's hands",
                where_(file, block)
            ));
        }
        if block
            .blocks
            .iter()
            .any(|b| b.block_type == "required_providers")
        {
            bag.module.push(format!(
                "{}: providers are pinned by Ratchet, a draft declares none",
                where_(file, block)
            ));
        }
    }
    for (file, block) in all.iter().filter(|x| x.1.block_type == "provider") {
        bag.sealed.push(format!(
            "{}: provider \"{}\" would let the draft reach another region or account; Ratchet configures the provider",
            where_(file, block),
            block.labels.first().map_or("?", String::as_str)
        ));
    }
    checks.push(result(
        "base-module",
        "Built on the base module Ratchet ships",
        &bag.module,
        if tf.is_empty() {
            "no infrastructure is drafted (nothing to build on)".into()
        } else {
            format!(
                "module \"{SANDBOX_BASE_MODULE_NAME}\" with {} machine{}",
                machines.len(),
                plural(machines.len())
            )
        },
    ));

    // ---- resources allow-list, isolation, local access, teardown, tags -------------------------------------------------
    let mut isolated: Vec<String> = vec![];
    let mut local: Vec<String> = vec![];
    let mut teardown: Vec<String> = vec![];
    let mut tagged: Vec<String> = vec![];
    let known_files: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();

    for (file, block) in &all {
        let w = where_(file, block);
        match block.block_type.as_str() {
            "resource" => {
                let ty = block.labels.first().map_or("", String::as_str);
                if !ALLOWED_RESOURCE_TYPES.contains(&ty) {
                    let msg = format!("{w}: resource {ty} is not allowed");
                    if NET_RESOURCE.is_match(ty) {
                        isolated.push(format!(
                            "{msg}: network and machines come from the base module (measuring hosts have no route to the internet)"
                        ));
                    } else if IAM_RESOURCE.is_match(ty) {
                        bag.sealed.push(format!(
                            "{msg}: identities come from the base module (evaluation only; agents cannot read results or the holdout)"
                        ));
                    } else {
                        isolated.push(format!(
                            "{msg} (allowed: {}; machines go in the base module's `machines`)",
                            ALLOWED_RESOURCE_TYPES.join(", ")
                        ));
                    }
                }
                if ty == "aws_s3_object" {
                    let bucket_expr = match block.attr("bucket") {
                        Some(HclValue::Expr(t)) => strip_ws(t),
                        _ => String::new(),
                    };
                    if bucket_expr != "module.sandbox.artifacts_bucket" && bucket_expr != "var.ratchet.artifacts_bucket" {
                        bag.sealed.push(format!("{w}: aws_s3_object must use bucket = module.sandbox.artifacts_bucket"));
                    }
                    if let Some(src) = block.attr("source") {
                        let text = match src {
                            HclValue::String { value, .. } => value.as_str(),
                            _ => "",
                        };
                        if !S3_SOURCE.is_match(text) || text.contains("..") {
                            local.push(format!(
                                "{w}: source may only read a file of the draft: \"${{path.module}}/<file>\""
                            ));
                        }
                    }
                    let key = literal_string(block.attr("key"));
                    let key_dyn = match block.attr("key") {
                        Some(HclValue::String { value, .. }) => value.clone(),
                        _ => String::new(),
                    };
                    let k = key.unwrap_or(key_dyn);
                    if !k.starts_with(ENVIRONMENT_OBJECT_PREFIX) {
                        bag.sealed.push(format!(
                            "{w}: aws_s3_object keys must start with \"{ENVIRONMENT_OBJECT_PREFIX}\" (results and the holdout live elsewhere)"
                        ));
                    }
                }
                if ty != "random_id" {
                    let ok = block.attr("tags").is_some_and(|t| text_contains(t, "module.sandbox.tags"));
                    if !ok {
                        tagged.push(format!(
                            "{w}: {ty}.{} must set tags = module.sandbox.tags (or merge it in)",
                            block.labels.get(1).map_or("?", String::as_str)
                        ));
                    }
                }
            }
            "data" => {
                let ty = block.labels.first().map_or("", String::as_str);
                if !ALLOWED_DATA_TYPES.contains(&ty) {
                    local.push(format!(
                        "{w}: data source {ty} is not allowed (allowed: {})",
                        ALLOWED_DATA_TYPES.join(", ")
                    ));
                }
            }
            "provisioner" => local.push(format!(
                "{w}: provisioner \"{}\" runs commands; a drafted environment only declares resources",
                block.labels.first().map_or("", String::as_str)
            )),
            "import" | "removed" | "moved" => teardown.push(format!(
                "{w}: {} blocks are not allowed (they adopt or orphan resources, so teardown could not remove everything)",
                block.block_type
            )),
            "lifecycle" => {
                if let Some(HclValue::Bool(true)) = block.attr("prevent_destroy") {
                    teardown.push(format!("{w}: prevent_destroy blocks teardown"));
                }
            }
            _ => {}
        }
        for (k, v) in &block.attrs {
            if [
                "deletion_protection",
                "skip_destroy",
                "prevent_destroy",
                "delete_protection",
                "disable_api_termination",
                "termination_protection",
            ]
            .contains(&k.as_str())
                && matches!(v, HclValue::Bool(true))
            {
                teardown.push(format!("{w}: {k} = true blocks teardown"));
            }
            if k == "force_destroy" && matches!(v, HclValue::Bool(false)) {
                teardown.push(format!(
                    "{w}: force_destroy = false can leave objects behind"
                ));
            }
            if k == "associate_public_ip_address" && !matches!(v, HclValue::Bool(false)) {
                isolated.push(format!("{w}: public addresses are not allowed"));
            }
        }
    }
    for f in &tf {
        if has_open_cidr(&f.text) {
            isolated.push(format!(
                "{}: open CIDR ranges (0.0.0.0/0, ::/0) are not allowed: measuring hosts have no route to the internet",
                f.path
            ));
        }
        if LOCAL_EXEC.is_match(&f.text) || DATA_EXTERNAL.is_match(&f.text) {
            local.push(format!(
                "{}: commands and external data sources are not allowed",
                f.path
            ));
        }
        if PATH_ROOT.is_match(&f.text) {
            local.push(format!(
                "{}: path.cwd / path.root reach outside the draft; use ${{path.module}}",
                f.path
            ));
        }
        if SEALED_WORDS.is_match(&COMMENT_LINE.replace_all(&f.text, "")) {
            bag.sealed.push(format!(
                "{}: refers to the sealed store, holdout, results or identities, which a draft never touches",
                f.path
            ));
        }
        if INSTANCE_PROFILE.is_match(&f.text) {
            bag.sealed
                .push(format!("{}: identities are not set by a draft", f.path));
        }
        for (func, arg) in file_calls(&f.text) {
            let m = MODULE_FILE_ARG.captures(&arg);
            match m {
                Some(c) if !c[1].contains("..") => {
                    let rel = &c[1];
                    if func != "fileexists"
                        && func != "fileset"
                        && !known_files.contains(format!("{prefix}{rel}").as_str())
                    {
                        local.push(format!(
                            "{}: {func}() reads {rel}, which is not part of the draft",
                            f.path
                        ));
                    }
                }
                _ => {
                    let shown: String = arg.chars().take(60).collect();
                    local.push(format!(
                        "{}: {func}({shown}) must read a file inside the draft: {func}(\"${{path.module}}/<file>\")",
                        f.path
                    ));
                }
            }
        }
    }
    for f in files.iter().filter(|x| {
        x.path.starts_with(&prefix) && !is_tf(&x.path) && !SUPPORT_FILE.is_match(&x.path)
    }) {
        let lines: Vec<&str> = f
            .content
            .split('\n')
            .filter(|l| !COMMENT_ONLY.is_match(l))
            .collect();
        if let Some(hit) = INTERNET_IN_SCRIPT
            .iter()
            .find(|re| lines.iter().any(|l| re.is_match(l)))
        {
            let line = lines
                .iter()
                .find(|l| hit.is_match(l))
                .copied()
                .unwrap_or("");
            let shown: String = crate::hcl::js_trim(line).chars().take(80).collect();
            isolated.push(format!(
                "{}: `{shown}` needs the internet; measuring hosts have no route out (ship what they need in the repo's container or the seed data)",
                f.path
            ));
        }
    }
    checks.push(result(
        "isolated",
        "Isolated: no internet on the measuring hosts",
        &isolated,
        if tf.is_empty() {
            "no infrastructure is drafted".into()
        } else {
            "measuring hosts sit in the sandbox's private subnet: no public address, no route to the internet".into()
        },
    ));
    checks.push(result(
        "local-access",
        "Reads and runs nothing on your machine",
        &local,
        "only files of the draft are read; no commands run".into(),
    ));
    checks.push(result(
        "sealed",
        "Results sealed: agents can't read results or the holdout",
        &bag.sealed,
        "the evaluation identity is the base module's; objects live under env/".into(),
    ));
    checks.push(result(
        "tagged",
        "Everything tagged for the budget",
        &tagged,
        "the base module tags every machine and volume; nothing else is created untagged".into(),
    ));
    checks.push(result(
        "teardown",
        "Full teardown",
        &teardown,
        "nothing blocks, adopts or orphans a resource: destroy removes everything".into(),
    ));

    // ---- same hardware -----------------------------------------------------------------------------------------------
    for m in &machines {
        if matches!(
            m.role,
            EnvironmentRole::App | EnvironmentRole::Evaluator | EnvironmentRole::Load
        ) && m.machine.starts_with('t')
            && m.machine[1..].starts_with(|c: char| c.is_ascii_digit())
        {
            bag.same.push(format!(
                "\"{}\" is {}: burstable instances change speed with CPU credits; pin a fixed-performance type (c7i, m7i, r7i) for {}",
                m.name, m.machine, m.role
            ));
        }
    }
    checks.push(result(
        "same-hardware",
        "Same hardware every time",
        &bag.same,
        if machines.is_empty() {
            "the evaluator's pinned machine is used".into()
        } else {
            format!(
                "{} machine{}, each a literal instance type",
                machines.len(),
                plural(machines.len())
            )
        },
    ));

    // ---- scope -------------------------------------------------------------------------------------------------------
    let infra_files = files.iter().filter(|f| f.path.starts_with(&prefix)).count();
    let has_tf = !tf.is_empty();
    if input.scope == EnvironmentScope::None && infra_files > 0 {
        bag.scope.push(format!(
            "scope none needs no infrastructure, but {infra_files} file(s) under {prefix} were drafted"
        ));
    }
    if input.scope == EnvironmentScope::Single && has_tf && machines.len() != 1 {
        bag.scope.push(format!(
            "scope single is one machine, the draft has {}; use scope full for a stack",
            machines.len()
        ));
    }
    if input.scope == EnvironmentScope::Full {
        if !has_tf {
            bag.scope
                .push(format!("scope full needs OpenTofu ({prefix}main.tf)"));
        } else if machines.len() < 2 {
            bag.scope.push(
                "scope full is a stack of at least two machines (for one machine use scope single)"
                    .into(),
            );
        }
    }
    checks.push(result(
        "scope",
        "The scope fits the files",
        &bag.scope,
        format!("scope {}", input.scope),
    ));

    // ---- cost --------------------------------------------------------------------------------------------------------
    let specs = if has_tf || input.scope == EnvironmentScope::Full {
        machines
    } else {
        default_machines(input.scope, &hardware)
    };
    let est = estimate_machines_cost(
        &specs,
        &CostEstimateOptions {
            time_limit_seconds,
            prices: input.prices.clone(),
        },
    );
    let cost_problems: Vec<String> = est
        .unpriced
        .iter()
        .map(|t| {
            format!(
                "no price known for {t}: use a type from {}, ...",
                INSTANCE_USD_PER_HOUR
                    .iter()
                    .take(4)
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();
    checks.push(result(
        "cost",
        "Priced before it is created",
        &cost_problems,
        format!(
            "${}/h, ready in ~{} min",
            to_fixed(est.cost.usd_per_hour, 2),
            est.cost.startup_minutes
        ),
    ));

    checks.sort_by_key(|c| {
        ENVIRONMENT_CHECK_IDS
            .iter()
            .position(|id| *id == c.id)
            .unwrap_or(99)
    });
    EnvironmentValidation {
        ok: checks.iter().all(|c| c.ok),
        problems: checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| format!("{}: {}", c.label, c.detail))
            .collect(),
        checks,
        machines: specs,
        resources: est.resources,
        cost: est.cost,
    }
}
