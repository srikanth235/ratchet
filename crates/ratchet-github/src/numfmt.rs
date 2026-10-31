//! Number formatting that matches JavaScript, so reports and squash messages are byte-identical to the TS ones.

use serde::{Serialize, Serializer};

/// A float as `serde_json` would print a JS number: integral values without `.0`.
pub fn js_number(n: f64) -> serde_json::Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15 {
        serde_json::Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
}

/// `serialize_with` for `f64` fields: integral values print as `412`, not `412.0`.
pub fn ser_num<S: Serializer>(n: &f64, s: S) -> Result<S::Ok, S::Error> {
    js_number(*n).serialize(s)
}

/// `serialize_with` for `Option<f64>` fields.
pub fn ser_opt_num<S: Serializer>(n: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match n {
        Some(n) => js_number(*n).serialize(s),
        None => s.serialize_none(),
    }
}

/// `Number.isInteger(n) ? String(n) : String(Number(n.toPrecision(5)))`.
pub fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() {
        return format!("{}", n as i64);
    }
    if !n.is_finite() {
        return if n.is_nan() {
            "NaN".into()
        } else if n > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    // five significant digits, then the shortest representation that round-trips
    let five = format!("{:.4e}", n);
    let rounded: f64 = five.parse().unwrap_or(n);
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// `n.toFixed(digits)` for the magnitudes reports deal with.
pub fn to_fixed(n: f64, digits: usize) -> String {
    format!("{n:.digits$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_js_number_to_string() {
        assert_eq!(num(412.0), "412");
        assert_eq!(num(0.02), "0.02");
        assert_eq!(num(123456.789), "123460");
        assert_eq!(num(1.23456789), "1.2346");
        assert_eq!(num(-3.5), "-3.5");
        assert_eq!(js_number(180.0).to_string(), "180");
        assert_eq!(
            js_number(56.310679611650485).to_string(),
            "56.310679611650485"
        );
    }
}
