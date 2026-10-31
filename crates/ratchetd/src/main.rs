//! `ratchetd`: the Ratchet daemon.
//!
//! ```text
//! ratchetd [serve]                 serve the API on the unix socket (default)
//! ratchetd openapi                 write api/openapi.yaml  (--stdout, --check, --out PATH)
//! ratchetd launchagent install     write ~/Library/LaunchAgents/com.ratchet.ratchetd.plist and load it
//! ratchetd launchagent uninstall
//! ratchetd launchagent print       print the plist (for an SMAppService bundle: --bundle-program)
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use ratchet_engine::desktop::{
    DefaultPathsInput, NoPowerBlocker, PowerSaveHook, default_live_paths, default_secret_store,
};
use ratchet_engine::runtime::{ConnectionTesterDeps, create_connection_tester};
use ratchet_github::{GithubApiOptions, HttpGithubApi};
use ratchetd::api::ApiEvent;
use ratchetd::launchagent::{self, LaunchAgentSpec, Launchctl, NoLaunchctl, SystemLaunchctl};
use ratchetd::real::{RealService, RealServiceOptions, SettingsStore};
use ratchetd::server::{Daemon, DaemonConfig, shutdown_signal};
use ratchetd::service::AppService;
use ratchetd::sim::{SimOptions, SimService};
use ratchetd::{openapi, paths};

#[derive(Parser)]
#[command(name = "ratchetd", version, about = "The Ratchet daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the API on the unix socket (the default).
    Serve(ServeArgs),
    /// Generate the OpenAPI document.
    Openapi(OpenapiArgs),
    /// Install or remove the LaunchAgent.
    Launchagent {
        #[command(subcommand)]
        action: LaunchAgentCmd,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// The scripted simulation with seeded demo data (design work, tests).
    Sim,
    /// The real engine. Demos still run simulated, one runtime per demo project.
    #[default]
    Real,
}

#[derive(Args, Clone, Default)]
struct ServeArgs {
    /// `real` (default) or `sim`.
    #[arg(long, value_enum, default_value_t = Mode::Real, env = "RATCHET_MODE")]
    mode: Mode,
    /// The settings file of the real engine (default: `<support dir>/settings.json`).
    #[arg(long, env = "RATCHET_SETTINGS")]
    settings: Option<PathBuf>,
    /// Plaintext secrets file for machines without a keychain (mode 0600). Without it, secrets are refused there.
    #[arg(long, env = "RATCHET_SECRETS_FILE")]
    secrets_file: Option<PathBuf>,
    /// The monorepo root the demos run from (`examples/`, `node_modules`).
    #[arg(long, env = "RATCHET_REPO_ROOT")]
    repo_root: Option<PathBuf>,
    /// Bundled resources (`infra/aws`, `bin/tofu`) of a packaged app.
    #[arg(long, env = "RATCHET_RESOURCES_PATH")]
    resources_path: Option<PathBuf>,
    /// The unix socket (default: `<support dir>/ratchetd.sock`).
    #[arg(long, env = "RATCHET_SOCKET")]
    socket: Option<PathBuf>,
    /// The ledger file (default: `<support dir>/ratchet.db`; `:memory:` for a throwaway one).
    #[arg(long, env = "RATCHET_DB")]
    ledger: Option<String>,
    /// Do not seed demo data into an empty ledger.
    #[arg(long)]
    no_seed: bool,
    /// Milliseconds between simulated agent turns.
    #[arg(long, default_value_t = 2500)]
    tick_ms: u64,
}

