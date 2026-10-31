//! Small shell helpers and constants shared by every generated script (bench steps, the dependency-snapshot job and the
//! container image job). Free of other imports from this crate so that none of the script builders import each other in
//! a cycle. Port of `shell.ts`.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

use crate::errors::{BenchError, BenchResult};

pub const SENTINEL: &str = "@@RATCHET@@";

/// Unprivileged OS user that runs setup, guardrails and every process of the CANDIDATE (its server, its build).
/// Created and IMDS-blocked by the instance's user-data (`benchUserData`).
pub const BENCH_USER: &str = "ratchet-bench";
/// Second unprivileged user: the evaluator harness (`evaluator.run`, trusted code from bench.ref) runs as this user and
/// is the only one that can open the holdout (file mode 0600). Also IMDS-blocked by the user-data.
pub const HOLDOUT_USER: &str = "ratchet-holdout";

/// POSIX single-quote a string. A NUL byte cannot be passed in a shell argument: the script builders reject it up front
/// with [`reject_nul`] (the TypeScript `sq` threw).
pub fn sq(s: &str) -> String {
    debug_assert!(!s.contains('\0'), "NUL byte in a shell argument");
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `BenchError` when any of `values` contains a NUL byte (they end up in shell scripts).
pub fn reject_nul<'a>(values: impl IntoIterator<Item = &'a str>) -> BenchResult<()> {
    if values.into_iter().any(|v| v.contains('\0')) {
        return Err(BenchError::infra("NUL byte in a shell argument"));
    }
    Ok(())
}

static SAFE_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/@{}~^+-]*$").expect("static regex"));

/// A sha / ref is interpolated into git commands; keep it to plain ref characters and never leading '-'.
pub fn assert_safe_ref<'a>(r: &'a str, what: &str) -> BenchResult<&'a str> {
    if !SAFE_REF.is_match(r) || r.contains("..") {
        return Err(BenchError::infra(format!(
            "{what} {} is not a plain git ref or sha",
            json_str(r)
        )));
    }
    Ok(r)
}

/// `JSON.stringify` of a string.
pub fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Order of JavaScript's default `Array.prototype.sort` on strings: by UTF-16 code unit.
pub fn js_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Sort strings the way JavaScript's default `sort()` does.
pub fn js_sort(v: &mut [String]) {
    v.sort_by(|a, b| js_cmp(a, b));
}

/// Fill `{{name}}` placeholders of a script template in one pass (substituted text is not rescanned). Only
/// `{{identifier}}` tokens are placeholders (docker's `{{.Id}}` and `{{json .X}}` are left alone); an identifier that is
/// not in `vars` is a programming error and panics, so a typo can never leak into a generated script.
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(e)
                if e > 0
                    && after[..e]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                let name = &after[..e];
                match vars.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(v),
                    None => panic!("unfilled placeholder {{{{{name}}}}} in a script template"),
                }
                rest = &after[e + 2..];
            }
            _ => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `new Date(ms).toISOString()`.
pub fn iso_timestamp(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s, milli) = (
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000,
    );
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_identifiers_only() {
        let out = fill("a {{x}} b {{.Id}} c {{json .Y}} {{x}}", &[("x", "{{y}}")]);
        assert_eq!(out, "a {{y}} b {{.Id}} c {{json .Y}} {{y}}");
    }

    #[test]
    #[should_panic(expected = "unfilled placeholder")]
    fn fill_panics_on_unknown_identifier() {
        fill("{{nope}}", &[]);
    }

    #[test]
    fn iso() {
        assert_eq!(iso_timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_timestamp(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(iso_timestamp(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn js_order_is_utf16() {
        let mut v = vec!["\u{FF5E}".to_string(), "\u{1F600}".to_string()];
        js_sort(&mut v);
        assert_eq!(v[0], "\u{1F600}"); // astral (surrogate D83D) sorts before U+FF5E in UTF-16
    }
}
