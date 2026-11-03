//! Guardrails: hard pass/fail checks a candidate must satisfy (`guardrails:` in ratchet.yaml).
//!
//! Port of `packages/core/src/config/guardrails.ts` and the guardrail parts of `config/schema.ts`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::numfmt::js_number_to_string;

/// Comparison operators allowed in guardrail threshold strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum Comparator {
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "==")]
    Eq,
}

impl Comparator {
    /// In the order they are listed in error messages.
    pub const ALL: [Comparator; 5] = [Self::Lt, Self::Le, Self::Gt, Self::Ge, Self::Eq];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Eq => "==",
        }
    }

    /// `a <op> b`.
    pub fn compare(self, a: f64, b: f64) -> bool {
        match self {
            Self::Lt => a < b,
            Self::Le => a <= b,
            Self::Gt => a > b,
            Self::Ge => a >= b,
            Self::Eq => a == b,
        }
    }
}

impl std::fmt::Display for Comparator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A guardrail that runs a command; passes iff exit status is 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CommandGuardrail {
    pub run: String,
}

/// A guardrail that compares a metric reported by the benchmark/run to a bound.
///
/// `"< 0.001"` is `{ op: <, value: 0.001, relative: false }`;
/// `"< 1.2x"` is `{ op: <, value: 1.2, relative: true }` (observed < 1.2 * baseline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ThresholdGuardrail {
    pub metric: String,
    pub op: Comparator,
    pub value: f64,
    /// True when written with an `x` suffix: the bound is a multiple of the baseline value.
    pub relative: bool,
    /// The original string, e.g. `"< 1.2x"`.
    pub raw: String,
}

/// A parsed guardrail. Serialized with a `kind` tag, as the TypeScript objects were.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Guardrail {
    Command(CommandGuardrail),
    Threshold(ThresholdGuardrail),
}

impl Guardrail {
    /// Human-readable name, e.g. `pytest -q` or `error_rate < 0.001`.
    pub fn name(&self) -> String {
        guardrail_name(self)
    }
}

/// Result of evaluating one guardrail; this is the shape the ratchet rule consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct GuardrailResult {
    /// Human-readable name, e.g. `pytest -q` or `error_rate < 0.001`.
    pub name: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub fn guardrail_name(g: &Guardrail) -> String {
    match g {
        Guardrail::Command(c) => c.run.clone(),
        Guardrail::Threshold(t) => format!("{} {}", t.metric, t.raw),
    }
}

/// JavaScript's `\s` (also what `String.prototype.trim` strips).
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{000B}' | '\u{000C}' | '\r' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// Parse `<comparator> <number>[x]` after trimming; `None` when it does not have that shape.
///
/// Equivalent to `/^\s*(<=|>=|==|<|>)\s*([+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)\s*(x?)\s*$/`.
fn match_threshold(raw: &str) -> Option<(Comparator, &str, bool)> {
    let s = raw.trim_start_matches(is_js_space);
    let (op, rest) = if let Some(r) = s.strip_prefix("<=") {
        (Comparator::Le, r)
    } else if let Some(r) = s.strip_prefix(">=") {
        (Comparator::Ge, r)
    } else if let Some(r) = s.strip_prefix("==") {
        (Comparator::Eq, r)
    } else if let Some(r) = s.strip_prefix('<') {
        (Comparator::Lt, r)
    } else {
        (Comparator::Gt, s.strip_prefix('>')?)
    };
    let rest = rest.trim_start_matches(is_js_space);
    let b = rest.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let int_len = digits(i);
    if int_len > 0 {
        i += int_len;
        if b.get(i) == Some(&b'.') {
            i += 1;
            i += digits(i);
        }
    } else if b.get(i) == Some(&b'.') && digits(i + 1) > 0 {
        i += 1 + digits(i + 1);
    } else {
        return None;
    }
    // optional exponent; only taken when complete (the regex backtracks otherwise, and the leftover then
    // fails to match the suffix, so "1e" is rejected either way)
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let d = digits(j);
        if d > 0 {
            i = j + d;
        }
    }
    let number = &rest[..i];
    let tail = rest[i..].trim_start_matches(is_js_space);
    let (relative, tail) = match tail.strip_prefix('x') {
        Some(t) => (true, t),
        None => (false, tail),
    };
    if !tail.trim_start_matches(is_js_space).is_empty() {
        return None;
    }
    Some((op, number, relative))
}

