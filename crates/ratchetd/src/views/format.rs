//! Small pure helpers of the mappers: relative times, number text, metric units.

pub const MIN_MS: i64 = 60_000;
pub const HOUR_MS: i64 = 60 * MIN_MS;
pub const DAY_MS: i64 = 24 * HOUR_MS;

/// "just now", "12m ago", "3h ago", "yesterday", "5d ago".
pub fn rel_time(at: i64, now: i64) -> String {
    let d = (now - at).max(0);
    if d < MIN_MS {
        "just now".into()
    } else if d < HOUR_MS {
        format!("{}m ago", d / MIN_MS)
    } else if d < DAY_MS {
        format!("{}h ago", d / HOUR_MS)
    } else {
        let days = d / DAY_MS;
        if days == 1 {
            "yesterday".into()
        } else {
            format!("{days}d ago")
        }
    }
}

/// `Number.isInteger(n) ? String(n) : String(Number(n.toFixed(2)))`.
pub fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let r = (n * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        // shortest decimal that reads back (like JS)
        format!("{r}")
    }
}

/// `x.toPrecision(6)` trimmed like `String(Number(...))`.
pub fn fmt_sig6(n: f64) -> String {
    ratchet_core::numfmt::precision_trimmed(n, 6)
}

/// Round to `dp` decimals.
pub fn round_to(n: f64, dp: i32) -> f64 {
    let m = 10f64.powi(dp);
    (n * m).round() / m
}

/// Compact dollars for a headline: `$184`, `$1.20`, `$12.5k`, `$1.2M`. Under $10 keeps cents; from $1,000 one decimal (`.0` dropped).
pub fn usd_compact(n: f64) -> String {
    let n = if n.is_finite() { n } else { 0.0 };
    let a = n.abs();
    let (scaled, suffix) = if a >= 1_000_000.0 {
        (n / 1_000_000.0, "M")
    } else if a >= 1_000.0 {
        (n / 1_000.0, "k")
    } else {
        (n, "")
    };
    let text = if !suffix.is_empty() {
        let t = format!("{scaled:.1}");
        t.strip_suffix(".0").unwrap_or(&t).to_owned()
    } else if a < 10.0 {
        format!("{scaled:.2}")
    } else {
        format!("{scaled:.0}")
    };
    match text.strip_prefix('-') {
        Some(rest) => format!("-${rest}{suffix}"),
        None => format!("${text}{suffix}"),
    }
}

/// Dollars with thousands separators, trailing zeros dropped: `1,200`, `0.5`, `3.25`.
pub fn usd_plain(n: f64) -> String {
    let mut s = format!("{:.2}", n.abs());
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    let (int, frac) = s.split_once('.').unwrap_or((s.as_str(), ""));
    let mut out = String::new();
    for (i, c) in int.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    let int: String = out.chars().rev().collect();
    let body = if frac.is_empty() {
        int
    } else {
        format!("{int}.{frac}")
    };
    if n < 0.0 { format!("-{body}") } else { body }
}

/// A token count for a line of text: `812`, `34k`, `2.4M`, `1.2B`.
pub fn tokens_compact(n: i64) -> String {
    let a = n.max(0) as f64;
    let (scaled, suffix) = if a >= 1e9 {
        (a / 1e9, "B")
    } else if a >= 1e6 {
        (a / 1e6, "M")
    } else if a >= 1e3 {
        (a / 1e3, "k")
    } else {
        return format!("{}", a as i64);
    };
    let t = if scaled >= 100.0 {
        format!("{scaled:.0}")
    } else {
        format!("{scaled:.1}")
    };
    format!("{}{suffix}", t.strip_suffix(".0").unwrap_or(&t))
}

/// Best-effort display unit from a metric key such as `http_req_duration.p95`.
pub fn unit_for_metric(metric: &str) -> &'static str {
    let m = metric.to_lowercase();
    // (^|[._])(rss|mem|memory|heap)
    let starts_mem = m.char_indices().any(|(i, _)| {
        (i == 0 || matches!(m.as_bytes()[i - 1], b'.' | b'_'))
            && ["rss", "mem", "heap"].iter().any(|p| m[i..].starts_with(p))
    });
    if starts_mem || m.ends_with("_mb") {
        return "MB";
    }
    if ["throughput", "events_per_s", "per_s", "rps", "qps"]
        .iter()
        .any(|p| m.contains(p))
    {
        return "k/s";
    }
    if m.contains("cold_start") || m.ends_with(".s") || m.ends_with("_s") || m.contains("seconds") {
        return "s";
    }
    let has_pct = m
        .match_indices(".p")
        .any(|(i, _)| m.as_bytes().get(i + 2).is_some_and(|b| b.is_ascii_digit()));
    if m.contains("duration")
        || m.contains("latency")
        || m.contains("render")
        || has_pct
        || m.ends_with("_ms")
        || m.contains("ms")
    {
        return "ms";
    }
    ""
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        let now = 10 * DAY_MS;
        assert_eq!(rel_time(now - 10_000, now), "just now");
        assert_eq!(rel_time(now - 12 * MIN_MS, now), "12m ago");
        assert_eq!(rel_time(now - 3 * HOUR_MS, now), "3h ago");
        assert_eq!(rel_time(now - 30 * HOUR_MS, now), "yesterday");
        assert_eq!(rel_time(now - 5 * DAY_MS, now), "5d ago");
        assert_eq!(rel_time(now + 5, now), "just now");
    }

    #[test]
    fn numbers_read_like_the_typescript() {
        assert_eq!(fmt_num(182.0), "182");
        assert_eq!(fmt_num(2.4149), "2.41");
        assert_eq!(fmt_num(41.2), "41.2");
        assert_eq!(fmt_num(-3.0), "-3");
    }

    #[test]
    fn money_and_tokens_for_headlines() {
        assert_eq!(usd_compact(184.0), "$184");
        assert_eq!(usd_compact(1.2), "$1.20");
        assert_eq!(usd_compact(12_500.0), "$12.5k");
        assert_eq!(usd_compact(12_000.0), "$12k");
        assert_eq!(usd_compact(2_900.0), "$2.9k");
        assert_eq!(usd_compact(1_200_000.0), "$1.2M");
        assert_eq!(usd_compact(0.0), "$0.00");
        assert_eq!(usd_plain(1200.0), "1,200");
        assert_eq!(usd_plain(0.5), "0.5");
        assert_eq!(usd_plain(1_234_567.0), "1,234,567");
        assert_eq!(tokens_compact(812), "812");
        assert_eq!(tokens_compact(34_000), "34k");
        assert_eq!(tokens_compact(2_400_000), "2.4M");
        assert_eq!(tokens_compact(125_000_000), "125M");
    }

    #[test]
    fn units() {
        assert_eq!(unit_for_metric("http_req_duration.p95"), "ms");
        assert_eq!(unit_for_metric("peak_rss_mb"), "MB");
        assert_eq!(unit_for_metric("memory.peak"), "MB");
        assert_eq!(unit_for_metric("throughput.events_per_s"), "k/s");
        assert_eq!(unit_for_metric("cold_start.s"), "s");
        assert_eq!(unit_for_metric("wape"), "");
        assert_eq!(unit_for_metric("render.p99"), "ms");
    }
}
