//! The runtimes: the engine wired to mock, real or mixed ports. Port of `packages/runtime`.
//!
//! - [`simulation`]: mock cloud + local git + local bench + scripted agents (a whole optimization on one machine);
//! - [`live`]: real AWS / GitHub ports; [`live::create_hybrid_runtime`] is the product runtime (agents on this machine);
//! - [`summary`] / [`live_checks`]: the simulation summary and the Settings "test connection" probes.

pub mod clock;
pub mod live;
pub mod live_checks;
pub mod simulate;
pub mod simulation;
pub mod summary;

pub use clock::system_clock;
pub use live::{
    HybridOptions, LiveOptions, LiveRuntime, LocalAgentsConfig, create_hybrid_runtime,
    create_live_runtime, create_live_runtime_for, resolve_live_images,
};
pub use live_checks::{
    ConnectionResult, ConnectionTarget, ConnectionTester, ConnectionTesterDeps, HttpProbe,
    create_connection_tester,
};
pub use simulate::{SimulateOptions, SimulateResult, simulate};
pub use simulation::{
    SIMULATION_PROJECTS, SimOverlay, SimulationOptions, SimulationProject, SimulationRepo,
    SimulationRuntime, create_simulation_runtime, prepare_simulation_repo,
};
pub use summary::{
    SimulationSummary, SummaryInput, build_simulation_summary, pr_to_path, write_simulation_summary,
};

use crate::settings::EngineSettings;

/// `{ ...base, ...over }` of the TypeScript runtimes: every setting `over` sets wins (the nested sandbox / reaper / noise groups
/// are merged field by field).
pub(crate) fn overlay_settings(base: EngineSettings, over: EngineSettings) -> EngineSettings {
    let mut s = base;
    macro_rules! pick {
        ($($f:ident),*) => { $( if over.$f.is_some() { s.$f = over.$f.clone(); } )* };
    }
    pick!(
        agents_where,
        secrets,
        tick_interval_ms,
        scratch_dir,
        runner_image_ref,
        token_pricing,
        warn_fraction,
        max_bench_attempts,
        drain_timeout_ms,
        confirm_baseline,
        fallback_hourly_usd,
        report_builder,
        cost_lag_ms,
        on_error,
        new_id
    );
    s.secret_names.extend(over.secret_names.clone());
    macro_rules! pick_in {
        ($g:ident: $($f:ident),*) => { $( if over.$g.$f.is_some() { s.$g.$f = over.$g.$f.clone(); } )* };
    }
    pick_in!(sandbox: auto, idle_ms, default_region, retry_ms, sandbox_id, input);
    pick_in!(reaper: max_experiment_lifetime_ms, max_bench_lifetime_ms, runner_grace_ms);
    pick_in!(noise: k, min_absolute);
    s
}
