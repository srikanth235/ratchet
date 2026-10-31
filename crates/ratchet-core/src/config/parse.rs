//! Validation of a parsed YAML document into a [`RatchetConfig`].
//!
//! This is the Rust equivalent of the zod `configSchema` in `packages/core/src/config/schema.ts`: it checks the
//! document field by field, collects every problem (with its YAML path and the same messages) instead of
//! stopping at the first, and then resolves defaults and the evaluator template.
//!
//! Notes on fidelity with zod:
//!  - issues come out in schema order (`version`, `pack`, `objective`, `evaluator`, ...), unknown keys last;
//!  - a field with a custom message reports that message for every kind of failure including "missing";
//!    a field without one reports zod's default wording, except a missing object, which reads "is required";
//!  - the template / `run` resolution only runs when the document is otherwise valid.

use serde_norway::{Mapping, Value};

use super::guardrails::{CommandGuardrail, Guardrail, is_js_space, parse_threshold};
use super::schema::{
    AgentConfig, DEFAULT_AGENT_INSTRUCTIONS, DEFAULT_BENCH_MACHINE, DEFAULT_EVALUATOR_REF,
    Environment, Evaluator, Objective, RatchetConfig, ValueModel,
};
use crate::enums::{DepsMode, Direction, EvaluatorKind, ValueBasis, ValuePeriod};
use crate::templates::{evaluator_template_ids, find_evaluator_template};

/// One problem found in `ratchet.yaml`.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ConfigError {
    /// YAML path, e.g. `guardrails[2].peak_rss` or `objective.direction`. Empty for whole-document errors.
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone)]
enum Seg {
    Key(String),
    Index(usize),
}

type Path = Vec<Seg>;

/// Render a path of keys/indices as `a.b[0].c`.
pub fn format_path(segments: &[PathSegment<'_>]) -> String {
    let mut out = String::new();
    for seg in segments {
        match seg {
            PathSegment::Index(i) => out.push_str(&format!("[{i}]")),
            PathSegment::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
        }
    }
    out
}

/// A segment accepted by [`format_path`].
#[derive(Debug, Clone, Copy)]
pub enum PathSegment<'a> {
    Key(&'a str),
    Index(usize),
}

fn path_string(path: &[Seg]) -> String {
    let segs: Vec<PathSegment<'_>> = path
        .iter()
        .map(|s| match s {
            Seg::Key(k) => PathSegment::Key(k),
            Seg::Index(i) => PathSegment::Index(*i),
        })
        .collect();
    format_path(&segs)
}

struct Cx {
    issues: Vec<ConfigError>,
    /// Issues that stop the zod pipe (everything except unknown keys).
    hard: usize,
}

impl Cx {
    fn new() -> Self {
        Cx {
            issues: Vec::new(),
            hard: 0,
        }
    }

    fn push(&mut self, path: &[Seg], message: impl Into<String>) {
        self.hard += 1;
        self.issues.push(ConfigError {
            path: path_string(path),
            message: message.into(),
        });
    }

    /// An `unrecognized_keys` issue. Zod (4.6) does not stop a pipe's transform for these.
    fn push_unknown(&mut self, path: &[Seg], message: impl Into<String>) {
        self.issues.push(ConfigError {
            path: path_string(path),
            message: message.into(),
        });
    }
}

fn child(path: &[Seg], key: &str) -> Path {
    let mut p = path.to_vec();
    p.push(Seg::Key(key.to_owned()));
    p
}

fn at(path: &[Seg], i: usize) -> Path {
    let mut p = path.to_vec();
    p.push(Seg::Index(i));
    p
}

fn untag(v: &Value) -> &Value {
    match v {
        Value::Tagged(t) => untag(&t.value),
        other => other,
    }
}

/// zod's name for the type of a value, as used in "expected X, received Y".
fn type_name(v: &Value) -> &'static str {
    match untag(v) {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "array",
        Value::Mapping(_) | Value::Tagged(_) => "object",
    }
}

