//! Ratchet's bench: the pipeline that decides whether a candidate really is better.
//!
//! Port of `packages/bench`. A candidate is evaluated by the SAME steps everywhere
//! (`checkout -> setup -> command guardrails -> bench x N -> threshold guardrails`, see [`pipeline`]); only the executor
//! differs:
//!
//!  - [`local::LocalBenchRunner`]: child processes on this machine (simulation mode, `ratchet bench`). NOT a security
//!    boundary.
//!  - [`remote::RemoteBenchRunner`]: a fresh, offline bench instance per job, driven by generated shell scripts through
//!    `CloudCompute::run_command` ([`remote_scripts`]); dependency snapshots ([`deps`], [`remote_deps`]) and the repo's
//!    container image ([`image`], [`image_scripts`]) arrive from the artifacts bucket and are restored offline.
//!
//! Around them: [`queue::BenchQueue`] (fair, one job at a time per optimization), [`evaluate`] (compose the bench outcomes
//! with the ratchet rule), [`overlay`] (protected files always come from `bench.ref`), [`stage`] (pre-stage the trees in the
//! artifacts bucket) and the setuid candidate launcher (`src/bin/ratchet-candidate-launcher.rs`, [`launcher`]).
//!
//! `PARITY.md` maps every TypeScript test to its Rust counterpart.

pub mod deps;
pub mod deps_scripts;
pub mod errors;
pub mod evaluate;
pub mod exec;
pub mod flatten;
pub mod image;
pub mod image_scripts;
pub mod launcher;
pub mod local;
pub mod log;
pub mod node_path;
pub mod overlay;
pub mod pipeline;
pub mod posix;
pub mod queue;
pub mod remote;
pub mod remote_deps;
pub mod remote_scripts;
pub mod remote_session;
pub mod shell;
pub mod stage;

#[cfg(feature = "testkit")]
pub mod testkit;

pub use deps::{
    Compression, DEPS_BASE_ARCHIVE, DEPS_PREFIX, DEPS_USER, DepsKeyInput, DepsManifest, DepsPlan, DepsStrategy,
    DepsStrategyId, FileHashes, NO_DEPS, NPM, PNPM, ResolvedDeps, arch_from_instance_type,
    dependency_change_guardrail, deps_key, hash_named_files, lockfile_changes, manifest_key,
    offline_setup_hint, parse_deps_manifest, pinned_deps_files, resolve_deps_strategy, root_entries,
};
pub use deps_scripts::{DepsBuildInput, deps_build_script};
pub use errors::{BenchError, BenchResult};
pub use evaluate::{
    BenchResultRecord, CandidateEvaluation, EvaluateCandidateInput, bench_result_record, bench_stats,
    evaluate_candidate, metrics_from_stats,
};
pub use exec::{ProcessOptions, ProcessResult, run_process};
pub use flatten::{flatten_numeric, median_metrics, resolve_metric};
pub use image::{
    EnvironmentSettings, IMAGE_PREFIX, ImageKeyInput, ImageManifest, ImagePlan, ImageSpec, ResolvedImage,
    container_setup_hint, environment_change_guardrail, environment_changes, environment_files_of,
    image_key, image_manifest_key, image_tag, normalize_repo_relative, parse_dockerfile,
    parse_image_manifest, parse_jsonc, read_environment, resolve_image_source,
};
pub use image_scripts::{
    CANDIDATE_HELPER, ContainerOptions, ContainerRunSpec, ContainerRuntime, ImageBuildInput, ImageRestore,
    container_run, image_build_script, image_restore_fragment,
};
pub use local::{LocalBenchRunner, LocalBenchRunnerOptions};
pub use log::BenchLog;
pub use node_path::{
    NodeRuntimeInfo, current_node_runtime, ensure_node_shim, node_dir_for, node_shim_script,
    resolve_node_on_path, with_node_on_path,
};
pub use overlay::{OverlayResult, apply_overlay, is_protected, list_files, restore_protected};
pub use pipeline::{
    BenchProgress, CheckoutVeto, DEFAULT_TIMEOUTS, PipelineExecutor, ProgressBenchRunner, StepOutcome,
    StepSpec, StepTimeouts, command_guardrail_result, evaluate_thresholds, log_key, parse_bench_output,
};
pub use queue::{BenchQueue, BenchRequest, BenchTicket, QueueEvent, state_label};
pub use remote::{RemoteBenchRunner, RemoteBenchRunnerOptions};
pub use remote_deps::{DepsSnapshots, PreparedEnvironment};
pub use remote_scripts::{
    BENCH_USER, CANDIDATE_LAUNCHER, DepsRestore, HOLDOUT_USER, Isolation, StagedArchive, protected_pcre,
};
pub use stage::{STAGING_PREFIX, StageInput, StagedSource, stage_deps_base, stage_source};