#[derive(Args)]
struct OpenapiArgs {
    /// Print to stdout instead of writing the file.
    #[arg(long)]
    stdout: bool,
    /// Exit 1 when the file on disk differs from the generated document.
    #[arg(long)]
    check: bool,
    /// Where to write (default: `api/openapi.yaml` in the repository).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Emit JSON instead of YAML (with --stdout).
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum LaunchAgentCmd {
    /// Write the plist and load it with launchctl.
    Install(InstallArgs),
    /// Unload the agent and remove the plist.
    Uninstall {
        /// Home directory (default: $HOME).
        #[arg(long)]
        home: Option<PathBuf>,
        /// Only remove the plist; do not call launchctl.
        #[arg(long)]
        no_load: bool,
    },
    /// Print the plist.
    Print(InstallArgs),
}

#[derive(Args)]
struct InstallArgs {
    /// The ratchetd binary (default: this one).
    #[arg(long)]
    program: Option<PathBuf>,
    /// Relative path inside the app bundle, for an SMAppService plist (`Contents/MacOS/ratchetd`).
    #[arg(long)]
    bundle_program: Option<String>,
    /// Home directory (default: $HOME).
    #[arg(long)]
    home: Option<PathBuf>,
    /// Write the plist but do not call launchctl.
    #[arg(long)]
    no_load: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RATCHET_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match cli.command {
        None => serve(cli.serve).await,
        Some(Command::Serve(a)) => serve(a).await,
        Some(Command::Openapi(a)) => generate(&a),
        Some(Command::Launchagent { action }) => launch_agent(action),
    }
}

async fn serve(args: ServeArgs) -> Result<()> {
    let socket = args.socket.clone().unwrap_or_else(paths::socket_path);
    let ledger_path = match args.ledger.as_deref() {
        Some(":memory:") => None,
        Some(p) => Some(PathBuf::from(p)),
        None => Some(paths::ledger_path()),
    };
    let (service, stop): (Arc<dyn AppService>, Stop) = match args.mode {
        Mode::Sim => {
            let service = SimService::new(SimOptions {
                ledger_path,
                seed_demo: !args.no_seed,
                tick_ms: args.tick_ms,
                ..SimOptions::default()
            })
            .await
            .context("cannot start the simulation service")?;
            let s = service.clone();
            (
                Arc::new(service),
                Box::new(move || Box::pin(async move { s.shutdown() })),
            )
        }
        Mode::Real => {
            let service = real_service(&args, ledger_path).await?;
            let s = service.clone();
            (
                Arc::new(service),
                Box::new(move || Box::pin(async move { s.shutdown().await })),
            )
        }
    };
    let daemon = Daemon::start(service, DaemonConfig::new(&socket))
        .await
        .with_context(|| format!("cannot listen on {}", socket.display()))?;
    shutdown_signal().await;
    tracing::info!("shutting down");
    stop().await;
    daemon
        .stop()
        .await
        .context("the server failed while stopping")?;
    Ok(())
}

type Stop =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// The real engine with the app's desktop logic: Keychain secrets, GitHub, the machine's AWS profiles and agent CLIs.
async fn real_service(args: &ServeArgs, ledger_path: Option<PathBuf>) -> Result<RealService> {
    let support = paths::support_dir();
    let settings = Arc::new(SettingsStore::new(
        Some(
            args.settings
                .clone()
                .unwrap_or_else(|| support.join("settings.json")),
        ),
        default_secret_store(args.secrets_file.clone()),
        std::env::vars().collect(),
    ));
    let repo_root = args
        .repo_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let live_defaults = default_live_paths(&DefaultPathsInput {
        is_packaged: args.resources_path.is_some(),
        resources_path: args.resources_path.clone().unwrap_or_default(),
        repo_root: repo_root.clone(),
        exists: None,
    });
    let mut opts = RealServiceOptions::new(settings, support.join("sandboxes"));
    opts.ledger_path = ledger_path;
    opts.live_defaults = live_defaults;
    opts.repo_root = repo_root;
    opts.work_root = support.join("sim").join(std::process::id().to_string());
    opts.app_version = Some(ratchetd::VERSION.to_owned());
    opts.github = Some(Arc::new(HttpGithubApi::new(GithubApiOptions::from_env())));
    opts.connection_probe = Some(Arc::new(create_connection_tester(
        ConnectionTesterDeps::default(),
    )));
    let service = RealService::new(opts)
        .await
        .context("cannot start the real engine")?;
    // Keep the machine awake while a run is active. The blocker is the host's to provide (the Mac app supplies an IOKit
    // assertion); the daemon alone has none, so this is a hook point.
    let hook = PowerSaveHook::new(NoPowerBlocker);
    let mut events = service.subscribe();
    tokio::spawn(async move {
        while let Ok(e) = events.recv().await {
            if let ApiEvent::RunActive { active, .. } = e {
                hook.on_run_active(active);
            }
        }
        hook.release();
    });
    Ok(service)
}

fn generate(a: &OpenapiArgs) -> Result<()> {
    let doc = if a.json {
        openapi::openapi_json()
    } else {
        openapi::openapi_yaml()
    };
    if a.stdout {
        print!("{doc}");
        return Ok(());
    }
    let path = a.out.clone().unwrap_or_else(openapi::committed_path);
    if a.check {
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        if on_disk != doc {
            bail!(
                "{} is stale; run `cargo run -p ratchetd -- openapi` and commit it",
                path.display()
            );
        }
        println!("{} is up to date", path.display());
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, doc).with_context(|| format!("cannot write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn home(h: Option<PathBuf>) -> Result<PathBuf> {
    h.or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .context("cannot find the home directory; pass --home")
}

fn spec_from(a: &InstallArgs) -> Result<LaunchAgentSpec> {
    let program = match &a.program {
        Some(p) => p.clone(),
        None => std::env::current_exe()?,
    };
    let mut spec = LaunchAgentSpec::new(program, paths::support_dir());
    spec.bundle_program.clone_from(&a.bundle_program);
    if let Some(h) = std::env::var_os("RATCHET_HOME") {
        spec.env.push((
            "RATCHET_HOME".into(),
            PathBuf::from(h).display().to_string(),
        ));
    }
    Ok(spec)
}

fn launch_agent(cmd: LaunchAgentCmd) -> Result<()> {
    let ctl = |no_load: bool| -> Box<dyn Launchctl> {
        if no_load || !cfg!(target_os = "macos") {
            Box::new(NoLaunchctl)
        } else {
            Box::new(SystemLaunchctl)
        }
    };
    match cmd {
        LaunchAgentCmd::Print(a) => {
            print!("{}", launchagent::plist(&spec_from(&a)?));
        }
        LaunchAgentCmd::Install(a) => {
            let spec = spec_from(&a)?;
            let path =
                launchagent::install(&spec, &home(a.home.clone())?, ctl(a.no_load).as_ref())?;
            println!("installed {}", path.display());
        }
        LaunchAgentCmd::Uninstall { home: h, no_load } => {
            let removed =
                launchagent::uninstall(launchagent::LABEL, &home(h)?, ctl(no_load).as_ref())?;
            println!(
                "{}",
                if removed {
                    "removed"
                } else {
                    "was not installed"
                }
            );
        }
    }
    Ok(())
}
