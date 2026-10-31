//! Engine settings and their defaults. Port of `packages/engine/src/settings.ts`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use ratchet_core::NoiseFloorOptions;
use ratchet_core::config::RatchetConfig;
use ratchet_core::enums::{AgentKind, AgentsWhere};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::SandboxInput;
use ratchet_ledger::Ledger;

/// Where API keys come from (desktop: the OS keychain). Values go to `put_secret` and nowhere else.
pub type SecretSource = Arc<dyn Fn(AgentKind, &str) -> Option<String> + Send + Sync>;

/// Builds the pull request body (`defaultReport` when none is configured).
#[async_trait]
pub trait ReportBuilder: Send + Sync {
    async fn build(
        &self,
        ledger: &Ledger,
        optimization_id: &OptimizationId,
    ) -> Result<String, String>;
}

/// What a custom sandbox-input hook is given.
pub struct SandboxInputContext<'a> {
    pub sandbox_id: &'a str,
    pub region: &'a str,
    pub config: &'a RatchetConfig,
}

/// Called for non-fatal errors the engine swallowed to keep ticking: `(message, context)`.
pub type OnError = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The invisible sandbox. Off by default (the sandbox is then applied and destroyed explicitly, as in the tests and for
/// advanced use); the live and hybrid runtimes turn it on.
#[derive(Clone, Default)]
pub struct SandboxSettings {
    /// Create the sandbox lazily at the first run start and destroy it after the idle window. Default false.
    pub auto: Option<bool>,
    /// How long after the last run ends the sandbox is destroyed (default 30 min).
    pub idle_ms: Option<i64>,
    /// Region when ratchet.yaml names none (`environment.region`). Default `us-east-1`.
    pub default_region: Option<String>,
    /// After a failed automatic destroy, wait this long before trying again (default 5 min).
    pub retry_ms: Option<i64>,
    /// Sandbox id for a region. Default `ratchet-sandbox` (`ratchet-sim` in simulation) in the default region,
    /// `<id>-<region>` elsewhere.
    pub sandbox_id: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
    /// Override the plan inputs (instance types, limits) derived from ratchet.yaml.
    pub input: Option<Arc<dyn Fn(&SandboxInputContext<'_>) -> Option<SandboxInput> + Send + Sync>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenPricing {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

pub const DEFAULT_TOKEN_PRICING: TokenPricing = TokenPricing {
    input_usd_per_mtok: 3.0,
    output_usd_per_mtok: 15.0,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ReaperSettings {
    /// default 2 h (IAM boundary default)
    pub max_experiment_lifetime_ms: Option<i64>,
    /// default 2 h
    pub max_bench_lifetime_ms: Option<i64>,
    /// runner lifetime = time limit + grace (default 30 min)
    pub runner_grace_ms: Option<i64>,
}

#[derive(Clone, Default)]
pub struct EngineSettings {
    pub sandbox: SandboxSettings,
    /// Where agents run when an optimization does not say. Default: `local` if the engine was given a `local_runners`
    /// port, else `cloud`.
    pub agents_where: Option<AgentsWhere>,
    pub secrets: Option<SecretSource>,
    /// Secret names pushed to each agent's runner. Defaults below.
    pub secret_names: BTreeMap<AgentKind, Vec<String>>,
    /// `run()` tick period = metering interval = max budget overshoot (default 15 s).
    pub tick_interval_ms: Option<u64>,
    /// Where ratchet.yaml / RATCHET.md are checked out to read them (default: the OS tmp dir).
    pub scratch_dir: Option<std::path::PathBuf>,
    pub runner_image_ref: Option<String>,
    /// Used when a `usage` event carries tokens but no `cost_usd`.
    pub token_pricing: Option<TokenPricing>,
    /// Fraction of the budget at which a warning is emitted (default 0.8).
    pub warn_fraction: Option<f64>,
    pub noise: NoiseFloorOptions,
    /// Attempts per bench job before the candidate is rejected as an infrastructure failure (default 2).
    pub max_bench_attempts: Option<u32>,
    /// After a graceful stop, how long queued candidates may still be evaluated (default 30 min).
    pub drain_timeout_ms: Option<i64>,
    /// Also bench the baseline against the hidden trace so `confirm` compares like with like (default true).
    pub confirm_baseline: Option<bool>,
    /// Price used when the PriceBook fails and no earlier price is known (USD/h, default 10: conservative).
    pub fallback_hourly_usd: Option<f64>,
    pub reaper: ReaperSettings,
    pub report_builder: Option<Arc<dyn ReportBuilder>>,
    /// How long after an optimization ends billing data is assumed incomplete before `reconcile_costs` writes
    /// corrections (default 24 h, Cost Explorer's lag).
    pub cost_lag_ms: Option<i64>,
    pub on_error: Option<OnError>,
    /// Id source for optimizations (default `opt-<8 hex>`). Must produce `[a-z0-9-]` ids.
    pub new_id: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

pub fn default_secret_names(kind: AgentKind) -> Vec<String> {
    match kind {
        AgentKind::ClaudeCode => vec!["ANTHROPIC_API_KEY".into(), "GITHUB_TOKEN".into()],
        AgentKind::Codex => vec!["OPENAI_API_KEY".into(), "GITHUB_TOKEN".into()],
        AgentKind::Mock => vec![],
    }
}

#[derive(Clone)]
pub struct ResolvedSandboxSettings {
    pub auto: bool,
    pub idle_ms: i64,
    pub default_region: String,
    pub retry_ms: i64,
    pub sandbox_id: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
    pub input: Option<Arc<dyn Fn(&SandboxInputContext<'_>) -> Option<SandboxInput> + Send + Sync>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedReaper {
    pub max_experiment_lifetime_ms: i64,
    pub max_bench_lifetime_ms: i64,
    pub runner_grace_ms: i64,
}

#[derive(Clone)]
pub struct ResolvedSettings {
    pub sandbox: ResolvedSandboxSettings,
    pub agents_where: Option<AgentsWhere>,
    pub secrets: SecretSource,
    pub secret_names: BTreeMap<AgentKind, Vec<String>>,
    pub tick_interval_ms: u64,
    pub scratch_dir: Option<std::path::PathBuf>,
    pub runner_image_ref: String,
    pub token_pricing: TokenPricing,
    pub warn_fraction: f64,
    pub noise: NoiseFloorOptions,
    pub max_bench_attempts: u32,
    pub drain_timeout_ms: i64,
    pub confirm_baseline: bool,
    pub fallback_hourly_usd: f64,
    pub reaper: ResolvedReaper,
    pub report_builder: Option<Arc<dyn ReportBuilder>>,
    pub cost_lag_ms: i64,
    pub on_error: OnError,
    pub new_id: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

const HOUR: i64 = 3_600_000;

pub fn resolve_settings(s: EngineSettings) -> ResolvedSettings {
    let mut secret_names: BTreeMap<AgentKind, Vec<String>> = AgentKind::ALL
        .iter()
        .map(|k| (*k, default_secret_names(*k)))
        .collect();
    secret_names.extend(s.secret_names);
    ResolvedSettings {
        sandbox: ResolvedSandboxSettings {
            auto: s.sandbox.auto.unwrap_or(false),
            idle_ms: s.sandbox.idle_ms.unwrap_or(30 * 60_000),
            default_region: s
                .sandbox
                .default_region
                .unwrap_or_else(|| "us-east-1".into()),
            retry_ms: s.sandbox.retry_ms.unwrap_or(5 * 60_000),
            sandbox_id: s.sandbox.sandbox_id,
            input: s.sandbox.input,
        },
        agents_where: s.agents_where,
        secrets: s.secrets.unwrap_or_else(|| Arc::new(|_, _| None)),
        secret_names,
        tick_interval_ms: s.tick_interval_ms.unwrap_or(15_000),
        scratch_dir: s.scratch_dir,
        runner_image_ref: s
            .runner_image_ref
            .unwrap_or_else(|| "ratchet-runner".into()),
        token_pricing: s.token_pricing.unwrap_or(DEFAULT_TOKEN_PRICING),
        warn_fraction: s.warn_fraction.unwrap_or(0.8),
        noise: s.noise,
        max_bench_attempts: s.max_bench_attempts.unwrap_or(2),
        drain_timeout_ms: s.drain_timeout_ms.unwrap_or(30 * 60_000),
        confirm_baseline: s.confirm_baseline.unwrap_or(true),
        fallback_hourly_usd: s.fallback_hourly_usd.unwrap_or(10.0),
        reaper: ResolvedReaper {
            max_experiment_lifetime_ms: s.reaper.max_experiment_lifetime_ms.unwrap_or(2 * HOUR),
            max_bench_lifetime_ms: s.reaper.max_bench_lifetime_ms.unwrap_or(2 * HOUR),
            runner_grace_ms: s.reaper.runner_grace_ms.unwrap_or(30 * 60_000),
        },
        report_builder: s.report_builder,
        cost_lag_ms: s.cost_lag_ms.unwrap_or(24 * HOUR),
        on_error: s.on_error.unwrap_or_else(|| Arc::new(|_, _| {})),
        new_id: s.new_id,
    }
}
