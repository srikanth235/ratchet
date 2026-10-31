//! The text the agent sees: the generated `.ratchet/context.md`, the first prompt and the per-turn resume message.

use crate::domain::{
    CandidateVerdict, DepsMode, Direction, Guardrail, INSTALL_FILES, ProtectedSource, RunnerConfig,
    effective_protected,
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub n: usize,
    pub sha: String,
    pub verdict: CandidateVerdict,
    pub reasons: Vec<String>,
    /// Ratchet best reported with this verdict.
    pub best: f64,
    pub at: i64,
}

pub struct ContextInput<'a> {
    pub config: &'a RunnerConfig,
    pub best_value: f64,
    pub history: &'a [HistoryEntry],
    pub now: i64,
    pub spend_usd: f64,
    pub turn: u32,
    /// Short sha of the commit the agent's tree is based on (the ratchet best, once known).
    pub best_sha: Option<&'a str>,
}

pub fn format_duration(ms: i64) -> String {
    if ms <= 0 {
        return "0m".to_string();
    }
    let total_min = ms / 60_000;
    let (h, m) = (total_min / 60, total_min % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// `Math.round` (halves toward +infinity).
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `Number.prototype.toFixed`: like `format!("{:.d}")`, except that an exact tie rounds away from zero instead of to even.
pub(crate) fn to_fixed(x: f64, digits: usize) -> String {
    let ax = x.abs();
    let long = format!("{:.*}", digits + 30, ax);
    let cut = long.find('.').map_or(long.len(), |dot| dot + 1 + digits);
    let rest = &long[cut.min(long.len())..];
    let tie = rest.starts_with('5') && rest[1..].bytes().all(|b| b == b'0');
    let magnitude = if tie {
        let scale = 10f64.powi(digits as i32);
        format!("{:.*}", digits, ((ax * scale).floor() + 1.0) / scale)
    } else {
        format!("{:.*}", digits, ax)
    };
    if x < 0.0 && magnitude.bytes().any(|b| b != b'0' && b != b'.') {
        format!("-{magnitude}")
    } else {
        magnitude
    }
}

/// JS `String(number)` for the values this module prints: integers without a fraction, others rounded to 3 decimals.
fn fmt(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() {
        format!("{}", n as i64)
    } else {
        format!("{}", js_round(n * 1000.0) / 1000.0)
    }
}

fn usd(n: f64) -> String {
    format!("${}", to_fixed(n, 2))
}

fn percent_1(x: f64) -> String {
    to_fixed(x, 1)
}

pub fn improvement_pct(config: &RunnerConfig, from: f64, to: f64) -> String {
    if from == 0.0 {
        return "n/a".to_string();
    }
    let gain = if config.config.objective.direction == Direction::Minimize {
        (from - to) / from
    } else {
        (to - from) / from
    };
    format!(
        "{}{}%",
        if gain >= 0.0 { "" } else { "-" },
        percent_1((gain * 100.0).abs())
    )
}

/// The generated `.ratchet/context.md`: everything the agent needs to know that RATCHET.md cannot say statically.
pub fn build_context(input: &ContextInput<'_>) -> String {
    let ContextInput {
        config, history, ..
    } = input;
    let o = &config.config.objective;
    let remaining_ms = config.deadline_at - input.now;
    let remaining_usd = (config.budget_usd - input.spend_usd).max(0.0);
    let mut lines: Vec<String> = Vec::new();
    lines.push("# Ratchet context (generated; refreshed before every turn)".to_string());
    lines.push(String::new());
    lines.push("## Objective".to_string());
    let min_gain = config.min_gain.or(o.min_gain);
    let gain_text = match min_gain {
        None => "the gain calibrated from the baseline noise".to_string(),
        Some(g) => format!("{}%", percent_1(g * 100.0)),
    };
    let direction = if o.direction == Direction::Minimize {
        "minimize"
    } else {
        "maximize"
    };
    lines.push(format!(
        "- Metric: `{}` ({direction}). A candidate must beat the current best by more than {gain_text} and more than the benchmark noise floor.",
        o.metric
    ));
    lines.push(format!("- Baseline: {}", fmt(config.baseline_value)));
    let best_sha = input
        .best_sha
        .filter(|s| !s.is_empty())
        .map(|s| format!(" (commit {s})"))
        .unwrap_or_default();
    lines.push(format!(
        "- Current ratchet best: {}{best_sha}; improvement so far {}",
        fmt(input.best_value),
        improvement_pct(config, config.baseline_value, input.best_value)
    ));
    lines.push(String::new());
    lines.push("## Limits".to_string());
    lines.push(format!(
        "- Time remaining: {}",
        format_duration(remaining_ms)
    ));
    lines.push(format!(
        "- Budget remaining: {} of {}",
        usd(remaining_usd),
        usd(config.budget_usd)
    ));
    lines.push(format!(
        "- Turn: {}; candidates submitted: {}",
        input.turn,
        history.len()
    ));
    lines.push(String::new());
    lines.push("## History of your candidates".to_string());
    if history.is_empty() {
        lines.push("None yet.".to_string());
    } else {
        lines.push("| # | commit | verdict | best after | notes |".to_string());
        lines.push("|---|---|---|---|---|".to_string());
        for h in *history {
            let sha: String = h.sha.chars().take(8).collect();
            let notes = h.reasons.join("; ").replace('|', "/");
            let notes = if notes.is_empty() {
                "-".to_string()
            } else {
                notes
            };
            lines.push(format!(
                "| {} | {sha} | {} | {} | {notes} |",
                h.n,
                h.verdict,
                fmt(h.best)
            ));
        }
    }
    lines.push(String::new());
    lines.push("## Rules".to_string());
    let guarded: Vec<String> = effective_protected(&config.config)
        .into_iter()
        .filter(|e| e.source != ProtectedSource::Install)
        .map(|e| format!("`{}`", e.glob))
        .collect();
    let install: Vec<String> = INSTALL_FILES.iter().map(|f| format!("`{f}`")).collect();
    lines.push(format!(
        "- Protected paths (a candidate touching them is blocked): {}, and lockfiles / install config ({}) at any depth",
        guarded.join(", "),
        install.join(", ")
    ));
    let guardrails: Vec<String> = config
        .config
        .guardrails
        .iter()
        .map(|g| match g {
            Guardrail::Command { run } => format!("`{run}` must pass"),
            Guardrail::Threshold { metric, raw, .. } => format!("`{metric}` {raw}"),
        })
        .collect();
    lines.push(format!(
        "- Guardrails checked on every candidate: {}",
        if guardrails.is_empty() {
            "none".to_string()
        } else {
            guardrails.join("; ")
        }
    ));
    if config.config.evaluator.deps != DepsMode::None {
        lines.push("- Dependencies are fixed: the trusted benchmark has no network and installs from the lockfile of the base ref. A candidate that changes the lockfile (pnpm-lock.yaml, package-lock.json) is blocked, so do not add or upgrade packages.".to_string());
    }
    lines.push("- The trusted benchmark runs on pinned hardware and a hidden trace; your own measurements only guide you.".to_string());
    lines.push(String::new());
    lines.push("## How to submit".to_string());
    lines.push("- Commit one idea per commit. The runner submits your newest commit (or commits a dirty tree for you) with the trailer `Ratchet-Candidate: true`; do not push yourself.".to_string());
    lines.push("- After each verdict you are resumed with the result. When another agent moves the ratchet best, the runner rebases your branch onto it (conflicts resolved in favour of the best).".to_string());
    lines.push("- To abandon rejected work and start from the last accepted tree: `git reset --hard refs/ratchet/accepted`.".to_string());
    format!("{}\n", lines.join("\n"))
}

pub fn build_initial_prompt(config: &RunnerConfig, context: &str) -> String {
    [
        config.brief.trim_end(),
        "",
        "---",
        "",
        "You are running under Ratchet. The generated context below is also written to `.ratchet/context.md` and refreshed before every turn; re-read it when you resume.",
        "Work in this repository, make one improvement, verify it with the tests and your own measurements, commit it, and end your turn. Ratchet will submit the commit to the trusted benchmark and resume you with the verdict.",
        "",
        context.trim_end(),
        "",
    ]
    .join("\n")
}

pub struct ResumeInput<'a> {
    pub entry: Option<&'a HistoryEntry>,
    pub best_value: f64,
    pub config: &'a RunnerConfig,
    pub now: i64,
    pub spend_usd: f64,
    pub notes: &'a [String],
    pub user_messages: &'a [String],
    /// Why there is no verdict (agent made no change).
    pub no_change: bool,
}

pub fn build_resume_message(input: &ResumeInput<'_>) -> String {
    let config = input.config;
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = input.entry {
        let sha: String = e.sha.chars().take(8).collect();
        parts.push(format!(
            "Verdict for candidate {} ({sha}): {}.",
            e.n,
            e.verdict.as_str().to_uppercase()
        ));
        if !e.reasons.is_empty() {
            parts.push(
                e.reasons
                    .iter()
                    .map(|r| format!("- {r}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        if e.verdict == CandidateVerdict::Verified {
            parts.push("The ratchet moved to your commit.".to_string());
        } else {
            parts.push("The ratchet did not move. Your commit is still in your tree; use `git reset --hard refs/ratchet/accepted` to drop it.".to_string());
        }
    } else if input.no_change {
        parts.push("No change to the repository was detected after your last turn, so nothing was submitted. Try a different idea, or finish if you are out of ideas.".to_string());
    }
    parts.push(format!(
        "Current ratchet best: {} {} (baseline {}).",
        fmt(input.best_value),
        config.config.objective.metric,
        fmt(config.baseline_value)
    ));
    parts.push(format!(
        "Time remaining: {}. Budget remaining: {}.",
        format_duration(config.deadline_at - input.now),
        usd((config.budget_usd - input.spend_usd).max(0.0))
    ));
    parts.extend(input.notes.iter().cloned());
    parts.extend(
        input
            .user_messages
            .iter()
            .map(|u| format!("Message from the user: {u}")),
    );
    parts.push(
        "`.ratchet/context.md` has been refreshed. Continue with your next idea.".to_string(),
    );
    parts.join("\n\n")
}
