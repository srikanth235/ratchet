//! Shared helpers: reserved tags, id validation, shell quoting. (TS `util.ts`)

use std::sync::LazyLock;

use regex::Regex;
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};

/// Reserved tag keys (infra/contract.md identity model).
pub const TAG_OPT: &str = "ratchet:opt";
pub const TAG_AGENT: &str = "ratchet:agent";
pub const TAG_ROLE: &str = "ratchet:role";
pub const TAG_SANDBOX: &str = "ratchet:sandbox";
pub const TAG_PURPOSE: &str = "ratchet:purpose";

pub const DEFAULT_AGENT_ROLE_PATH: &str = "/ratchet/agent/";

const MAX_ID_LEN: usize = 64;

static ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9-]+$").expect("static regex"));
static INSTANCE_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^i-[0-9a-z]{1,40}$").expect("static regex"));
static SANDBOX_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9-]{2,32}$").expect("static regex"));

/// `JSON.stringify` of a string, for error messages.
pub(crate) fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

/// Ids are substituted into IAM names, tag values, SSM paths and policy patterns, so they are restricted to
/// `[a-z0-9-]` (infra/contract.md).
pub fn assert_valid_id(kind: &str, id: &str) -> Result<()> {
    if id.is_empty() || id.chars().count() > MAX_ID_LEN || !ID_RE.is_match(id) {
        return Err(Error::invalid(format!(
            "invalid {kind} id {}: must match [a-z0-9-], 1-{MAX_ID_LEN} chars",
            json_str(id)
        )));
    }
    Ok(())
}

/// EC2 instance ids end up in API calls and filters (`*` is a wildcard in EC2 filters), so keep them plain.
pub fn assert_valid_instance_id(id: &str) -> Result<()> {
    if !INSTANCE_ID_RE.is_match(id) {
        return Err(Error::invalid(format!(
            "invalid instance id {}: must match i-[0-9a-z]+",
            json_str(id)
        )));
    }
    Ok(())
}

pub fn assert_valid_sandbox_id(id: &str) -> Result<()> {
    if !SANDBOX_ID_RE.is_match(id) {
        return Err(Error::invalid(format!(
            "invalid sandbox id {}: must match [a-z0-9-], 2-32 chars",
            json_str(id)
        )));
    }
    Ok(())
}

/// IAM names are limited to 64 chars; keep readable names and hash-shorten long ones.
pub fn bounded_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let digest = Sha1::digest(name.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let head: String = name.chars().take(max - 9).collect();
    format!("{head}-{}", &hex[..8])
}

/// Last path segment of an ARN (role/profile name).
pub fn arn_name(arn: &str) -> &str {
    match arn.rfind('/') {
        Some(i) => &arn[i + 1..],
        None => arn,
    }
}

/// Single-quote `s` for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A JSON number the way JavaScript would print `v`: whole values without a fraction (`2`, not `2.0`).
pub(crate) fn js_number(v: f64) -> serde_json::Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        serde_json::Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_name_keeps_short_names_and_hash_shortens_long_ones() {
        assert_eq!(bounded_name("ratchet-agent-a-b", 64), "ratchet-agent-a-b");
        let long = "x".repeat(100);
        let b = bounded_name(&long, 64);
        assert_eq!(b.chars().count(), 64);
        assert_eq!(&b[..55], &long[..55]);
        assert_eq!(b.as_bytes()[55], b'-');
        // sha1("x" * 100)[..8] as the TypeScript `boundedName` computes it: role names must stay stable across the port
        assert!(b.ends_with("-50e48369"));
        let long_role = format!("ratchet-agent-{}-{}", "o".repeat(60), "a".repeat(60));
        assert_eq!(
            bounded_name(&long_role, 64),
            "ratchet-agent-ooooooooooooooooooooooooooooooooooooooooo-77ef3e51"
        );
    }

    #[test]
    fn arn_name_is_the_last_segment() {
        assert_eq!(arn_name("arn:aws:iam::1:role/ratchet/agent/r1"), "r1");
        assert_eq!(arn_name("plain"), "plain");
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a'b"), r"'a'\''b'");
        assert_eq!(sh_quote(""), "''");
    }
}