fn json_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// Parse a guardrail threshold string such as `"< 0.001"` or `"<= 1.2x"`.
///
/// Returns an error message when the string is malformed.
pub fn parse_threshold(metric: &str, raw: &str) -> Result<ThresholdGuardrail, String> {
    let Some((op, number, relative)) = match_threshold(raw) else {
        let ops: Vec<&str> = Comparator::ALL.iter().map(|c| c.as_str()).collect();
        return Err(format!(
            "invalid threshold {}: expected a comparator ({}) followed by a number, optionally suffixed with \"x\" for a multiple of baseline (e.g. \"< 0.001\" or \"< 1.2x\")",
            json_quote(raw),
            ops.join(", ")
        ));
    };
    let value: f64 = number.parse().unwrap_or(f64::NAN);
    if !value.is_finite() {
        return Err(format!(
            "invalid threshold {}: number is not finite",
            json_quote(raw)
        ));
    }
    if relative && value <= 0.0 {
        return Err(format!(
            "invalid threshold {}: a baseline multiple must be greater than 0",
            json_quote(raw)
        ));
    }
    Ok(ThresholdGuardrail {
        metric: metric.to_owned(),
        op,
        value,
        relative,
        raw: js_trim(raw).to_owned(),
    })
}

/// Evaluate a threshold guardrail. For relative bounds (`"< 1.2x"`) the bound is `value * baseline`, so
/// `baseline` is required. A non-finite observation fails.
pub fn evaluate_threshold(
    g: &ThresholdGuardrail,
    observed: f64,
    baseline: Option<f64>,
) -> GuardrailResult {
    let name = guardrail_name(&Guardrail::Threshold(g.clone()));
    if !observed.is_finite() {
        return GuardrailResult {
            name,
            passed: false,
            detail: Some(format!(
                "{} was not a finite number ({})",
                g.metric,
                js_number_to_string(observed)
            )),
        };
    }
    let mut bound = g.value;
    if g.relative {
        match baseline {
            Some(b) if b.is_finite() => bound = g.value * b,
            _ => {
                return GuardrailResult {
                    name,
                    passed: false,
                    detail: Some(format!(
                        "{} needs a baseline value to evaluate \"{}\"",
                        g.metric, g.raw
                    )),
                };
            }
        }
    }
    let passed = g.op.compare(observed, bound);
    let mut detail = format!(
        "{} = {}, required {} {}",
        g.metric,
        js_number_to_string(observed),
        g.op,
        js_number_to_string(bound)
    );
    if g.relative {
        detail.push_str(&format!(
            " ({}x baseline {})",
            js_number_to_string(g.value),
            baseline
                .map(js_number_to_string)
                .unwrap_or_else(|| "undefined".into())
        ));
    }
    GuardrailResult {
        name,
        passed,
        detail: Some(detail),
    }
}

