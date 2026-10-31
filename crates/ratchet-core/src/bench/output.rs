//! Helpers for reading a benchmark command's output.
//!
//! Contract: the benchmark prints a JSON object to stdout; if it prints several, the LAST top-level JSON
//! object on stdout wins. Other text around it is ignored. The objective metric is looked up by dotted path
//! (`http_req_duration.p95`).
//!
//! Port of `packages/core/src/bench/output.ts`.

use serde_json::{Map, Value};

/// Extract the last top-level, parseable JSON object from arbitrary text.
pub fn extract_last_json_object(text: &str) -> Option<Map<String, Value>> {
    let bytes = text.as_bytes();
    let mut last: Option<Map<String, Value>> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let Some(end) = find_object_end(bytes, i) else {
            i += 1; // unbalanced; try the next '{'
            continue;
        };
        // `{` and `}` are ASCII, so these indices are char boundaries.
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text[i..=end]) {
            last = Some(map);
            i = end + 1;
            continue;
        }
        // not JSON (e.g. a `{` in log text): try the next `{`
        i += 1;
    }
    last
}

/// Index of the `}` matching the `{` at `start`, honouring JSON strings; `None` if unbalanced.
fn find_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if c == b'\\' {
                i += 1;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Look up a dotted path. Keys that themselves contain dots are handled by trying progressively longer
/// joined keys (so `a.b.c` finds `{"a.b": {"c": 1}}`).
pub fn lookup_path<'a>(obj: &'a Value, path: &str) -> Option<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    walk(obj, &parts, 0)
}

fn walk<'a>(cur: &'a Value, parts: &[&str], from: usize) -> Option<&'a Value> {
    if from >= parts.len() {
        return Some(cur);
    }
    let Value::Object(rec) = cur else {
        return None;
    };
    for to in (from + 1)..=parts.len() {
        let key = parts[from..to].join(".");
        if let Some(child) = rec.get(&key)
            && let Some(r) = walk(child, parts, to)
        {
            return Some(r);
        }
    }
    None
}

/// Why a metric could not be extracted (the message is user-facing).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MetricError(pub String);

/// Extract a finite numeric metric from bench stdout.
pub fn extract_metric(stdout: &str, metric_path: &str) -> Result<f64, MetricError> {
    let Some(obj) = extract_last_json_object(stdout) else {
        return Err(MetricError("no JSON object found on stdout".into()));
    };
    let root = Value::Object(obj);
    let Some(v) = lookup_path(&root, metric_path) else {
        return Err(MetricError(format!(
            "metric \"{metric_path}\" not found in JSON output"
        )));
    };
    match v.as_f64() {
        Some(n) if n.is_finite() => Ok(n),
        _ => Err(MetricError(format!(
            "metric \"{metric_path}\" is not a finite number (got {})",
            serde_json::to_string(v).unwrap_or_default()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Option<Map<String, Value>> {
        match v {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    // output.test.ts: extractLastJsonObject "finds a lone object"
    #[test]
    fn finds_a_lone_object() {
        assert_eq!(extract_last_json_object("{\"a\":1}"), obj(json!({"a": 1})));
    }

    // "takes the last object when there are several"
    #[test]
    fn takes_the_last_object_when_there_are_several() {
        assert_eq!(
            extract_last_json_object("{\"a\":1}\nnoise\n{\"a\":2}\n"),
            obj(json!({"a": 2}))
        );
    }

    // "ignores surrounding text, including stray braces"
    #[test]
    fn ignores_surrounding_text_including_stray_braces() {
        assert_eq!(
            extract_last_json_object(
                "starting {not json}\nrunning...\n{\"x\":{\"y\":[1,2,{\"z\":3}]}}\ndone"
            ),
            obj(json!({"x": {"y": [1, 2, {"z": 3}]}}))
        );
    }

    // "handles braces and escaped quotes inside strings"
    #[test]
    fn handles_braces_and_escaped_quotes_inside_strings() {
        assert_eq!(
            extract_last_json_object(r#"{"s":"a } \" { b"}"#),
            obj(json!({"s": "a } \" { b"}))
        );
    }

    // "handles pretty-printed multi-line JSON"
    #[test]
    fn handles_pretty_printed_multi_line_json() {
        assert_eq!(
            extract_last_json_object("log\n{\n  \"m\": {\n    \"p95\": 12.5\n  }\n}\n"),
            obj(json!({"m": {"p95": 12.5}}))
        );
    }

    // "returns undefined when there is none"
    #[test]
    fn returns_none_when_there_is_none() {
        assert_eq!(extract_last_json_object(""), None);
        assert_eq!(extract_last_json_object("hello {"), None);
        assert_eq!(extract_last_json_object("[1,2,3]"), None);
    }

    #[test]
    fn multibyte_text_around_the_object_is_fine() {
        assert_eq!(
            extract_last_json_object("héllo → {\"é\":1} ✓"),
            obj(json!({"é": 1}))
        );
    }

    // lookupPath "walks dotted paths"
    #[test]
    fn walks_dotted_paths() {
        assert_eq!(
            lookup_path(
                &json!({"http_req_duration": {"p95": 12}}),
                "http_req_duration.p95"
            ),
            Some(&json!(12))
        );
        assert_eq!(lookup_path(&json!({"a": 1}), "a"), Some(&json!(1)));
    }

    // "supports keys containing dots and parens"
    #[test]
    fn supports_keys_containing_dots_and_parens() {
        assert_eq!(
            lookup_path(&json!({"a.b": {"c": 3}}), "a.b.c"),
            Some(&json!(3))
        );
        assert_eq!(
            lookup_path(&json!({"m": {"values": {"p(95)": 7}}}), "m.values.p(95)"),
            Some(&json!(7))
        );
    }

    // "returns undefined for missing paths and non-objects"
    #[test]
    fn returns_none_for_missing_paths_and_non_objects() {
        assert_eq!(lookup_path(&json!({"a": {"b": 1}}), "a.c"), None);
        assert_eq!(lookup_path(&json!({"a": 1}), "a.b"), None);
        assert_eq!(lookup_path(&json!({}), "toString"), None);
    }

    // extractMetric "returns the number"
    #[test]
    fn extract_metric_returns_the_number() {
        assert_eq!(
            extract_metric("x\n{\"lat\":{\"p95\":42.5}}", "lat.p95"),
            Ok(42.5)
        );
    }

    // "explains failures"
    #[test]
    fn extract_metric_explains_failures() {
        assert_eq!(
            extract_metric("nothing", "lat.p95"),
            Err(MetricError("no JSON object found on stdout".into()))
        );
        let e = extract_metric("{\"a\":1}", "lat.p95").unwrap_err();
        assert!(e.0.contains("not found"), "{e}");
        let e = extract_metric("{\"lat\":{\"p95\":\"fast\"}}", "lat.p95").unwrap_err();
        assert!(e.0.contains("not a finite number"), "{e}");
        assert!(e.0.ends_with("(got \"fast\")"), "{e}");
    }
}
