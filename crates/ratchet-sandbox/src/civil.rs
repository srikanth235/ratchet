//! Tiny UTC calendar helpers (no date crate needed for two conversions).

pub(crate) const DAY_MS: i64 = 86_400_000;

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD` (UTC) of an epoch-ms instant, the only date format Cost Explorer accepts for DAILY granularity.
pub(crate) fn ymd(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(DAY_MS));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Epoch ms of a UTC date and time.
pub(crate) fn epoch_ms(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: i64) -> i64 {
    days_from_civil(y, m, d) * DAY_MS + ((hh * 60 + mm) * 60 + ss) * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_known_dates() {
        // 2026-10-28 13:30 UTC, the instant the cost-explorer tests use
        let t = epoch_ms(2026, 10, 28, 13, 30, 0);
        assert_eq!(t, 1_793_194_200_000);
        assert_eq!(ymd(t), "2026-10-28");
        assert_eq!(ymd(t + DAY_MS / 2), "2026-10-29");
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(-1), "1969-12-31");
        assert_eq!(ymd(epoch_ms(2028, 2, 29, 23, 59, 59)), "2028-02-29");
        assert_eq!(epoch_ms(2026, 1, 2, 3, 4, 5), 1_767_323_045_000);
    }
}