/// Exit-code based evaluation for command guardrails.
pub fn evaluate_command_result(g: &CommandGuardrail, exit_code: i32) -> GuardrailResult {
    GuardrailResult {
        name: g.run.clone(),
        passed: exit_code == 0,
        detail: if exit_code == 0 {
            None
        } else {
            Some(format!("exited with status {exit_code}"))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> ThresholdGuardrail {
        parse_threshold("m", s).unwrap_or_else(|e| panic!("{e}"))
    }

    // config.test.ts parseThreshold: it.each parses %j
    #[test]
    fn parses_thresholds() {
        let cases: [(&str, Comparator, f64, bool); 10] = [
            ("< 0.001", Comparator::Lt, 0.001, false),
            ("<=5", Comparator::Le, 5.0, false),
            (">= 0.99", Comparator::Ge, 0.99, false),
            ("> 10", Comparator::Gt, 10.0, false),
            ("== 0", Comparator::Eq, 0.0, false),
            ("< 1.2x", Comparator::Lt, 1.2, true),
            ("  <  2 x ", Comparator::Lt, 2.0, true),
            ("< 1e-3", Comparator::Lt, 0.001, false),
            ("< .5", Comparator::Lt, 0.5, false),
            ("> -1", Comparator::Gt, -1.0, false),
        ];
        for (raw, op, value, relative) in cases {
            let t = ok(raw);
            assert_eq!(
                (t.metric.as_str(), t.op, t.value, t.relative),
                ("m", op, value, relative),
                "{raw:?}"
            );
        }
    }

    // config.test.ts parseThreshold: it.each rejects %j
    #[test]
    fn rejects_malformed_thresholds() {
        for raw in [
            "", "0.001", "< ", "<< 1", "< abc", "< 1.2y", "< 1 2", "!= 3", "< 0x",
        ] {
            assert!(
                parse_threshold("m", raw).is_err(),
                "{raw:?} should be rejected"
            );
        }
    }

    #[test]
    fn threshold_error_messages_match_the_typescript_ones() {
        assert_eq!(
            parse_threshold("m", "about 0.001").unwrap_err(),
            "invalid threshold \"about 0.001\": expected a comparator (<, <=, >, >=, ==) followed by a number, optionally suffixed with \"x\" for a multiple of baseline (e.g. \"< 0.001\" or \"< 1.2x\")"
        );
        assert_eq!(
            parse_threshold("m", "< 0x").unwrap_err(),
            "invalid threshold \"< 0x\": a baseline multiple must be greater than 0"
        );
        assert_eq!(
            parse_threshold("m", "< 1e999").unwrap_err(),
            "invalid threshold \"< 1e999\": number is not finite"
        );
    }

    #[test]
    fn raw_is_trimmed_and_numbers_follow_javascript_grammar() {
        assert_eq!(ok("  < 1.2x  ").raw, "< 1.2x");
        assert_eq!(ok("< 5.").value, 5.0);
        assert_eq!(ok("< 1.e2").value, 100.0);
        assert_eq!(ok("< +3").value, 3.0);
        assert!(parse_threshold("m", "< 1e").is_err());
        assert!(parse_threshold("m", "< .").is_err());
        assert!(parse_threshold("m", "< 1.2.3").is_err());
    }

    fn g(raw: &str) -> ThresholdGuardrail {
        ok(raw)
    }

    // config.test.ts "guardrail evaluation": absolute thresholds
    #[test]
    fn absolute_thresholds() {
        assert!(evaluate_threshold(&g("< 0.001"), 0.0005, None).passed);
        assert!(!evaluate_threshold(&g("< 0.001"), 0.001, None).passed);
        assert!(evaluate_threshold(&g("<= 0.001"), 0.001, None).passed);
        assert!(!evaluate_threshold(&g(">= 0.99"), 0.98, None).passed);
    }

    // "relative thresholds need a baseline"
    #[test]
    fn relative_thresholds_need_a_baseline() {
        assert!(evaluate_threshold(&g("< 1.2x"), 110.0, Some(100.0)).passed);
        assert!(!evaluate_threshold(&g("< 1.2x"), 121.0, Some(100.0)).passed);
        let r = evaluate_threshold(&g("< 1.2x"), 110.0, None);
        assert!(!r.passed);
        assert!(r.detail.unwrap().contains("baseline"));
    }

    // "non-finite observations fail"
    #[test]
    fn non_finite_observations_fail() {
        let r = evaluate_threshold(&g("< 1"), f64::NAN, None);
        assert!(!r.passed);
        assert_eq!(r.detail.as_deref(), Some("m was not a finite number (NaN)"));
    }

    // "command results"
    #[test]
    fn command_results() {
        let c = CommandGuardrail {
            run: "pytest -q".into(),
        };
        assert_eq!(
            evaluate_command_result(&c, 0),
            GuardrailResult {
                name: "pytest -q".into(),
                passed: true,
                detail: None
            }
        );
        let r = evaluate_command_result(&c, 1);
        assert!(!r.passed);
        assert_eq!(r.detail.as_deref(), Some("exited with status 1"));
    }

    #[test]
    fn detail_strings_match_javascript_number_formatting() {
        let r = evaluate_threshold(&g("< 1.2x"), 110.0, Some(100.0));
        assert_eq!(
            r.detail.as_deref(),
            Some("m = 110, required < 120 (1.2x baseline 100)")
        );
        let r = evaluate_threshold(&g("< 0.001"), 1e-7, None);
        assert_eq!(r.detail.as_deref(), Some("m = 1e-7, required < 0.001"));
        assert_eq!(r.name, "m < 0.001");
    }

    #[test]
    fn guardrails_serialize_with_a_kind_tag() {
        let v = serde_json::to_value(Guardrail::Threshold(g("< 1.2x"))).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "threshold", "metric": "m", "op": "<", "value": 1.2, "relative": true, "raw": "< 1.2x"})
        );
        let v = serde_json::to_value(Guardrail::Command(CommandGuardrail {
            run: "pytest -q".into(),
        }))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "command", "run": "pytest -q"})
        );
        let back: Guardrail = serde_json::from_value(v).unwrap();
        assert_eq!(back.name(), "pytest -q");
    }
}
