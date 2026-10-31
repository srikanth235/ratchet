//! `simulate --summary <file>` output and the end-of-run summary. Port of `packages/runtime/src/summary.ts`.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use ratchet_core::enums::CandidateVerdict;
use ratchet_core::rows::{OptimizationSummary, SpendBreakdown};
use ratchet_core::value::ValueEstimate;
use serde::{Deserialize, Serialize};

/// What `simulate --summary <file>` writes, and what the CLI prints at the end of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationSummary {
    pub project: String,
    pub pack: Option<String>,
    pub optimization_id: String,
    pub objective: String,
    pub metric: String,
    pub status: String,
    pub baseline: Option<f64>,
    pub best: Option<f64>,
    /// Improvement vs baseline in percent (positive = better in the metric's direction).
    pub gain_pct: Option<f64>,
    /// Verified ratchet moves.
    pub wins: i64,
    pub candidates: i64,
    /// Candidates per final verdict; every verdict is present, 0 when none.
    pub verdicts: IndexMap<String, u64>,
    pub spend_usd: f64,
    pub spend: SummarySpend,
    /// Dollar value of the win (`objective.value` in ratchet.yaml); `null` when the config has no value model.
    pub value: Option<SummaryValue>,
    /// The pull request as the git host returned it (a `file://` URL in simulation).
    pub pr: Option<String>,
    /// The same as a filesystem path when the host is local, else the URL.
    pub pr_path: Option<String>,
    pub work_root: String,
    /// The ledger SQLite file of the run.
    pub db: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarySpend {
    pub estimated_usd: f64,
    pub actual_usd: f64,
    pub by_category: IndexMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryValue {
    pub improvement: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub usd_per_year: f64,
    pub basis: String,
    pub return_on_spend: Option<f64>,
}

pub struct SummaryInput<'a> {
    pub project: String,
    pub pack: Option<String>,
    /// `objective.name ?? objective.metric`.
    pub objective: String,
    pub summary: &'a OptimizationSummary,
    pub candidates: Vec<CandidateVerdict>,
    pub spend: &'a SpendBreakdown,
    pub value: Option<ValueEstimate>,
    pub roi: Option<f64>,
    pub pr: Option<String>,
    pub work_root: String,
    pub db: String,
}

/// `file:///x/y` becomes `/x/y`; anything else is returned as is.
pub fn pr_to_path(pr: &str) -> String {
    if !pr.starts_with("file:") {
        return pr.to_owned();
    }
    url::Url::parse(pr)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .map_or_else(|| pr.to_owned(), |p| p.to_string_lossy().into_owned())
}

pub fn build_simulation_summary(i: SummaryInput<'_>) -> SimulationSummary {
    let mut verdicts: IndexMap<String, u64> = CandidateVerdict::ALL
        .iter()
        .map(|v| (v.as_str().to_owned(), 0))
        .collect();
    for v in &i.candidates {
        *verdicts.entry(v.as_str().to_owned()).or_insert(0) += 1;
    }
    let by_category = ratchet_core::enums::SpendCategory::ALL
        .iter()
        .map(|c| (c.as_str().to_owned(), i.spend.by_category.get(*c)))
        .collect();
    SimulationSummary {
        project: i.project,
        pack: i.pack,
        optimization_id: i.summary.id.to_string(),
        objective: i.objective,
        metric: i.summary.metric.clone(),
        status: i.summary.status.as_str().to_owned(),
        baseline: i.summary.baseline_value,
        best: i.summary.best_value,
        gain_pct: i.summary.gain_pct,
        wins: i.summary.verified_wins,
        candidates: i.summary.candidate_count,
        verdicts,
        spend_usd: i.summary.spend_usd,
        spend: SummarySpend {
            estimated_usd: i.spend.estimated,
            actual_usd: i.spend.actual,
            by_category,
        },
        value: i.value.map(|v| SummaryValue {
            improvement: v.improvement,
            unit: v.unit.filter(|u| !u.is_empty()),
            usd_per_year: v.usd_per_year,
            basis: v.basis.as_str().to_owned(),
            return_on_spend: i.roi,
        }),
        pr_path: i.pr.as_deref().map(pr_to_path),
        pr: i.pr,
        work_root: i.work_root,
        db: i.db,
    }
}

/// Write the summary as pretty JSON (creating parent directories; atomic rename so readers never see half a file).
/// Returns the absolute path.
pub fn write_simulation_summary(
    path: impl AsRef<Path>,
    summary: &SimulationSummary,
) -> std::io::Result<PathBuf> {
    let file = std::path::absolute(path.as_ref())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = file.clone().into_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let mut text = serde_json::to_string_pretty(summary).map_err(std::io::Error::other)?;
    text.push('\n');
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &file)?;
    Ok(file)
}
