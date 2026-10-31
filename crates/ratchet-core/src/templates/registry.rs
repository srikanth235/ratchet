//! Evaluator template registry: metadata and file lists only.
//!
//! A template = a `ratchet.yaml` fragment + the evaluator's files + a `RATCHET.md` brief (see
//! [`render_template`](super::render_template)). Two kinds exist:
//!  - `template`: real, runnable evaluator files. `evaluator.template: <id>` in ratchet.yaml resolves to its command.
//!  - `demo`: "Try a demo" entries that only point at a ready-made project under `examples/`; they run in
//!    simulation with no setup, have no files of their own and cannot be used as `evaluator.template`.
//!
//! "Packs" are not a user-facing concept any more: they are the template `category`.
//!
//! Port of `packages/core/src/templates/registry.ts`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::files::template_files;
use crate::enums::{Direction, EvaluatorKind, TemplateCategory, TemplateKind};

/// The default objective of a template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TemplateObjective {
    /// Key the evaluator prints, as a dotted path.
    pub metric: String,
    pub direction: Direction,
    pub unit: String,
}

/// Public, UI-friendly description of one template. Stable shape: the app's gallery renders it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    /// Id used by `evaluator.template`, `ratchet init --template` and `build_template_proposal`.
    pub id: String,
    pub kind: TemplateKind,
    pub category: TemplateCategory,
    pub title: String,
    /// One sentence: what improving this means.
    pub description: String,
    /// What the evaluator reports, in plain words (e.g. "k6 p50/p95/p99, error rate").
    pub measures: String,
    /// The hidden workload that re-checks every win, or `None` when the template has none.
    pub holdout: Option<String>,
    /// Default bench machine (instance type).
    pub machine: String,
    pub evaluator_kind: EvaluatorKind,
    pub objective: TemplateObjective,
    /// Placeholder USD per unit of improvement per year, written to the generated config (replace it).
    pub default_value_usd: f64,
    /// Repo paths the template adds (empty for demos).
    pub files: Vec<String>,
    /// Demos only: the example project, relative to the repository root (e.g. `examples/demo-search`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

/// What `evaluator.template` resolves to inside the config.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatorTemplate {
    pub id: String,
    pub run: String,
    pub evaluator_kind: EvaluatorKind,
    pub machine: String,
    /// Default guardrails, in ratchet.yaml form (`(key, value)`: `("run", "...")` or `("error_rate", "< 0.001")`),
    /// used when `guardrails:` is absent.
    pub guardrails: Vec<(String, String)>,
    /// Globs the template's evaluator needs protected (its files and data).
    pub protect: Vec<String>,
}

/// A template definition with its private fields.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TemplateDef {
    pub id: &'static str,
    pub kind: TemplateKind,
    pub category: TemplateCategory,
    pub title: &'static str,
    pub description: &'static str,
    pub measures: &'static str,
    pub holdout: Option<&'static str>,
    pub machine: &'static str,
    pub evaluator_kind: EvaluatorKind,
    pub metric: &'static str,
    pub direction: Direction,
    pub unit: &'static str,
    pub default_value_usd: f64,
    pub example: Option<&'static str>,
    pub run: Option<&'static str>,
    pub guardrails: &'static [(&'static str, &'static str)],
    pub protect: &'static [&'static str],
    /// Command guardrail written to the generated ratchet.yaml as a TODO (the repo's own test command).
    pub test_command: bool,
}

#[allow(clippy::too_many_arguments)]
const fn template(
    id: &'static str,
    category: TemplateCategory,
    title: &'static str,
    description: &'static str,
    measures: &'static str,
    holdout: Option<&'static str>,
    machine: &'static str,
    evaluator_kind: EvaluatorKind,
    (metric, unit): (&'static str, &'static str),
    default_value_usd: f64,
    run: &'static str,
    guardrails: &'static [(&'static str, &'static str)],
    protect: &'static [&'static str],
    test_command: bool,
) -> TemplateDef {
    TemplateDef {
        id,
        kind: TemplateKind::Template,
        category,
        title,
        description,
        measures,
        holdout,
        machine,
        evaluator_kind,
        metric,
        direction: Direction::Minimize,
        unit,
        default_value_usd,
        example: None,
        run: Some(run),
        guardrails,
        protect,
        test_command,
    }
}

