//! Number formatting helpers that reproduce JavaScript semantics where the TypeScript code relied on them
//! (`Number.prototype.toFixed`, `toPrecision` and `String(number)`).
//!
//! JavaScript rounds the exact binary value half-up (an exact tie goes to the larger magnitude); Rust's
//! formatter rounds ties to even. The helpers here work on the exact decimal expansion of the double, so they
//! agree with V8 on every input, ties included.

/// Exact decimal digits of a positive finite double: `d0.d1d2d3... x 10^exp10`, with enough digits to be exact.
fn exact_digits(x: f64) -> (Vec<u8>, i32) {
    debug_assert!(x > 0.0 && x.is_finite());
    // 800 fractional digits cover the longest exact expansion of any double (767 significant digits).
    let s = format!("{:.800e}", x);
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let digits: Vec<u8> = mant
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    (digits, exp.parse().unwrap_or(0))
}

/// Add one to a decimal digit vector (most significant first). Returns true when the length grew.
fn increment(digits: &mut Vec<u8>) -> bool {
    for d in digits.iter_mut().rev() {
        if *d == 9 {
            *d = 0;
        } else {
            *d += 1;
            return false;
        }
    }
    digits.insert(0, 1);
    true
}

fn digits_to_string(d: &[u8]) -> String {
    d.iter().map(|d| char::from(b'0' + d)).collect()
}

/// `Number.prototype.toFixed(digits)` for finite numbers below 1e21 (larger ones print in exponent form, as in
/// JavaScript, via [`js_number_to_string`]).
pub fn to_fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if x < 0.0 {
        return format!("-{}", to_fixed(-x, digits));
    }
    if x >= 1e21 {
        return js_number_to_string(x);
    }
    let int_str = if x == 0.0 {
        "0".to_string()
    } else {
        let (d, e) = exact_digits(x);
        let k = i64::from(e) + 1 + digits as i64; // significant digits kept
        if k < 0 {
            "0".to_string()
        } else {
            let k = k as usize;
            let mut kept: Vec<u8> = d[..k.min(d.len())].to_vec();
            kept.resize(k, 0);
            if d.get(k).copied().unwrap_or(0) >= 5 {
                if kept.is_empty() {
                    kept = vec![1];
                } else {
                    increment(&mut kept);
                }
            }
            if kept.is_empty() {
                "0".to_string()
            } else {
                digits_to_string(&kept)
            }
        }
    };
    if digits == 0 {
        return int_str;
    }
    let padded = if int_str.len() <= digits {
        format!("{}{}", "0".repeat(digits + 1 - int_str.len()), int_str)
    } else {
        int_str
    };
    let split = padded.len() - digits;
    format!("{}.{}", &padded[..split], &padded[split..])
}

/// `Number.prototype.toPrecision(p)` for `1 <= p <= 100`.
pub fn to_precision(x: f64, p: usize) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if x < 0.0 {
        return format!("-{}", to_precision(-x, p));
    }
    let p = p.clamp(1, 100);
    let (mut digits, mut e): (Vec<u8>, i32) = if x == 0.0 {
        (vec![0; p], 0)
    } else {
        let (d, e) = exact_digits(x);
        let mut kept = d[..p].to_vec();
        let mut e = e;
        if d[p] >= 5 && increment(&mut kept) {
            kept.truncate(p);
            e += 1;
        }
        (kept, e)
    };
    digits.truncate(p);
    if x != 0.0 && (e < -6 || e >= p as i32) {
        // exponential notation
        let sign = if e >= 0 { '+' } else { '-' };
        let mant = if p == 1 {
            digits_to_string(&digits)
        } else {
            format!("{}.{}", digits[0], digits_to_string(&digits[1..]))
        };
        return format!("{mant}e{sign}{}", e.abs());
    }
    if x == 0.0 {
        e = 0;
    }
    let s = digits_to_string(&digits);
    if e == p as i32 - 1 {
        return s;
    }
    if e >= 0 {
        let split = (e + 1) as usize;
        return format!("{}.{}", &s[..split], &s[split..]);
    }
    format!("0.{}{}", "0".repeat((-(e + 1)) as usize), s)
}

/// `String(x)` for a JavaScript number (shortest round-trip digits; exponent form below 1e-6 and from 1e21).
pub fn js_number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if x < 0.0 {
        return format!("-{}", js_number_to_string(-x));
    }
    let s = format!("{x:e}"); // shortest round-trip digits, e.g. "1.2345e-7"
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let shortest: String = mant.chars().filter(char::is_ascii_digit).collect();
    let mut n = exp.parse::<i32>().unwrap_or(0) + 1; // position of the decimal point
    // Rust and ECMAScript agree on the number of digits but not on which k-digit decimal to print when two of
    // them round-trip: ECMAScript takes the one closest to the exact value, an exact tie going to the even digit.
    let digits = nearest_k_digits(x, shortest.len(), &mut n).unwrap_or(shortest);
    let k = digits.len() as i32;
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e >= 0 { '+' } else { '-' };
        if k == 1 {
            format!("{digits}e{sign}{}", e.abs())
        } else {
            format!("{}.{}e{sign}{}", &digits[..1], &digits[1..], e.abs())
        }
    }
}

