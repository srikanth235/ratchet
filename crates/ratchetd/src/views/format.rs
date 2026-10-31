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
