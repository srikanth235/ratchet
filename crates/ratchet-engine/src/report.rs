//! The fallback pull request body. Port of `packages/engine/src/report.ts`.

use std::collections::BTreeMap;

use ratchet_core::enums::{Direction, SpendCategory};
use ratchet_core::ids::OptimizationId;
use ratchet_core::numfmt::{precision_trimmed, to_fixed};
use ratchet_ledger::Ledger;

use crate::errors::{EngineError, EngineResult};

fn fmt(n: f64) -> String {
    precision_trimmed(n, 5)
}

/// Fallback PR body when no `ReportBuilder` is configured. A richer report builder replaces it.
pub async fn default_report(ledger: &Ledger, optimization_id: &OptimizationId) -> EngineResult<String> {
    let opt = ledger
        .get_optimization(optimization_id)
        .await?
        .ok_or_else(|| EngineError::not_found(format!("optimization {optimization_id} not found")))?;
    let baseline = ledger.get_baseline(optimization_id).await?;
    let best = ledger.current_best(optimization_id).await?;
    let history = ledger.ratchet_history(optimization_id).await?;
    let spend = ledger.spend_breakdown(optimization_id).await?;
    let cands = ledger.candidates_with_results(optimization_id).await?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in &cands {
        *counts.entry(c.candidate.verdict.to_string()).or_default() += 1;
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("## Ratchet: {} ({})", opt.metric, opt.direction));
    lines.push(String::new());
    if let (Some(baseline), Some(best)) = (&baseline, best) {
        let delta = match opt.direction {
            Direction::Minimize => baseline.value - best,
            Direction::Maximize => best - baseline.value,
        };
        let pct = if baseline.value == 0.0 {
            0.0
        } else {
            delta / baseline.value.abs() * 100.0
        };
        lines.push(format!(
            "**{} -> {}** ({}{}% {}), noise floor {}, {} verified win{}.",
            fmt(baseline.value),
            fmt(best),
            if pct >= 0.0 { "" } else { "-" },
            fmt(pct.abs()),
            if pct >= 0.0 { "better" } else { "worse" },
            fmt(baseline.noise_floor),
            history.len(),
            if history.len() == 1 { "" } else { "s" },
        ));
        lines.push(String::new());
    }
    lines.push("### Verified wins (ratchet order)".into());
    lines.push(String::new());
    if history.is_empty() {
        lines.push("None.".into());
    } else {
        lines.push("| # | Agent | Commit | From | To | Gain |".into());
        lines.push("|---|---|---|---|---|---|".into());
        for (i, m) in history.iter().enumerate() {
            let msg = cands
                .iter()
                .find(|c| c.candidate.id == m.candidate_id)
                .and_then(|c| c.candidate.message.split('\n').next())
                .unwrap_or("");
            let sha: String = m.sha.chars().take(10).collect();
            lines.push(format!(
                "| {} | {} | `{}` {} | {} | {} | {}% |",
                i + 1,
                m.agent_kind,
                sha,
                if msg.is_empty() {
                    String::new()
                } else {
                    format!("- {}", msg.replace('|', "/"))
                },
                fmt(m.from_value),
                fmt(m.to_value),
                fmt(m.gain_pct),
            ));
        }
    }
    lines.push(String::new());
    lines.push("### All candidates".into());
    lines.push(String::new());
    // `[...counts.entries()].sort()` sorts the `verdict,count` strings
    let mut entries: Vec<String> = counts.iter().map(|(v, n)| format!("{v},{n}")).collect();
    entries.sort();
    let summary = entries
        .iter()
        .map(|e| {
            let (v, n) = e.split_once(',').unwrap_or((e, "0"));
            format!("{n} {v}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(if summary.is_empty() { "none".into() } else { summary });
    lines.push(String::new());
    lines.push("### Spend".into());
    lines.push(String::new());
    lines.push(format!(
        "Total ${} of ${} budget (estimated ${}).",
        to_fixed(spend.total, 2),
        to_fixed(opt.budget_usd, 2),
        to_fixed(spend.estimated, 2)
    ));
    for cat in SpendCategory::ALL {
        let usd = spend.by_category.get(*cat);
        if usd > 0.0 {
            lines.push(format!("- {cat}: ${}", to_fixed(usd, 2)));
        }
    }
    if !history.is_empty() {
        lines.push(format!(
            "- per verified win: ${}",
            to_fixed(spend.total / history.len() as f64, 2)
        ));
    }
    lines.push(String::new());
    lines.push("_Every commit above was re-benchmarked on trusted hardware and confirmed on a hidden trace before it moved the ratchet._".into());
    Ok(lines.join("\n"))
}