/// zod's default message for a wrong type, with `undefined` read as "is required".
fn default_type_issue(expected: &str, v: Option<&Value>) -> String {
    match v {
        None => "is required".to_owned(),
        Some(v) => format!(
            "Invalid input: expected {expected}, received {}",
            type_name(v)
        ),
    }
}

fn get<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    map.get(key)
}

fn key_string(k: &Value) -> String {
    match untag(k) {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_owned(),
        other => serde_norway::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    }
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// Report keys outside `allowed` (zod `strictObject`).
fn unknown_keys(map: &Mapping, allowed: &[&str], path: &[Seg], cx: &mut Cx) {
    let unknown: Vec<String> = map
        .keys()
        .map(key_string)
        .filter(|k| !allowed.contains(&k.as_str()))
        .collect();
    if !unknown.is_empty() {
        let quoted: Vec<String> = unknown.iter().map(|k| quote(k)).collect();
        cx.push_unknown(
            path,
            format!(
                "unknown {} {}",
                if unknown.len() > 1 { "keys" } else { "key" },
                quoted.join(", ")
            ),
        );
    }
}

/// An object-typed value. `None` (and an issue) when it is not a mapping; `Ok(None)` when absent and optional.
fn object<'a>(
    v: Option<&'a Value>,
    required: bool,
    path: &[Seg],
    cx: &mut Cx,
) -> Option<&'a Mapping> {
    match v {
        None => {
            if required {
                cx.push(path, default_type_issue("object", None));
            }
            None
        }
        Some(v) => match untag(v) {
            Value::Mapping(m) => Some(m),
            other => {
                cx.push(path, default_type_issue("object", Some(other)));
                None
            }
        },
    }
}

struct StrRules<'a> {
    /// Message for any type failure (including missing). `None`: zod's default wording.
    type_msg: Option<&'a str>,
    /// Message for the empty string; `None`: no minimum length.
    empty_msg: Option<&'a str>,
}

impl<'a> StrRules<'a> {
    /// Message of the `min(1)` check. A check without a message of its own reports the schema's custom error
    /// (`z.string({ error })`) when there is one, zod's wording (`fallback`) otherwise.
    fn too_short(&self, check_msg: &'a str, fallback: &'a str) -> &'a str {
        if check_msg == DEFAULT_EMPTY {
            self.type_msg.unwrap_or(fallback)
        } else {
            check_msg
        }
    }
}

const DEFAULT_EMPTY: &str = "Too small: expected string to have >=1 characters";
const DEFAULT_EMPTY_LIST: &str = "Too small: expected array to have >=1 items";

/// A string field. `required`: a missing value is an issue.
fn string(
    v: Option<&Value>,
    required: bool,
    rules: &StrRules<'_>,
    path: &[Seg],
    cx: &mut Cx,
) -> Option<String> {
    match v {
        None => {
            if required {
                cx.push(
                    path,
                    rules
                        .type_msg
                        .map(str::to_owned)
                        .unwrap_or_else(|| default_type_issue("string", None)),
                );
            }
            None
        }
        Some(v) => match untag(v) {
            Value::String(s) => {
                if s.is_empty()
                    && let Some(m) = rules.empty_msg
                {
                    cx.push(path, rules.too_short(m, DEFAULT_EMPTY));
                    return None;
                }
                Some(s.clone())
            }
            other => {
                cx.push(
                    path,
                    rules
                        .type_msg
                        .map(str::to_owned)
                        .unwrap_or_else(|| default_type_issue("string", Some(other))),
                );
                // zod's `min` check also runs on values that have a `length`: an empty list reads as too short
                if let (Some(m), Value::Sequence(items)) = (rules.empty_msg, other)
                    && items.is_empty()
                {
                    cx.push(path, rules.too_short(m, DEFAULT_EMPTY_LIST));
                }
                None
            }
        },
    }
}

/// `z.string().min(1)` with zod's default messages.
fn plain_string(v: Option<&Value>, required: bool, path: &[Seg], cx: &mut Cx) -> Option<String> {
    string(
        v,
        required,
        &StrRules {
            type_msg: None,
            empty_msg: Some(DEFAULT_EMPTY),
        },
        path,
        cx,
    )
}

