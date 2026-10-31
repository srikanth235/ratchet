//! Simulation runtime: the real engine wired to local / mock ports, so a whole optimization runs on one machine against
//! `examples/demo-*`. Port of `packages/runtime/src/simulation.ts`.
//!
//! - AWS: `MockSandboxProvider` + `MockCloudCompute` (in-memory instances, real cost math)
//! - Git: `LocalGitHost` on a bare copy of the demo repo ("GitHub")
//! - Bench: `LocalBenchRunner` (real build, real tests, real evaluator)
//! - Agents: `LocalRunnerControl` running the real runner loop with `ScriptedAgent` (real patches from `ratchet-agents/scripts/<demo>`)
//!
//! Pack-aware: pick a demo with `project` (`search` and `cli` = perf pack, `forecast` = forecasting pack). A demo adapts its
//! ratchet.yaml for local runs with an optional `ratchet.sim.yaml` overlay (see [`SimOverlay`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use ratchet_agents::demo_scripts::ScriptError;
use ratchet_agents::{
    AgentScript, DemoVariant, LocalRunnerControl, LocalRunnerControlOptions, ScriptedAgent,
    ScriptedAgentOptions, load_demo_cli_script, load_demo_forecast_script, load_demo_search_script,
};
use ratchet_bench::{LocalBenchRunner, LocalBenchRunnerOptions, with_node_on_path};
use ratchet_core::config::{RatchetConfig, parse_config};
use ratchet_core::enums::{AgentKind, Mode, SandboxProviderKind, SandboxStatus, SpendCategory};
use ratchet_core::ports::{
    AgentAdapter, Clock, CostActuals, CostActualsQuery, CostReconciler, PortError, PortResult,
    RunnerControl, SandboxOutputs,
};
use ratchet_github::{
    CreateBareRemoteOptions, LocalGitHost, LocalGitHostOptions, create_bare_remote,
};
use ratchet_ledger::Ledger;
use ratchet_sandbox::sandbox::MockSandboxProviderOptions;
use ratchet_sandbox::{
    FsArtifactStore, MockCloudCompute, MockCloudComputeOptions, MockSandboxProvider,
    StaticPriceBook, billed,
};
use serde::Deserialize;
use serde_norway::{Mapping, Value};

use super::overlay_settings;
use crate::engine::{Engine, EngineOptions};
use crate::errors::{EngineError, EngineResult};
use crate::ports::{EnginePorts, GitSource};
use crate::settings::EngineSettings;

/// The shipped demo projects the simulator can optimize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum SimulationProject {
    #[default]
    Search,
    Forecast,
    Cli,
}

pub const SIMULATION_PROJECTS: [SimulationProject; 3] = [
    SimulationProject::Search,
    SimulationProject::Forecast,
    SimulationProject::Cli,
];

impl SimulationProject {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Forecast => "forecast",
            Self::Cli => "cli",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        SIMULATION_PROJECTS.into_iter().find(|p| p.as_str() == s)
    }

    /// The demo's source directory, relative to the monorepo root.
    pub fn dir(self) -> &'static str {
        match self {
            Self::Search => "examples/demo-search",
            Self::Forecast => "examples/demo-forecast",
            Self::Cli => "examples/demo-cli",
        }
    }

    pub fn pack(self) -> &'static str {
        match self {
            Self::Search | Self::Cli => "perf",
            Self::Forecast => "forecasting",
        }
    }

    /// The demo's shipped agent script for one variant (`cc` = Claude Code, `cx` = Codex).
    pub fn scripts(self, variant: DemoVariant) -> Result<AgentScript, ScriptError> {
        match self {
            Self::Search => load_demo_search_script(variant, None),
            Self::Forecast => load_demo_forecast_script(variant, None),
            Self::Cli => load_demo_cli_script(variant, None),
        }
    }
}

