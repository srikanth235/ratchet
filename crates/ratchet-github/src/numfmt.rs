//! Number formatting that matches JavaScript, so reports and squash messages are byte-identical to the TS ones. The
//! JavaScript number routines (`toFixed`, `toPrecision`, `String(number)`) are `ratchet_core::numfmt`'s.

use serde::{Serialize, Serializer};

pub use ratchet_core::numfmt::to_fixed;
use ratchet_core::numfmt::{js_number_to_string, precision_trimmed};

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
    if n.is_finite() && n.fract() == 0.0 {
        js_number_to_string(n)
    } else {
        precision_trimmed(n, 5)
    }
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