/// `nonEmpty(what)`: a string with "`what` must be a string" / "`what` must not be empty".
fn non_empty(
    v: Option<&Value>,
    required: bool,
    what: &str,
    path: &[Seg],
    cx: &mut Cx,
) -> Option<String> {
    let type_msg = format!("{what} must be a string");
    let empty_msg = format!("{what} must not be empty");
    string(
        v,
        required,
        &StrRules {
            type_msg: Some(&type_msg),
            empty_msg: Some(&empty_msg),
        },
        path,
        cx,
    )
}

/// A finite number. Any failure (wrong type, missing, NaN / infinity) reports `type_msg`.
fn number(
    v: Option<&Value>,
    required: bool,
    type_msg: &str,
    path: &[Seg],
    cx: &mut Cx,
) -> Option<f64> {
    match v {
        None => {
            if required {
                cx.push(path, type_msg);
            }
            None
        }
        Some(v) => match untag(v) {
            Value::Number(n) => match n.as_f64() {
                Some(f) if f.is_finite() => Some(f),
                _ => {
                    cx.push(path, type_msg);
                    None
                }
            },
            _ => {
                cx.push(path, type_msg);
                None
            }
        },
    }
}

/// A string restricted to `T`'s wire values. Any failure reports `msg`. Absent and optional: `Ok(None)`.
fn enumeration<T: std::str::FromStr>(
    v: Option<&Value>,
    required: bool,
    msg: &str,
    path: &[Seg],
    cx: &mut Cx,
) -> Option<T> {
    match v {
        None => {
            if required {
                cx.push(path, msg);
            }
            None
        }
        Some(v) => match untag(v) {
            Value::String(s) => match s.parse::<T>() {
                Ok(t) => Some(t),
                Err(_) => {
                    cx.push(path, msg);
                    None
                }
            },
            _ => {
                cx.push(path, msg);
                None
            }
        },
    }
}

fn names<T: Copy + std::fmt::Display>(all: &[T]) -> String {
    all.iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A repo-relative POSIX path: no leading `/`, no `..` segments, no backslashes.
fn repo_path(v: Option<&Value>, what: &str, path: &[Seg], cx: &mut Cx) -> Option<String> {
    let type_msg = format!("{what} must be a path inside the repository");
    let s = match untag(v?) {
        Value::String(s) => s.clone(),
        other => {
            cx.push(path, type_msg);
            if matches!(other, Value::Sequence(items) if items.is_empty()) {
                cx.push(path, format!("{what} must not be empty"));
            }
            return None;
        }
    };
    let before = cx.hard;
    if s.is_empty() {
        cx.push(path, format!("{what} must not be empty"));
    }
    let b = s.as_bytes();
    let drive_letter = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if s.starts_with('/') || drive_letter {
        cx.push(
            path,
            format!("{what} must be relative to the repository root"),
        );
    }
    if s.contains('\\') {
        cx.push(path, format!("{what} must use forward slashes"));
    }
    if s.split('/').any(|seg| seg == "..") {
        cx.push(
            path,
            format!("{what} must stay inside the repository (no \"..\")"),
        );
    }
    (cx.hard == before).then_some(s)
}

/// An egress host: `registry.npmjs.org` or `*.example.com`. No scheme, path or port; a bare `*` is not allowed.
fn is_host(s: &str) -> bool {
    let body = s.strip_prefix("*.").unwrap_or(s);
    let labels: Vec<&str> = body.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            let b = l.as_bytes();
            let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
            (1..=63).contains(&b.len())
                && alnum(b[0])
                && alnum(b[b.len() - 1])
                && b.iter().all(|&c| alnum(c) || c == b'-')
        })
}

