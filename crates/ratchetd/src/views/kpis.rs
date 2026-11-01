//! KPI cards of the Optimizations screen (port of `packages/desktop/src/main/kpis.ts`).
//!
//! Each card covers `range` and is compared with the equal-length window before it (no chip when that window has
//! no data). Everything comes from the ledger: ratchet moves, baselines, spend events and the optimizations'
//! `objective.value` columns. The value card appears only when some optimization has a value model.

use ratchet_core::enums::ValueBasis;
use ratchet_core::numfmt::to_fixed;
use ratchet_core::rows::{Optimization, OptimizationSummary, RatchetHistoryEntry};

use super::format::DAY_MS;
use super::value::{gain_vs_baseline, value_of_move};
use crate::api::{Kpi, KpiChange, KpiRange, Tone};

/// Sparkline resolution: samples between the first activity in the range and now.
const TREND_POINTS: usize = 10;

/// One verified step, flattened for the windows.
#[derive(Debug, Clone)]
pub struct Move {
    pub opt: String,
    pub at: i64,
    /// Gain vs the optimization's baseline after this move, percent (positive = better).
    pub gain_pct: f64,
    /// Yearly value of this step (`objective.value`), `None` when the optimization has no value model.
    pub usd_per_year: Option<f64>,
}

/// A spend event reduced to what the windows need.
#[derive(Debug, Clone)]
pub struct SpendAt {
    pub opt: String,
    pub at: i64,
    pub usd: f64,
}

/// One optimization with everything the KPIs read.
pub struct KpiInput<'a> {
    pub summary: &'a OptimizationSummary,
    pub row: &'a Optimization,
    pub history: &'a [RatchetHistoryEntry],
}

#[derive(Clone, Copy)]
struct Window {
    from: f64,
    to: f64,
}

impl Window {
    fn has(self, at: i64) -> bool {
        (at as f64) >= self.from && (at as f64) < self.to
    }
}

pub fn median(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = s.len() / 2;
    if s.len() % 2 == 1 {
        s[m]
    } else {
        (s[m - 1] + s[m]) / 2.0
    }
}

/// Median over optimizations of the gain vs baseline they had reached by `w.to` (only those with a win in `w`).
fn median_gain(moves: &[Move], w: Window) -> (f64, usize) {
    let mut last: std::collections::BTreeMap<&str, f64> = Default::default();
    for m in moves.iter().filter(|m| w.has(m.at)) {
        last.insert(m.opt.as_str(), m.gain_pct);
    }
    let v: Vec<f64> = last.values().copied().collect();
    (median(&v), v.len())
}

fn sign(n: f64) -> &'static str {
    if n > 0.0 {
        "+"
    } else if n < 0.0 {
        "\u{2212}"
    } else {
        "\u{00b1}"
    }
}

fn tone_of(d: f64) -> Tone {
    if d > 0.0 {
        Tone::Good
    } else if d < 0.0 {
        Tone::Bad
    } else {
        Tone::Neutral
    }
}