#[allow(clippy::too_many_arguments)]
const fn demo(
    id: &'static str,
    category: TemplateCategory,
    title: &'static str,
    description: &'static str,
    measures: &'static str,
    holdout: &'static str,
    machine: &'static str,
    evaluator_kind: EvaluatorKind,
    (metric, unit): (&'static str, &'static str),
    default_value_usd: f64,
    example: &'static str,
) -> TemplateDef {
    TemplateDef {
        id,
        kind: TemplateKind::Demo,
        category,
        title,
        description,
        measures,
        holdout: Some(holdout),
        machine,
        evaluator_kind,
        metric,
        direction: Direction::Minimize,
        unit,
        default_value_usd,
        example: Some(example),
        run: None,
        guardrails: &[],
        protect: &[],
        test_command: false,
    }
}

use EvaluatorKind as K;
use TemplateCategory as C;

static DEFS: [TemplateDef; 10] = [
    template(
        "http-latency",
        C::Perf,
        "HTTP latency",
        "make a web service answer faster under a fixed request mix",
        "p50 / p95 / p99 latency of an open-loop load test, error rate",
        Some("hidden request mix"),
        "c7i.2xlarge",
        K::Benchmark,
        ("http_req_duration.p95", "ms"),
        1000.0,
        "node bench/http-latency.mjs",
        &[("error_rate", "< 0.001")],
        &["bench/**"],
        true,
    ),
    template(
        "cli-cold-start",
        C::Perf,
        "CLI cold start",
        "make a command-line tool start and finish faster",
        "wall time of a set of invocations (hyperfine when installed), error rate",
        Some("hidden invocations"),
        "c7i.large",
        K::Benchmark,
        ("cold_start_ms.p50", "ms"),
        400.0,
        "node bench/cold-start.mjs",
        &[("error_rate", "< 0.001")],
        &["bench/**"],
        true,
    ),
    template(
        "test-suite-time",
        C::Perf,
        "Test suite time",
        "make the test suite run faster without losing tests",
        "duration of the test command, whether it passed",
        None,
        "c7i.2xlarge",
        K::Benchmark,
        ("test_suite_ms", "ms"),
        50.0,
        "node bench/test-time.mjs",
        &[("error_rate", "< 0.001")],
        &["bench/**"],
        false,
    ),
    template(
        "build-size",
        C::Perf,
        "Build time and bundle size",
        "make the build faster or the shipped bundle smaller",
        "build seconds, bundle KB (raw and gzip)",
        None,
        "c7i.xlarge",
        K::Benchmark,
        ("bundle_kb", "KB"),
        200.0,
        "node bench/build-size.mjs",
        &[("error_rate", "< 0.001")],
        &["bench/**"],
        true,
    ),
    template(
        "memory",
        C::Perf,
        "Peak memory",
        "make a workload use less memory",
        "peak resident set size (RSS) of the workload process tree",
        Some("hidden workload"),
        "c7i.xlarge",
        K::Benchmark,
        ("peak_rss_mb", "MB"),
        300.0,
        "node bench/memory.mjs",
        &[("error_rate", "< 0.001")],
        &["bench/**"],
        true,
    ),
    template(
        "forecast-backtest",
        C::Forecasting,
        "Forecast accuracy",
        "make a demand, traffic or revenue forecast more accurate",
        "WAPE / MAPE / bias of a rolling-origin backtest, look-ahead findings",
        Some("most recent periods"),
        "c7i.xlarge",
        K::Backtest,
        ("wape", "WAPE point"),
        10000.0,
        "node eval/backtest.mjs",
        &[("leakage_violations", "== 0")],
        &["eval/**", "data/**"],
        true,
    ),
    // llm-eval maximizes its score
    {
        let mut t = template(
            "llm-eval",
            C::Llm,
            "LLM eval score",
            "make an LLM-backed task score higher, or cost less per call",
            "score on an eval set, cost and tokens per call, error rate",
            Some("hidden eval set"),
            "c7i.large",
            K::Eval,
            ("score", "score point"),
            5000.0,
            "node eval/llm-eval.mjs",
            &[("error_rate", "< 0.05")],
            &["eval/**"],
            true,
        );
        t.direction = Direction::Maximize;
        t
    },
    demo(
        "demo-search",
        C::Perf,
        "Demo: search API latency",
        "a search service whose p95 latency agents bring down (runs in simulation, no setup)",
        "k6 p50 / p95 / p99, error rate",
        "hidden query mix",
        "c7i.2xlarge",
        K::Benchmark,
        ("http_req_duration.p95", "ms"),
        1200.0,
        "examples/demo-search",
    ),
    demo(
        "demo-cli",
        C::Perf,
        "Demo: CLI cold start",
        "a task-manager CLI whose cold start agents shave (runs in simulation, no setup)",
        "hyperfine wall time per invocation",
        "hidden invocations",
        "c7i.large",
        K::Benchmark,
        ("cold_start_ms.p50", "ms"),
        400.0,
        "examples/demo-cli",
    ),
    demo(
        "demo-forecast",
        C::Forecasting,
        "Demo: SKU demand forecast",
        "a weekly demand forecaster whose WAPE agents lower (runs in simulation, no setup)",
        "WAPE / MAPE / bias of a rolling-origin backtest",
        "the 12 weeks after the training data",
        "c7i.large",
        K::Backtest,
        ("wape", "WAPE point"),
        20000.0,
        "examples/demo-forecast",
    ),
];