#[derive(Clone)]
pub struct SimulationOptions {
    pub ledger: Ledger,
    /// Scratch root. Must live inside the monorepo so bench checkouts resolve the workspace's node_modules.
    pub work_root: PathBuf,
    /// The monorepo root (for node_modules/.bin and the demo source).
    pub repo_root: PathBuf,
    /// Which shipped demo to optimize. Default `search`. Ignored for the source dir when `project_dir` is given.
    pub project: SimulationProject,
    /// Source project to optimize. Default: the `project`'s directory under `repo_root`.
    pub project_dir: Option<PathBuf>,
    /// Load-generator duration per bench run, seconds (the overlay's `{duration}`). Default 5.
    pub bench_duration_sec: Option<u32>,
    /// Evaluator repeats (overrides the overlay and ratchet.yaml). Default: the overlay's, else ratchet.yaml's, else 3.
    pub repeats: Option<u32>,
    /// Agent scripts per kind. Default: the project's `cc` for claude-code (and mock), `cx` for codex.
    pub scripts: BTreeMap<AgentKind, AgentScript>,
    pub clock: Option<Arc<dyn Clock>>,
    pub settings: EngineSettings,
    /// TypeScript compiler script the bench setup runs (`node <tsc> -p tsconfig.build.json`). Default
    /// `<repo_root>/node_modules/typescript/bin/tsc`.
    pub tsc: Option<PathBuf>,
    /// Directory put first on PATH for bench steps. Default `<repo_root>/node_modules/.bin`.
    pub bin_dir: Option<PathBuf>,
}

impl SimulationOptions {
    pub fn new(
        ledger: Ledger,
        work_root: impl Into<PathBuf>,
        repo_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            ledger,
            work_root: work_root.into(),
            repo_root: repo_root.into(),
            project: SimulationProject::Search,
            project_dir: None,
            bench_duration_sec: None,
            repeats: None,
            scripts: BTreeMap::new(),
            clock: None,
            settings: EngineSettings::default(),
            tsc: None,
            bin_dir: None,
        }
    }
}

pub struct SimulationRuntime {
    pub engine: Engine,
    pub sandbox: Arc<MockSandboxProvider>,
    /// The in-memory cloud, exposed so tests can assert teardown left nothing tagged.
    pub compute: Arc<MockCloudCompute>,
    /// `repo_url` to pass to `create_optimization`.
    pub repo_url: String,
    pub base_ref: String,
    /// The simulated repo's ratchet.yaml (after the overlay), parsed: objective, value model, guardrails.
    pub config: RatchetConfig,
    pub git: Arc<LocalGitHost>,
    /// The mock environment drafter (no agent, no AWS): deterministic drafts per scope, validated and priced by the real
    /// validator and cost estimator. Approving one commits its files to the simulated repo through the same commit / PR
    /// flow as live mode.
    pub environment: Arc<dyn ratchet_env::EnvironmentDrafter>,
}

impl SimulationRuntime {
    pub async fn dispose(&self) {
        self.engine.close().await;
    }
}

/// `ratchet.sim.yaml`: how a demo's ratchet.yaml is adapted for local runs.
///
/// ```yaml
/// sealed:                # optional: produce the sealed holdout/trace OUTSIDE the repo (run in the built project dir)
///   run: node dist/eval/holdout.js --seed 424242 --out "{out}"
///   file: sealed-holdout.json
/// evaluator:             # any evaluator keys, merged over ratchet.yaml's
///   run: node dist/eval/backtest.js
///   repeats: 2           # numbers go unquoted; the `repeats` option overrides it
///   holdout: "{sealed}"
/// ```
///
/// Placeholders in strings: `{out}` (path the sealed command must write), `{sealed}` (that path, for use in the overlay) and
/// `{duration}` (load-generator seconds). `evaluator.repeats`: the `repeats` option, else the overlay's, else ratchet.yaml's, else 3.
/// Without an overlay ratchet.yaml is used as it is.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SimOverlay {
    #[serde(default)]
    pub sealed: Option<SimSealed>,
    #[serde(default)]
    pub evaluator: Option<Mapping>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimSealed {
    pub run: String,
    pub file: String,
}

const SIM_OVERLAY: &str = "ratchet.sim.yaml";