fn count_change(cur: usize, prev: usize) -> KpiChange {
    let d = cur as f64 - prev as f64;
    KpiChange {
        text: format!("{}{}", sign(d), d.abs()),
        tone: tone_of(d),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Wants {
    Higher,
    Lower,
    Neutral,
}

/// Relative change; `wants` flips the tone; `None` when there is no previous value to divide by.
fn pct_change(cur: f64, prev: f64, wants: Wants) -> Option<KpiChange> {
    if prev <= 0.0 {
        return None;
    }
    let r = (((cur - prev) / prev) * 100.0).round();
    let tone = if wants == Wants::Neutral || r == 0.0 {
        Tone::Neutral
    } else if (r > 0.0) == (wants == Wants::Higher) {
        Tone::Good
    } else {
        Tone::Bad
    };
    Some(KpiChange {
        text: format!("{}{}%", sign(r), r.abs()),
        tone,
    })
}

fn group_thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// `$1.2M`, `$12.3k`, `$1,234`, `$42`, `$4.2`.
pub fn usd(v: f64) -> String {
    let a = v.abs();
    if a >= 1_000_000.0 {
        format!("${}M", to_fixed(v / 1_000_000.0, 1))
    } else if a >= 10_000.0 {
        format!("${}k", to_fixed(v / 1000.0, 1))
    } else if a >= 100.0 {
        format!("${}", group_thousands(v.round() as i64))
    } else {
        format!("${}", to_fixed(v, if a >= 10.0 { 0 } else { 1 }))
    }
}

/// `84.0x` style return multiples.
pub fn roi(x: f64) -> String {
    if x >= 1_000_000.0 {
        format!("{}M\u{00d7}", to_fixed(x / 1_000_000.0, 1))
    } else if x >= 10_000.0 {
        format!("{}k\u{00d7}", to_fixed(x / 1000.0, 1))
    } else {
        format!("{}\u{00d7}", to_fixed(x, if x >= 100.0 { 0 } else { 1 }))
    }
}

/// Flatten the ratchet histories into moves ordered by time.
pub fn moves_of(inputs: &[KpiInput<'_>]) -> Vec<Move> {
    let mut moves: Vec<Move> = inputs
        .iter()
        .flat_map(|i| {
            i.history.iter().map(move |m| Move {
                opt: i.summary.id.as_str().to_owned(),
                at: m.at,
                gain_pct: match i.summary.baseline_value {
                    None => m.gain_pct,
                    Some(b) => gain_vs_baseline(i.summary.direction, b, m.to_value),
                },
                usd_per_year: value_of_move(i.row, m.from_value, m.to_value),
            })
        })
        .collect();
    moves.sort_by_key(|m| m.at);
    moves
}

pub fn build_kpis(
    inputs: &[KpiInput<'_>],
    spend_events: &[SpendAt],
    now: i64,
    range: KpiRange,
) -> Vec<Kpi> {
    let len = (range.days() * DAY_MS) as f64;
    let nowf = now as f64;
    let cur = Window {
        from: nowf - len,
        to: nowf + 1.0,
    };
    let prev = Window {
        from: nowf - 2.0 * len,
        to: nowf - len,
    };

    let moves = moves_of(inputs);
    let ids: std::collections::BTreeSet<&str> =
        inputs.iter().map(|i| i.summary.id.as_str()).collect();
    let mut spend: Vec<&SpendAt> = spend_events
        .iter()
        .filter(|e| ids.contains(e.opt.as_str()))
        .collect();
    spend.sort_by_key(|e| e.at);

    let cur_moves: Vec<&Move> = moves.iter().filter(|m| cur.has(m.at)).collect();
    let prev_moves: Vec<&Move> = moves.iter().filter(|m| prev.has(m.at)).collect();
    let cur_spend: f64 = spend.iter().filter(|e| cur.has(e.at)).map(|e| e.usd).sum();
    let prev_spend: f64 = spend.iter().filter(|e| prev.has(e.at)).map(|e| e.usd).sum();
    // "Previous data" = the workspace did anything (spent or ratcheted) in the previous window.
    let had_prev = !prev_moves.is_empty() || spend.iter().any(|e| prev.has(e.at));

    // Sparklines: from the first activity inside the range to now, so a quiet start does not flatten the line.
    let first = cur_moves
        .iter()
        .map(|m| m.at as f64)
        .chain(spend.iter().filter(|e| cur.has(e.at)).map(|e| e.at as f64))
        .fold(nowf, f64::min);
    let ticks: Vec<f64> = (0..TREND_POINTS)
        .map(|i| first + ((nowf - first) * (i as f64 + 1.0)) / TREND_POINTS as f64)
        .collect();
    let up_to = |t: f64| Window {
        from: cur.from,
        to: t + 1.0,
    };
    let spend_to = |t: f64| {
        let w = up_to(t);
        spend
            .iter()
            .filter(|e| w.has(e.at))
            .map(|e| e.usd)
            .sum::<f64>()
    };
    let wins_to = |t: f64| cur_moves.iter().filter(|m| (m.at as f64) <= t).count();

    let mut kpis = Vec::new();

    // Verified wins
    let opts_with_wins = cur_moves
        .iter()
        .map(|m| m.opt.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    kpis.push(Kpi {
        id: "wins".into(),
        label: "Verified wins".into(),
        value: cur_moves.len().to_string(),
        unit: Some(format!("last {}", range_label(range))),
        change: had_prev.then(|| count_change(cur_moves.len(), prev_moves.len())),
        foot: if cur_moves.is_empty() {
            "none in this range".into()
        } else {
            format!(
                "across {opts_with_wins} optimization{}",
                if opts_with_wins == 1 { "" } else { "s" }
            )
        },
        trend: ticks.iter().map(|t| wins_to(*t) as f64).collect(),
    });

    // Median gain vs baseline of the optimizations that ratcheted in the range
    let (g, g_n) = median_gain(&moves, cur);
    let (g_prev, g_prev_n) = median_gain(&moves, prev);
    let gd = g.round() - g_prev.round();
    kpis.push(Kpi {
        id: "gain".into(),
        label: "Median gain".into(),
        value: if g_n > 0 {
            to_fixed(g, 0)
        } else {
            "\u{2014}".into()
        },
        unit: Some(if g_n > 0 {
            "% better".into()
        } else {
            String::new()
        }),
        change: (g_n > 0 && g_prev_n > 0).then(|| KpiChange {
            text: format!("{}{} pts", sign(gd), gd.abs()),
            tone: tone_of(gd),
        }),
        foot: if g_n > 0 {
            format!(
                "vs baseline \u{00b7} {g_n} optimization{}",
                if g_n == 1 { "" } else { "s" }
            )
        } else {
            "no verified wins in this range".into()
        },
        trend: if g_n > 0 {
            ticks
                .iter()
                .map(|t| median_gain(&moves, up_to(*t)).0)
                .collect()
        } else {
            Vec::new()
        },
    });

    // Spend
    let day_ago = now - DAY_MS;
    let today: f64 = spend
        .iter()
        .filter(|e| e.at >= day_ago)
        .map(|e| e.usd)
        .sum();
    kpis.push(Kpi {
        id: "spend".into(),
        label: "Spend".into(),
        value: usd(cur_spend),
        unit: None,
        change: if had_prev {
            pct_change(cur_spend, prev_spend, Wants::Neutral)
        } else {
            None
        },
        foot: format!("{} in the last 24h", usd(today)),
        trend: ticks.iter().map(|t| spend_to(*t)).collect(),
    });

    // Cost per win
    let cpw = (!cur_moves.is_empty()).then(|| cur_spend / cur_moves.len() as f64);
    let prev_cpw = (!prev_moves.is_empty()).then(|| prev_spend / prev_moves.len() as f64);
    kpis.push(Kpi {
        id: "cpw".into(),
        label: "Cost per win".into(),
        value: cpw.map_or("\u{2014}".into(), usd),
        unit: Some(if cpw.is_some() {
            "avg".into()
        } else {
            String::new()
        }),
        change: match (cpw, prev_cpw) {
            (Some(c), Some(p)) => pct_change(c, p, Wants::Lower),
            _ => None,
        },
        foot: if cpw.is_none() {
            "no verified wins in this range".into()
        } else {
            "all spend \u{00f7} verified wins".into()
        },
        trend: ticks
            .iter()
            .filter(|t| wins_to(**t) > 0)
            .map(|t| spend_to(*t) / wins_to(*t) as f64)
            .collect(),
    });

    // Value created (only with a value model somewhere)
    let valued: Vec<&KpiInput<'_>> = inputs
        .iter()
        .filter(|i| i.row.value_usd_per_unit.is_some())
        .collect();
    if !valued.is_empty() {
        let v_ids: std::collections::BTreeSet<&str> =
            valued.iter().map(|i| i.summary.id.as_str()).collect();
        let value_in = |w: Window| -> f64 {
            moves
                .iter()
                .filter(|m| w.has(m.at))
                .filter_map(|m| m.usd_per_year)
                .sum()
        };
        let v = value_in(cur);
        let v_spend: f64 = spend
            .iter()
            .filter(|e| v_ids.contains(e.opt.as_str()) && cur.has(e.at))
            .map(|e| e.usd)
            .sum();
        let estimated = valued
            .iter()
            .any(|i| i.row.value_basis != Some(ValueBasis::Measured));
        kpis.push(Kpi {
            id: "value".into(),
            label: "Value created".into(),
            value: usd(v),
            unit: None,
            change: if had_prev {
                pct_change(v, value_in(prev), Wants::Higher)
            } else {
                None
            },
            foot: format!(
                "per year{} \u{00b7} {}",
                if v_spend > 0.0 && v > 0.0 {
                    format!(" \u{00b7} {} ROI", roi(v / v_spend))
                } else {
                    String::new()
                },
                if estimated { "estimated" } else { "measured" }
            ),
            trend: ticks.iter().map(|t| value_in(up_to(*t))).collect(),
        });
    }
    kpis
}

fn range_label(r: KpiRange) -> &'static str {
    match r {
        KpiRange::Days7 => "7d",
        KpiRange::Days30 => "30d",
        KpiRange::Days90 => "90d",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_and_multiples() {
        assert_eq!(usd(4.2), "$4.2");
        assert_eq!(usd(42.0), "$42");
        assert_eq!(usd(1234.4), "$1,234");
        assert_eq!(usd(12_345.0), "$12.3k");
        assert_eq!(usd(1_500_000.0), "$1.5M");
        assert_eq!(roi(84.0), "84.0\u{00d7}");
        assert_eq!(roi(250.0), "250\u{00d7}");
        assert_eq!(roi(12_500.0), "12.5k\u{00d7}");
    }

    #[test]
    fn median_of_even_and_odd() {
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
    }

    #[test]
    fn changes() {
        assert_eq!(count_change(5, 3).text, "+2");
        assert_eq!(count_change(1, 3).tone, Tone::Bad);
        assert_eq!(count_change(3, 3).text, "\u{00b1}0");
        assert!(pct_change(1.0, 0.0, Wants::Higher).is_none());
        let c = pct_change(80.0, 100.0, Wants::Lower).unwrap();
        assert_eq!((c.text.as_str(), c.tone), ("\u{2212}20%", Tone::Good));
    }
}