/// The `k`-digit decimal nearest to the positive finite `x` (an exact tie rounds to the even digit), as digits
/// without trailing zeros, with `n` set to the decimal point position. `None` when it does not read back as `x`.
fn nearest_k_digits(x: f64, k: usize, n: &mut i32) -> Option<String> {
    let (d, e) = exact_digits(x);
    if k == 0 || k >= d.len() {
        return None;
    }
    let mut kept = d[..k].to_vec();
    let mut e = e;
    let rest = &d[k..];
    let up = match rest[0].cmp(&5) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => rest[1..].iter().any(|&b| b != 0) || kept[k - 1] % 2 == 1,
    };
    if up && increment(&mut kept) {
        kept.truncate(k);
        e += 1;
    }
    while kept.len() > 1 && kept.last() == Some(&0) {
        kept.pop();
    }
    let text = digits_to_string(&kept);
    let candidate = format!("{}.{}e{e}", &text[..1], &text[1..]);
    (candidate.parse::<f64>().ok() == Some(x)).then(|| {
        *n = e + 1;
        text
    })
}

/// `String(Number(x.toFixed(digits)))`.
pub fn fixed_trimmed(x: f64, digits: usize) -> String {
    let parsed: f64 = to_fixed(x, digits).parse().unwrap_or(x);
    js_number_to_string(parsed)
}

/// `String(Number(x.toPrecision(precision)))`: the value rounded to `precision` significant digits and printed
/// the way JavaScript prints a number.
pub fn precision_trimmed(x: f64, precision: usize) -> String {
    let parsed: f64 = to_precision(x, precision).parse().unwrap_or(x);
    js_number_to_string(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values come from node (see tests/golden.rs for the bulk comparison against V8).
    #[test]
    fn to_fixed_matches_javascript() {
        assert_eq!(to_fixed(0.25, 1), "0.3");
        assert_eq!(to_fixed(0.35, 1), "0.3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(0.5, 0), "1");
        assert_eq!(to_fixed(9.5, 0), "10");
        assert_eq!(to_fixed(0.0, 2), "0.00");
        assert_eq!(to_fixed(-0.0, 1), "0.0");
        assert_eq!(to_fixed(-0.25, 1), "-0.3");
        assert_eq!(to_fixed(0.015, 6), "0.015000");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(0.375, 2), "0.38");
        assert_eq!(to_fixed(0.04, 1), "0.0");
        assert_eq!(to_fixed(0.0004, 1), "0.0");
        assert_eq!(to_fixed(123.456, 0), "123");
        assert_eq!(to_fixed(99.95, 1), "100.0");
        assert_eq!(to_fixed(1e21, 2), "1e+21");
    }

    #[test]
    fn to_precision_matches_javascript() {
        assert_eq!(to_precision(1234565.0, 6), "1.23457e+6");
        assert_eq!(to_precision(10.0, 6), "10.0000");
        assert_eq!(to_precision(0.0, 6), "0.00000");
        assert_eq!(to_precision(0.000123456789, 6), "0.000123457");
        assert_eq!(to_precision(1e-7, 6), "1.00000e-7");
        assert_eq!(to_precision(123456.0, 6), "123456");
        assert_eq!(to_precision(999999.5, 6), "1.00000e+6");
        assert_eq!(to_precision(3.0, 6), "3.00000");
        assert_eq!(to_precision(-2.5, 1), "-3");
    }

    #[test]
    fn js_number_to_string_matches_javascript() {
        assert_eq!(js_number_to_string(1.0), "1");
        assert_eq!(js_number_to_string(0.1), "0.1");
        assert_eq!(js_number_to_string(0.0001), "0.0001");
        assert_eq!(js_number_to_string(1e-7), "1e-7");
        assert_eq!(js_number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(js_number_to_string(123456789012.0), "123456789012");
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1.2345e25), "1.2345e+25");
        assert_eq!(js_number_to_string(-0.0), "0");
        assert_eq!(js_number_to_string(100.0), "100");
        assert_eq!(js_number_to_string(0.000001), "0.000001");
    }

    #[test]
    fn trimmed_helpers() {
        assert_eq!(fixed_trimmed(1.0, 1), "1");
        assert_eq!(fixed_trimmed(0.9, 1), "0.9");
        assert_eq!(fixed_trimmed(1.5, 1), "1.5");
        assert_eq!(fixed_trimmed(0.0, 1), "0");
        assert_eq!(fixed_trimmed(0.04, 1), "0");
        assert_eq!(precision_trimmed(1234565.0, 6), "1234570");
        assert_eq!(precision_trimmed(97.123456789, 6), "97.1235");
        assert_eq!(precision_trimmed(3.0, 6), "3");
    }
}
