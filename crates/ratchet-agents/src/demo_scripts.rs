//! The shipped demo scripts (`scripts/demo-search`, `demo-forecast`, `demo-cli`) for `ScriptedAgent`.
//!
//! A script is a JSON file whose `patch` steps name a `.patch` file in the same directory instead of carrying the diff; loading
//! inlines the patches. The directories live next to the crate sources; pass another directory to load a copy.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::scripted::{AgentScript, ScriptAttempt, ScriptStep};

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("cannot read {0}: {1}")]
    Read(PathBuf, String),
    #[error("{0} is not a valid script: {1}")]
    Parse(PathBuf, String),
}

pub fn demo_search_script_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/demo-search"))
}

pub fn demo_forecast_script_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/demo-forecast"))
}

pub fn demo_cli_script_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/demo-cli"))
}

/// The two agents every demo ships a script for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoVariant {
    Cc,
    Cx,
}

impl DemoVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            DemoVariant::Cc => "cc",
            DemoVariant::Cx => "cx",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cc" => Some(DemoVariant::Cc),
            "cx" => Some(DemoVariant::Cx),
            _ => None,
        }
    }
}

/// Read a script JSON and inline its patch files (`patch_dir` defaults to the script's own directory).
pub fn load_script(file: &Path, patch_dir: Option<&Path>) -> Result<AgentScript, ScriptError> {
    let text = std::fs::read_to_string(file).map_err(|e| ScriptError::Read(file.to_path_buf(), e.to_string()))?;
    let mut raw: Value = serde_json::from_str(&text).map_err(|e| ScriptError::Parse(file.to_path_buf(), e.to_string()))?;
    let dir = patch_dir.map(Path::to_path_buf).or_else(|| file.parent().map(Path::to_path_buf)).unwrap_or_default();
    let name = raw.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let mut attempts: Vec<ScriptAttempt> = Vec::new();
    for attempt in raw.get_mut("attempts").and_then(Value::as_array_mut).into_iter().flatten() {
        let attempt_name = attempt.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let mut steps: Vec<ScriptStep> = Vec::new();
        for step in attempt.get_mut("steps").and_then(Value::as_array_mut).into_iter().flatten() {
            if let Some(obj) = step.as_object_mut() {
                if obj.get("type").and_then(Value::as_str) == Some("patch") {
                    if let Some(Value::String(patch_file)) = obj.remove("file") {
                        let path = dir.join(&patch_file);
                        let diff = std::fs::read_to_string(&path).map_err(|e| ScriptError::Read(path.clone(), e.to_string()))?;
                        obj.insert("diff".to_string(), Value::String(diff));
                    }
                }
            }
            steps.push(serde_json::from_value(step.clone()).map_err(|e| ScriptError::Parse(file.to_path_buf(), e.to_string()))?);
        }
        attempts.push(ScriptAttempt { name: attempt_name, steps });
    }
    Ok(AgentScript { name, attempts })
}

/// The shipped demo-search scripts. `cc` and `cx` play the same six real fixes from `examples/demo-search/SOLUTIONS.md` in
/// different orders, interleaved with deliberate bad attempts (protected-path edit, contract break, noise-level no-op).
pub fn load_demo_search_script(variant: DemoVariant, dir: Option<&Path>) -> Result<AgentScript, ScriptError> {
    let dir = dir.map(Path::to_path_buf).unwrap_or_else(demo_search_script_dir);
    load_script(&dir.join(format!("{}.script.json", variant.as_str())), Some(&dir))
}

/// The shipped demo-forecast scripts. `cc` and `cx` play the five real improvements from `examples/demo-forecast/SOLUTIONS.md`
/// in different orders, interleaved with deliberate bad attempts (evaluator / data-generator edit, look-ahead leakage, a
/// backtest-overfit trick, a noise-level tweak).
pub fn load_demo_forecast_script(variant: DemoVariant, dir: Option<&Path>) -> Result<AgentScript, ScriptError> {
    let dir = dir.map(Path::to_path_buf).unwrap_or_else(demo_forecast_script_dir);
    load_script(&dir.join(format!("{}.script.json", variant.as_str())), Some(&dir))
}

/// The shipped demo-cli scripts. `cc` and `cx` play the seven real cold-start fixes from `examples/demo-cli/SOLUTIONS.md` in
/// different orders, interleaved with deliberate bad attempts (a bench-harness edit, a golden-file edit, an overfit to the visible
/// invocation set, a noise-level tweak).
pub fn load_demo_cli_script(variant: DemoVariant, dir: Option<&Path>) -> Result<AgentScript, ScriptError> {
    let dir = dir.map(Path::to_path_buf).unwrap_or_else(demo_cli_script_dir);
    load_script(&dir.join(format!("{}.script.json", variant.as_str())), Some(&dir))
}