/// Legacy `ratchet init --pack` names, mapped to the template that replaced them.
pub const TEMPLATE_ALIASES: [(&str, &str); 2] = [
    ("perf", "cli-cold-start"),
    ("forecasting", "forecast-backtest"),
];

/// Resolve a template id or a legacy pack alias to a canonical template id (unchanged when unknown).
pub fn resolve_template_id(id_or_alias: &str) -> &str {
    TEMPLATE_ALIASES
        .iter()
        .find(|(a, _)| *a == id_or_alias)
        .map(|(_, id)| *id)
        .unwrap_or(id_or_alias)
}

pub(crate) fn find_def(id: &str) -> Option<&'static TemplateDef> {
    DEFS.iter().find(|d| d.id == id)
}

fn info_of(d: &TemplateDef) -> TemplateInfo {
    let files = if d.kind == TemplateKind::Template {
        std::iter::once("ratchet.yaml".to_owned())
            .chain(template_files(d.id).into_iter().map(|(p, _)| p))
            .collect()
    } else {
        Vec::new()
    };
    TemplateInfo {
        id: d.id.to_owned(),
        kind: d.kind,
        category: d.category,
        title: d.title.to_owned(),
        description: d.description.to_owned(),
        measures: d.measures.to_owned(),
        holdout: d.holdout.map(str::to_owned),
        machine: d.machine.to_owned(),
        evaluator_kind: d.evaluator_kind,
        objective: TemplateObjective {
            metric: d.metric.to_owned(),
            direction: d.direction,
            unit: d.unit.to_owned(),
        },
        default_value_usd: d.default_value_usd,
        files,
        example: d.example.map(str::to_owned),
    }
}

/// Filter of [`list_templates`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateFilter {
    pub kind: Option<TemplateKind>,
    pub category: Option<TemplateCategory>,
}

/// Every template for the gallery: the runnable ones first (in table order), then the "Try a demo" entries.
/// Pure data; every call returns fresh copies.
pub fn list_templates(filter: TemplateFilter) -> Vec<TemplateInfo> {
    DEFS.iter()
        .filter(|d| {
            filter.kind.is_none_or(|k| d.kind == k)
                && filter.category.is_none_or(|c| d.category == c)
        })
        .map(info_of)
        .collect()
}

/// One template by id (legacy pack aliases accepted).
pub fn get_template(id_or_alias: &str) -> Option<TemplateInfo> {
    find_def(resolve_template_id(id_or_alias)).map(info_of)
}

/// Ids usable as `evaluator.template` (the runnable templates, not the demos).
pub fn evaluator_template_ids() -> Vec<String> {
    DEFS.iter()
        .filter(|d| d.kind == TemplateKind::Template)
        .map(|d| d.id.to_owned())
        .collect()
}

/// Resolve `evaluator.template`. Demos and unknown ids give `None`.
pub fn find_evaluator_template(id: &str) -> Option<EvaluatorTemplate> {
    let d = DEFS
        .iter()
        .find(|d| d.id == id && d.kind == TemplateKind::Template)?;
    Some(EvaluatorTemplate {
        id: d.id.to_owned(),
        run: d.run?.to_owned(),
        evaluator_kind: d.evaluator_kind,
        machine: d.machine.to_owned(),
        guardrails: d
            .guardrails
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        protect: d.protect.iter().map(|s| (*s).to_owned()).collect(),
    })
}