fn fill_str(s: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = s.to_owned();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

fn fill(value: &Value, vars: &BTreeMap<&str, String>) -> Value {
    match value {
        Value::String(s) => Value::String(fill_str(s, vars)),
        Value::Sequence(a) => Value::Sequence(a.iter().map(|v| fill(v, vars)).collect()),
        Value::Mapping(m) => {
            Value::Mapping(m.iter().map(|(k, v)| (k.clone(), fill(v, vars))).collect())
        }
        other => other.clone(),
    }
}

fn err(msg: impl Into<String>) -> EngineError {
    EngineError::invalid_input(msg)
}

pub struct SimulationRepo {
    pub remote_dir: PathBuf,
    pub base_ref: String,
    pub bench_ref: String,
    /// The adapted ratchet.yaml as a YAML document.
    pub config: Value,
}

pub struct PrepareSimulationRepo {
    pub work_root: PathBuf,
    pub repo_root: PathBuf,
    pub project: SimulationProject,
    pub project_dir: Option<PathBuf>,
    pub bench_duration_sec: Option<u32>,
    pub repeats: Option<u32>,
}

async fn sh(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> EngineResult<()> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().copied())
        .output()
        .await
        .map_err(|e| err(format!("git {}: {e}", args.join(" "))))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Copy the project into a bare "GitHub" remote, adapted for fast local runs, with a protected `ratchet/bench` ref.
pub async fn prepare_simulation_repo(opts: PrepareSimulationRepo) -> EngineResult<SimulationRepo> {
    let project_dir = std::path::absolute(
        opts.project_dir
            .clone()
            .unwrap_or_else(|| opts.repo_root.join(opts.project.dir())),
    )
    .map_err(|e| err(e.to_string()))?;
    let root = std::path::absolute(&opts.work_root).map_err(|e| err(e.to_string()))?;
    std::fs::create_dir_all(&root).map_err(|e| err(e.to_string()))?;

    let overlay_path = project_dir.join(SIM_OVERLAY);
    let overlay: SimOverlay = if overlay_path.exists() {
        let text = std::fs::read_to_string(&overlay_path).map_err(|e| err(e.to_string()))?;
        serde_norway::from_str(&text).map_err(|e| err(format!("{SIM_OVERLAY}: {e}")))?
    } else {
        SimOverlay::default()
    };
    let duration = opts.bench_duration_sec.unwrap_or(5);

    // Sealed holdout / hidden trace: same distribution family, a seed the agents never see, kept outside the repo.
    let mut sealed: Option<String> = None;
    if let Some(s) = &overlay.sealed {
        let sealed_dir = root.join("sealed");
        std::fs::create_dir_all(&sealed_dir).map_err(|e| err(e.to_string()))?;
        let file = sealed_dir.join(&s.file);
        if !file.exists() {
            let cmd = fill_str(
                &s.run,
                &BTreeMap::from([("out", file.to_string_lossy().into_owned())]),
            );
            // `node` must resolve even when the app was launched from the Dock (no node on PATH).
            let env: IndexMap<String, String> = std::env::vars().collect();
            let env = with_node_on_path(&env, &Default::default());
            let out = tokio::process::Command::new("sh")
                .args(["-c", &cmd])
                .current_dir(&project_dir)
                .env_clear()
                .envs(env)
                .output()
                .await
                .map_err(|e| err(e.to_string()))?;
            if !out.status.success() {
                let first = String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned();
                return Err(err(format!(
                    "could not produce the sealed holdout with `{cmd}` in {} (is the project built? run `pnpm build`): Command failed: sh -c {cmd}{}",
                    project_dir.display(),
                    if first.is_empty() { String::new() } else { format!("\n{first}") }
                )
                .lines()
                .next()
                .unwrap_or("")
                .to_owned()));
            }
        }
        sealed = Some(file.to_string_lossy().into_owned());
    }

    let remote = create_bare_remote(
        &project_dir,
        CreateBareRemoteOptions {
            remote_dir: Some(root.join("remote.git")),
            work_dir: Some(root.join("seed")),
            branch: Some("main".into()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| err(e.to_string()))?;

    let cfg_path = remote.work_dir.join("ratchet.yaml");
    let text = std::fs::read_to_string(&cfg_path).map_err(|e| err(e.to_string()))?;
    let mut cfg: Value =
        serde_norway::from_str(&text).map_err(|e| err(format!("ratchet.yaml: {e}")))?;
    let top = cfg
        .as_mapping_mut()
        .ok_or_else(|| err("ratchet.yaml must be a mapping"))?;
    // `bench:` is the legacy alias of `evaluator:`
    let key = if top.contains_key("evaluator") {
        "evaluator"
    } else {
        "bench"
    };
    let ev = top
        .get_mut(key)
        .and_then(Value::as_mapping_mut)
        .ok_or_else(|| err("ratchet.yaml has no evaluator"))?;
    let vars = BTreeMap::from([
        ("duration", duration.to_string()),
        ("sealed", sealed.clone().unwrap_or_default()),
    ]);
    if let Some(over) = &overlay.evaluator {
        for (k, v) in over {
            ev.insert(k.clone(), fill(v, &vars));
        }
    }
    if let Some(r) = opts.repeats.filter(|r| *r > 0) {
        ev.insert("repeats".into(), Value::Number(r.into()));
    } else if !ev.contains_key("repeats") {
        ev.insert("repeats".into(), Value::Number(3.into()));
    }
    ev.remove("confirm_trace");
    if let Some(s) = &sealed {
        ev.insert("holdout".into(), Value::String(s.clone()));
    } else if overlay.sealed.is_none()
        && overlay
            .evaluator
            .as_ref()
            .is_none_or(|e| !e.contains_key("holdout"))
    {
        ev.remove("holdout");
    }
    let bench_ref = ev
        .get("ref")
        .and_then(Value::as_str)
        .unwrap_or("ratchet/bench")
        .to_owned();
    std::fs::write(
        &cfg_path,
        serde_norway::to_string(&cfg).map_err(|e| err(e.to_string()))?,
    )
    .map_err(|e| err(e.to_string()))?;
    let _ = std::fs::remove_file(remote.work_dir.join(SIM_OVERLAY));
    let env = [
        ("GIT_AUTHOR_NAME", "Ratchet"),
        ("GIT_AUTHOR_EMAIL", "ratchet@localhost"),
        ("GIT_COMMITTER_NAME", "Ratchet"),
        ("GIT_COMMITTER_EMAIL", "ratchet@localhost"),
    ];
    let w = &remote.work_dir;
    sh(
        w,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-am",
            "Simulation: local evaluator settings",
        ],
        &env,
    )
    .await?;
    sh(w, &["push", "-q", "origin", "main"], &env).await?;
    // The trusted evaluator lives on a protected ref; agents never push here.
    sh(
        w,
        &[
            "push",
            "-q",
            "origin",
            &format!("main:refs/heads/{bench_ref}"),
        ],
        &env,
    )
    .await?;
    Ok(SimulationRepo {
        remote_dir: remote.remote_dir,
        base_ref: "main".into(),
        bench_ref,
        config: cfg,
    })
}

/// The mock "bill" mirrors the ledger's own estimate, so a simulated reconcile is a no-op rather than a lag.
struct LedgerMirrorCosts(Ledger);

#[async_trait]
impl CostReconciler for LedgerMirrorCosts {
    async fn actuals(&self, q: &CostActualsQuery) -> PortResult<CostActuals> {
        let b = self
            .0
            .spend_breakdown(&q.optimization_id)
            .await
            .map_err(|e| PortError::failed(e.message))?;
        let pairs: Vec<(SpendCategory, f64)> = SpendCategory::ALL
            .iter()
            .map(|c| (*c, b.by_category.get(*c)))
            .collect();
        Ok(billed(&pairs))
    }
}

pub async fn create_simulation_runtime(opts: SimulationOptions) -> EngineResult<SimulationRuntime> {
    let clock: Arc<dyn Clock> = opts.clock.clone().unwrap_or_else(super::system_clock);
    let root = std::path::absolute(&opts.work_root).map_err(|e| err(e.to_string()))?;
    let project = opts.project;
    let repo = prepare_simulation_repo(PrepareSimulationRepo {
        work_root: root.clone(),
        repo_root: opts.repo_root.clone(),
        project,
        project_dir: opts.project_dir.clone(),
        bench_duration_sec: opts.bench_duration_sec.filter(|d| *d > 0),
        repeats: opts.repeats.filter(|r| *r > 0),
    })
    .await?;
    let parsed =
        parse_config(&serde_norway::to_string(&repo.config).map_err(|e| err(e.to_string()))?)
            .map_err(|errors| {
                err(format!(
                    "simulated ratchet.yaml is invalid: {}",
                    errors
                        .iter()
                        .map(|e| format!("{}: {}", e.path, e.message))
                        .collect::<Vec<_>>()
                        .join("; ")
                ))
            })?;
    let git = Arc::new(
        LocalGitHost::new(LocalGitHostOptions::new(&repo.remote_dir).pr_dir(root.join("prs")))
            .map_err(|e| err(e.to_string()))?,
    );
    let bin = opts
        .bin_dir
        .clone()
        .unwrap_or_else(|| opts.repo_root.join("node_modules/.bin"));
    let tsc = opts
        .tsc
        .clone()
        .unwrap_or_else(|| opts.repo_root.join("node_modules/typescript/bin/tsc"));
    let bench_scratch = root.join("bench");
    std::fs::create_dir_all(&bench_scratch).map_err(|e| err(e.to_string()))?;
    let artifacts = Arc::new(FsArtifactStore::new(root.join("artifacts")));
    let compute = Arc::new(MockCloudCompute::new(MockCloudComputeOptions::with_clock(
        clock.clone(),
    )));
    // The ledger outlives the process but the mock does not: sandboxes it recorded as applied stay applied.
    let initial: HashMap<String, SandboxOutputs> = opts
        .ledger
        .list_sandboxes()
        .await?
        .into_iter()
        .filter(|r| r.status == SandboxStatus::Applied && r.provider == SandboxProviderKind::Mock)
        .filter_map(|r| {
            let out = serde_json::from_value(r.outputs?).ok()?;
            Some((r.id.to_string(), out))
        })
        .collect();
    let sandbox = Arc::new(MockSandboxProvider::new(MockSandboxProviderOptions {
        initial,
        ..Default::default()
    }));

    let load = |v: DemoVariant| project.scripts(v).map_err(|e| err(e.to_string()));
    let mut scripts: BTreeMap<AgentKind, AgentScript> = BTreeMap::new();
    for (kind, variant) in [
        (AgentKind::ClaudeCode, DemoVariant::Cc),
        (AgentKind::Codex, DemoVariant::Cx),
        (AgentKind::Mock, DemoVariant::Cc),
    ] {
        let s = match opts.scripts.get(&kind) {
            Some(s) => s.clone(),
            None => load(variant)?,
        };
        scripts.insert(kind, s);
    }

    let mut bo = LocalBenchRunnerOptions::new(git.clone(), bench_scratch, clock.clone());
    bo.setup = Some(format!("node {} -p tsconfig.build.json", tsc.display()));
    bo.artifacts = Some(artifacts.clone());
    bo.env.insert(
        "PATH".into(),
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    bo.env.insert("CI".into(), "1".into());
    let bench = Arc::new(LocalBenchRunner::new(bo));

    let scripts = Arc::new(scripts);
    let adapter_clock = clock.clone();
    let mut ro = LocalRunnerControlOptions::new(
        root.join("agents"),
        Arc::new(move |config, _handle| {
            let script = scripts
                .get(&config.kind)
                .or_else(|| scripts.get(&AgentKind::ClaudeCode))
                .cloned()
                .ok_or_else(|| "no script".to_owned())?;
            Ok(Arc::new(ScriptedAgent::with_options(
                script,
                ScriptedAgentOptions {
                    clock: Some(adapter_clock.clone()),
                    ..Default::default()
                },
            )) as Arc<dyn AgentAdapter>)
        }),
    );
    ro.clock = Some(clock.clone());
    let runners: Arc<dyn RunnerControl> = Arc::new(LocalRunnerControl::new(ro));

    let base = EngineSettings {
        cost_lag_ms: Some(0),
        secrets: Some(Arc::new(|_, _| {
            Some("simulation-placeholder-secret".to_owned())
        })),
        scratch_dir: Some(root.join("scratch")),
        tick_interval_ms: Some(1000),
        ..EngineSettings::default()
    };
    let environment: Arc<dyn ratchet_env::EnvironmentDrafter> = Arc::new(
        ratchet_env::MockEnvironmentDrafter::new(ratchet_env::MockEnvironmentDrafterOptions {
            clock: Some(clock.clone()),
            git: Some(ratchet_env::fixed_git(
                git.clone() as Arc<dyn ratchet_core::ports::GitHost>
            )),
            ..Default::default()
        }),
    );
    let engine = Engine::new(EngineOptions {
        ledger: opts.ledger.clone(),
        mode: Mode::Simulation,
        clock,
        ports: EnginePorts {
            sandbox: sandbox.clone(),
            compute: compute.clone(),
            prices: Arc::new(StaticPriceBook::new([])),
            artifacts,
            git: GitSource::One(git.clone()),
            runners,
            local_runners: None,
            bench,
            costs: Some(Arc::new(LedgerMirrorCosts(opts.ledger.clone()))),
            // No agent, no AWS: deterministic drafts per scope; approving one commits its files to the simulated repo.
            environment: Some(environment.clone()),
        },
        settings: overlay_settings(base, opts.settings.clone()),
    })
    .await;

    Ok(SimulationRuntime {
        engine,
        sandbox,
        compute,
        repo_url: repo.remote_dir.to_string_lossy().into_owned(),
        base_ref: repo.base_ref,
        config: parsed,
        git,
        environment,
    })
}
