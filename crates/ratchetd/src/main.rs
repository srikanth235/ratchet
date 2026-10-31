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
use clap::{Args, Parser, Subcommand};
use ratchetd::launchagent::{self, LaunchAgentSpec, Launchctl, NoLaunchctl, SystemLaunchctl};
use ratchetd::server::{Daemon, DaemonConfig, shutdown_signal};
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

#[derive(Args, Clone, Default)]
struct ServeArgs {
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
    let socket = args.socket.unwrap_or_else(paths::socket_path);
    let ledger_path = match args.ledger.as_deref() {
        Some(":memory:") => None,
        Some(p) => Some(PathBuf::from(p)),
        None => Some(paths::ledger_path()),
    };
    // TODO(engine): `ratchet-engine` replaces the simulation here once it implements `AppService`.
    let service = SimService::new(SimOptions {
        ledger_path,
        seed_demo: !args.no_seed,
        tick_ms: args.tick_ms,
        ..SimOptions::default()
    })
    .await
    .context("cannot start the simulation service")?;
    let daemon = Daemon::start(Arc::new(service.clone()), DaemonConfig::new(&socket))
        .await
        .with_context(|| format!("cannot listen on {}", socket.display()))?;
    shutdown_signal().await;
    tracing::info!("shutting down");
    service.shutdown();
    daemon
        .stop()
        .await
        .context("the server failed while stopping")?;
    Ok(())
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
