//! The region of an optimization comes from the repo (`environment.region` in ratchet.yaml), not from a global
//! setting; the account default is `us-east-1`. (TS `region.ts`)

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::util::json_str;

pub const DEFAULT_REGION: &str = "us-east-1";

static REGION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]{2}(?:-[a-z]+)+-[0-9]$").expect("static regex"));

pub fn is_region(r: &str) -> bool {
    REGION_RE.is_match(r)
}

/// Defensive read of `config.environment.region` (any other shape is ignored). A value that is present but is not a
/// region name is an error: silently falling back would launch machines in a region the repo did not ask for.
///
/// TODO(schema): take the typed `config.environment.region` once ratchet-core declares it.
pub fn environment_region(config: &Value) -> Result<Option<String>> {
    let Some(env) = config.get("environment").filter(|e| e.is_object()) else {
        return Ok(None);
    };
    match env.get("region") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) if is_region(s) => Ok(Some(s.clone())),
        Some(other) => Err(Error::invalid(format!(
            "environment.region {} is not an AWS region name (like us-east-1)",
            serde_json::to_string(other).unwrap_or_default()
        ))),
    }
}

/// The region for one optimization: the repo's, else the fallback (settings), else `us-east-1`.
pub fn region_for_optimization(config: &Value, fallback: Option<&str>) -> Result<String> {
    let r = match environment_region(config)? {
        Some(r) => r,
        None => fallback.unwrap_or(DEFAULT_REGION).to_string(),
    };
    if !is_region(&r) {
        return Err(Error::invalid(format!(
            "region {} is not an AWS region name (like us-east-1)",
            json_str(&r)
        )));
    }
    Ok(r)
}