/// `^[a-z][a-z0-9-]*\.[a-z0-9-]+$`
fn is_instance_type(s: &str) -> bool {
    let Some((family, size)) = s.split_once('.') else {
        return false;
    };
    let b = family.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !size.is_empty()
        && size
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// `^[a-z]{2}(?:-[a-z]+)+-\d+$`
fn is_aws_region(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() < 3 {
        return false;
    }
    let lower = |p: &str| !p.is_empty() && p.bytes().all(|c| c.is_ascii_lowercase());
    parts[0].len() == 2
        && lower(parts[0])
        && parts[1..parts.len() - 1].iter().all(|p| lower(p))
        && parts[parts.len() - 1].bytes().all(|c| c.is_ascii_digit())
        && !parts[parts.len() - 1].is_empty()
}

/// `^[a-z][a-z0-9-]*$`
fn is_template_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

const GUARDRAIL_MAPPING_MSG: &str = "each guardrail must be a mapping with a single key, e.g. `run: pytest -q` or `error_rate: \"< 0.001\"`";
const GUARDRAIL_STRING_MSG: &str =
    "guardrail values must be strings (e.g. run: \"pytest -q\" or error_rate: \"< 0.001\")";

/// One entry of `guardrails:`. `None` when it is invalid (issues recorded).
fn guardrail(v: &Value, path: &[Seg], cx: &mut Cx) -> Option<Guardrail> {
    let Value::Mapping(map) = untag(v) else {
        cx.push(path, GUARDRAIL_MAPPING_MSG);
        return None;
    };
    // z.record(z.string(), z.string()): every value must be a string
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut ok = true;
    for (k, val) in map {
        let key = key_string(k);
        match untag(val) {
            Value::String(s) => entries.push((key, s.clone())),
            _ => {
                cx.push(&child(path, &key), GUARDRAIL_STRING_MSG);
                ok = false;
            }
        }
    }
    if !ok {
        return None;
    }
    guardrail_from_entry(&entries, path, cx)
}

/// The transform of a valid record: exactly one key, `run` or a metric name.
fn guardrail_from_entry(
    entries: &[(String, String)],
    path: &[Seg],
    cx: &mut Cx,
) -> Option<Guardrail> {
    if entries.len() != 1 {
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        cx.push(
            path,
            format!(
                "a guardrail must have exactly one key (`run` or a metric name), found {}{}",
                entries.len(),
                if keys.is_empty() {
                    String::new()
                } else {
                    format!(": {}", keys.join(", "))
                }
            ),
        );
        return None;
    }
    let (key, value) = &entries[0];
    if key == "run" {
        if value.trim_matches(is_js_space).is_empty() {
            cx.push(&child(path, "run"), "`run` must be a non-empty command");
            return None;
        }
        return Some(Guardrail::Command(CommandGuardrail { run: value.clone() }));
    }
    match parse_threshold(key, value) {
        Ok(t) => Some(Guardrail::Threshold(t)),
        Err(msg) => {
            cx.push(&child(path, key), msg);
            None
        }
    }
}

const EVALUATOR_KEYS: [&str; 10] = [
    "kind",
    "ref",
    "run",
    "template",
    "repeats",
    "noise_factor",
    "hardware",
    "deps",
    "holdout",
    "confirm_trace",
];

/// The evaluator as written, before template defaults.
struct RawEvaluator {
    kind: Option<EvaluatorKind>,
    git_ref: Option<String>,
    run: Option<String>,
    template: Option<String>,
    repeats: Option<u64>,
    noise_factor: Option<f64>,
    hardware: Option<String>,
    deps: DepsMode,
    holdout: Option<String>,
}

fn evaluator(v: Option<&Value>, path: &[Seg], cx: &mut Cx) -> Option<RawEvaluator> {
    let map = object(v, true, path, cx)?;
    let before = cx.hard;

    let kind = {
        let msg = format!(
            "evaluator.kind must be one of {}",
            names(EvaluatorKind::ALL)
        );
        enumeration::<EvaluatorKind>(get(map, "kind"), false, &msg, &child(path, "kind"), cx)
    };
    let git_ref = non_empty(
        get(map, "ref"),
        false,
        "evaluator.ref",
        &child(path, "ref"),
        cx,
    );
    let run = non_empty(
        get(map, "run"),
        false,
        "evaluator.run",
        &child(path, "run"),
        cx,
    );
    let template = {
        let p = child(path, "template");
        let type_msg = "evaluator.template must be a template id (e.g. http-latency)";
        let s = string(
            get(map, "template"),
            false,
            &StrRules {
                type_msg: Some(type_msg),
                empty_msg: None,
            },
            &p,
            cx,
        );
        match s {
            Some(s) if !is_template_id(&s) => {
                cx.push(
                    &p,
                    "evaluator.template must be a template id such as http-latency",
                );
                None
            }
            other => other,
        }
    };
    let repeats = {
        let p = child(path, "repeats");
        let n = number(
            get(map, "repeats"),
            false,
            "repeats must be an integer",
            &p,
            cx,
        );
        match n {
            Some(n) => {
                // zod stops the checks of a number at the first failed `.int()`
                if n.fract() != 0.0 || n.abs() > 9_007_199_254_740_991.0 {
                    cx.push(&p, "repeats must be an integer");
                    None
                } else if n < 1.0 {
                    cx.push(&p, "repeats must be >= 1");
                    None
                } else {
                    Some(n as u64)
                }
            }
            None => None,
        }
    };
    let noise_factor = {
        let p = child(path, "noise_factor");
        let n = number(
            get(map, "noise_factor"),
            false,
            "noise_factor must be a number (the k of noise floor = k x stddev, default 2)",
            &p,
            cx,
        );
        match n {
            Some(n) => {
                let mut valid = true;
                if n < 0.0 {
                    cx.push(&p, "noise_factor must be >= 0");
                    valid = false;
                }
                if n > 10.0 {
                    cx.push(&p, "noise_factor must be <= 10");
                    valid = false;
                }
                valid.then_some(n)
            }
            None => None,
        }
    };
    let hardware = non_empty(
        get(map, "hardware"),
        false,
        "evaluator.hardware",
        &child(path, "hardware"),
        cx,
    );
    let deps = {
        let msg = format!("evaluator.deps must be one of {}", names(DepsMode::ALL));
        enumeration::<DepsMode>(get(map, "deps"), false, &msg, &child(path, "deps"), cx)
    };
    let holdout = string(
        get(map, "holdout"),
        false,
        &StrRules {
            type_msg: Some("holdout must be a string"),
            empty_msg: Some(DEFAULT_EMPTY),
        },
        &child(path, "holdout"),
        cx,
    );
    let confirm_trace = string(
        get(map, "confirm_trace"),
        false,
        &StrRules {
            type_msg: Some("confirm_trace must be a string"),
            empty_msg: Some(DEFAULT_EMPTY),
        },
        &child(path, "confirm_trace"),
        cx,
    );
    unknown_keys(map, &EVALUATOR_KEYS, path, cx);

    if cx.hard > before {
        return None;
    }
    // `holdout ?? confirm_trace`
    let trace = holdout.or(confirm_trace);
    Some(RawEvaluator {
        kind,
        git_ref,
        run,
        template,
        repeats,
        noise_factor,
        hardware,
        deps: deps.unwrap_or(DepsMode::Auto),
        holdout: trace,
    })
}

fn value_model(v: Option<&Value>, path: &[Seg], cx: &mut Cx) -> Option<ValueModel> {
    let map = object(v, false, path, cx)?;
    let before = cx.hard;
    let p = child(path, "usd_per_unit");
    let usd = number(
        get(map, "usd_per_unit"),
        true,
        "value.usd_per_unit must be a number (USD per one unit of metric improvement)",
        &p,
        cx,
    );
    if let Some(u) = usd
        && u <= 0.0
    {
        cx.push(&p, "value.usd_per_unit must be > 0");
    }
    let period = enumeration::<ValuePeriod>(
        get(map, "period"),
        false,
        "value.period must be \"month\" or \"year\"",
        &child(path, "period"),
        cx,
    );
    let basis = enumeration::<ValueBasis>(
        get(map, "basis"),
        false,
        "value.basis must be \"estimated\" or \"measured\"",
        &child(path, "basis"),
        cx,
    );
    let unit = plain_string(get(map, "unit"), false, &child(path, "unit"), cx);
    let note = plain_string(get(map, "note"), false, &child(path, "note"), cx);
    unknown_keys(
        map,
        &["usd_per_unit", "period", "basis", "unit", "note"],
        path,
        cx,
    );
    if cx.hard > before {
        return None;
    }
    Some(ValueModel {
        usd_per_unit: usd?,
        period: period.unwrap_or(ValuePeriod::Year),
        basis: basis.unwrap_or(ValueBasis::Estimated),
        unit,
        note,
    })
}

fn objective(v: Option<&Value>, path: &[Seg], cx: &mut Cx) -> Option<Objective> {
    let map = object(v, true, path, cx)?;
    let before = cx.hard;
    let name = plain_string(get(map, "name"), false, &child(path, "name"), cx);
    let metric = non_empty(
        get(map, "metric"),
        true,
        "objective.metric",
        &child(path, "metric"),
        cx,
    );
    let direction = enumeration::<Direction>(
        get(map, "direction"),
        true,
        "direction must be \"minimize\" or \"maximize\"",
        &child(path, "direction"),
        cx,
    );
    let min_gain = {
        let p = child(path, "min_gain");
        let n = number(
            get(map, "min_gain"),
            false,
            "min_gain must be a number (a fraction, e.g. 0.03 for 3%)",
            &p,
            cx,
        );
        match n {
            Some(n) => {
                let mut valid = true;
                if n < 0.0 {
                    cx.push(&p, "min_gain must be >= 0");
                    valid = false;
                }
                if n > 1.0 {
                    cx.push(
                        &p,
                        "min_gain is a fraction and must be <= 1 (0.03 means 3%)",
                    );
                    valid = false;
                }
                valid.then_some(n)
            }
            None => None,
        }
    };
    let value = value_model(get(map, "value"), &child(path, "value"), cx);
    unknown_keys(
        map,
        &["name", "metric", "direction", "min_gain", "value"],
        path,
        cx,
    );
    if cx.hard > before {
        return None;
    }
    Some(Objective {
        name,
        metric: metric?,
        direction: direction?,
        min_gain,
        value,
    })
}

fn environment(v: Option<&Value>, path: &[Seg], cx: &mut Cx) -> Option<Environment> {
    let map = object(v, false, path, cx)?;
    let before = cx.hard;
    let image = repo_path(
        get(map, "image"),
        "environment.image",
        &child(path, "image"),
        cx,
    );
    let machine = {
        let p = child(path, "machine");
        let msg = "environment.machine must be an instance type such as c7i.2xlarge";
        let s = string(
            get(map, "machine"),
            false,
            &StrRules {
                type_msg: Some(msg),
                empty_msg: None,
            },
            &p,
            cx,
        );
        match s {
            Some(s) if !is_instance_type(&s) => {
                cx.push(&p, msg);
                None
            }
            other => other,
        }
    };
    let region = {
        let p = child(path, "region");
        let type_msg = "environment.region must be a region such as us-east-1";
        let s = string(
            get(map, "region"),
            false,
            &StrRules {
                type_msg: Some(type_msg),
                empty_msg: None,
            },
            &p,
            cx,
        );
        match s {
            Some(s) if !is_aws_region(&s) => {
                cx.push(
                    &p,
                    "environment.region must be an AWS region such as us-east-1",
                );
                None
            }
            other => other,
        }
    };
    let network = {
        let p = child(path, "network");
        match get(map, "network") {
            None => None,
            Some(v) => match untag(v) {
                Value::Sequence(items) => {
                    let mut hosts = Vec::new();
                    let mut ok = true;
                    for (i, item) in items.iter().enumerate() {
                        let ip = at(&p, i);
                        match untag(item) {
                            Value::String(s) if is_host(s) => hosts.push(s.clone()),
                            Value::String(_) => {
                                cx.push(
                                    &ip,
                                    "environment.network entries must be host names such as registry.npmjs.org (no scheme, path or port; \"*.\" wildcards are allowed)",
                                );
                                ok = false;
                            }
                            _ => {
                                cx.push(&ip, "environment.network entries must be host names");
                                ok = false;
                            }
                        }
                    }
                    ok.then_some(hosts)
                }
                _ => {
                    cx.push(&p, "environment.network must be a list of host names");
                    None
                }
            },
        }
    };
    unknown_keys(map, &["image", "machine", "region", "network"], path, cx);
    if cx.hard > before {
        return None;
    }
    Some(Environment {
        image,
        machine,
        region,
        network,
    })
}

fn string_list(
    v: Option<&Value>,
    list_msg: &str,
    item_what: &str,
    path: &[Seg],
    cx: &mut Cx,
) -> Option<Option<Vec<String>>> {
    match v {
        None => Some(None),
        Some(v) => match untag(v) {
            Value::Sequence(items) => {
                let mut out = Vec::new();
                let mut ok = true;
                for (i, item) in items.iter().enumerate() {
                    match non_empty(Some(item), true, item_what, &at(path, i), cx) {
                        Some(s) => out.push(s),
                        None => ok = false,
                    }
                }
                ok.then_some(Some(out))
            }
            _ => {
                cx.push(path, list_msg);
                None
            }
        },
    }
}

struct RawAgent {
    instructions: Option<String>,
    allowed_instance_types: Option<Vec<String>>,
}

fn agent(v: Option<&Value>, path: &[Seg], cx: &mut Cx) -> Option<RawAgent> {
    let map = object(v, false, path, cx)?;
    let before = cx.hard;
    let instructions = non_empty(
        get(map, "instructions"),
        false,
        "agent.instructions",
        &child(path, "instructions"),
        cx,
    );
    let p = child(path, "allowed_instance_types");
    let types = string_list(
        get(map, "allowed_instance_types"),
        "allowed_instance_types must be a list of strings",
        "instance type",
        &p,
        cx,
    );
    let allowed_instance_types = match types {
        Some(Some(list)) if list.is_empty() => {
            cx.push(
                &p,
                "allowed_instance_types must list at least one instance type",
            );
            None
        }
        Some(other) => other,
        None => None,
    };
    unknown_keys(map, &["instructions", "allowed_instance_types"], path, cx);
    if cx.hard > before {
        return None;
    }
    Some(RawAgent {
        instructions,
        allowed_instance_types,
    })
}

/// Validate a YAML document (already parsed to a generic value).
pub(crate) fn validate(doc: &Value) -> Result<RatchetConfig, Vec<ConfigError>> {
    let Value::Mapping(top) = untag(doc) else {
        return Err(vec![ConfigError {
            path: String::new(),
            message: "ratchet.yaml must be a mapping at the top level".into(),
        }]);
    };
    let mut cx = Cx::new();
    let root: Path = Vec::new();

    // `bench:` is the legacy alias of `evaluator:` (only when `evaluator:` is absent)
    let evaluator_value = get(top, "evaluator").or_else(|| get(top, "bench"));
    let bench_is_alias = get(top, "evaluator").is_none() && get(top, "bench").is_some();

    // version: z.literal(0)
    match get(top, "version").map(untag) {
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => {}
        _ => cx.push(
            &child(&root, "version"),
            "version must be 0 (the only supported ratchet.yaml version)",
        ),
    }
    let pack = plain_string(get(top, "pack"), false, &child(&root, "pack"), &mut cx);
    let objective = objective(get(top, "objective"), &child(&root, "objective"), &mut cx);
    let evaluator = evaluator(evaluator_value, &child(&root, "evaluator"), &mut cx);
    let environment = environment(
        get(top, "environment"),
        &child(&root, "environment"),
        &mut cx,
    );
    let infra = repo_path(get(top, "infra"), "infra", &child(&root, "infra"), &mut cx);

    let mut guardrails: Option<Vec<Guardrail>> = None;
    if let Some(v) = get(top, "guardrails") {
        let p = child(&root, "guardrails");
        match untag(v) {
            Value::Sequence(items) => {
                let mut list = Vec::new();
                let mut ok = true;
                for (i, item) in items.iter().enumerate() {
                    match guardrail(item, &at(&p, i), &mut cx) {
                        Some(g) => list.push(g),
                        None => ok = false,
                    }
                }
                if ok {
                    guardrails = Some(list);
                }
            }
            _ => cx.push(&p, "guardrails must be a list"),
        }
    }

    let protected = match string_list(
        get(top, "protected"),
        "protected must be a list of globs",
        "protected glob",
        &child(&root, "protected"),
        &mut cx,
    ) {
        Some(list) => list.unwrap_or_default(),
        None => Vec::new(),
    };
    let agent = agent(get(top, "agent"), &child(&root, "agent"), &mut cx);

    // unknown top-level keys (a lone `bench:` is the alias, not unknown)
    let mut allowed = vec![
        "version",
        "pack",
        "objective",
        "evaluator",
        "environment",
        "infra",
        "guardrails",
        "protected",
        "agent",
    ];
    if bench_is_alias {
        allowed.push("bench");
    }
    unknown_keys(top, &allowed, &root, &mut cx);

    // Any issue but an unknown key stops the zod pipe before its transform.
    if cx.hard > 0 {
        return Err(cx.issues);
    }

    // ---- everything parsed: resolve defaults and the evaluator template (the zod transform)
    let (Some(objective), Some(raw_eval)) = (objective, evaluator) else {
        return Err(cx.issues);
    };
    let tpl = raw_eval
        .template
        .as_deref()
        .and_then(find_evaluator_template);
    let eval_path = child(&root, "evaluator");
    if let (Some(id), None) = (&raw_eval.template, &tpl) {
        cx.push(
            &child(&eval_path, "template"),
            format!(
                "unknown evaluator template {} (available: {})",
                quote(id),
                evaluator_template_ids().join(", ")
            ),
        );
    }
    let run = raw_eval
        .run
        .clone()
        .or_else(|| tpl.as_ref().map(|t| t.run.clone()));
    if run.is_none() && raw_eval.template.is_none() {
        cx.push(
            &child(&eval_path, "run"),
            "is required: give the evaluation command as `run`, or pick an evaluator `template`",
        );
    }
    if !cx.issues.is_empty() {
        return Err(cx.issues);
    }

    let guardrails = match guardrails {
        Some(g) => g,
        None => match &tpl {
            Some(t) => t
                .guardrails
                .iter()
                .filter_map(|(k, v)| {
                    let mut scratch = Cx::new();
                    guardrail_from_entry(&[(k.clone(), v.clone())], &[], &mut scratch)
                })
                .collect(),
            None => Vec::new(),
        },
    };

    let hardware = raw_eval
        .hardware
        .clone()
        .or_else(|| environment.as_ref().and_then(|e| e.machine.clone()))
        .or_else(|| tpl.as_ref().map(|t| t.machine.clone()))
        .unwrap_or_else(|| DEFAULT_BENCH_MACHINE.to_owned());
    let evaluator = Evaluator {
        kind: raw_eval
            .kind
            .or_else(|| tpl.as_ref().map(|t| t.evaluator_kind))
            .unwrap_or(EvaluatorKind::Benchmark),
        git_ref: raw_eval
            .git_ref
            .unwrap_or_else(|| DEFAULT_EVALUATOR_REF.to_owned()),
        run: run.unwrap_or_default(),
        template: raw_eval.template,
        repeats: raw_eval.repeats,
        noise_factor: raw_eval.noise_factor,
        hardware: hardware.clone(),
        deps: raw_eval.deps,
        confirm_trace: raw_eval.holdout.clone(),
        holdout: raw_eval.holdout,
    };
    let agent = {
        let a = agent;
        AgentConfig {
            instructions: a
                .as_ref()
                .and_then(|a| a.instructions.clone())
                .unwrap_or_else(|| DEFAULT_AGENT_INSTRUCTIONS.to_owned()),
            allowed_instance_types: a
                .and_then(|a| a.allowed_instance_types)
                .unwrap_or_else(|| vec![hardware]),
        }
    };
    Ok(RatchetConfig {
        version: 0,
        pack,
        objective,
        evaluator,
        environment,
        infra,
        guardrails,
        protected,
        agent,
    })
}
