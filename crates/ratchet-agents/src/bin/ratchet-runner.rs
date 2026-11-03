//! `ratchet-runner`: runs one agent on one branch on a cloud runner (or locally). All configuration comes from the environment
//! (`RATCHET_OPTIMIZATION_ID`, `RATCHET_AGENT_ID`, `RATCHET_ARTIFACTS_BUCKET` or `RATCHET_ARTIFACT_DIR`, ...) and the runner config
//! object in the artifacts store.
//!
//! Exit codes: 0 finished, 1 the agent failed or was unavailable, 2 misconfigured.

use std::sync::Arc;

use clap::Parser;
use ratchet_agents::{RunnerMainOptions, runner_main};
use ratchet_core::ports::ArtifactStore;
use ratchet_sandbox::FsArtifactStore;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

/// Runs one Ratchet agent on one branch. Configured through RATCHET_* environment variables; see docs/real-env.
#[derive(Parser, Debug)]
#[command(name = "ratchet-runner", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Run the agent (the default).
    Run,
    /// Serve the optimization tool set as an MCP server on stdio, against this runner's inbox and artifact store
    /// (RATCHET_OPTIMIZATION_ID, RATCHET_AGENT_ID, RATCHET_ARTIFACTS_BUCKET or RATCHET_ARTIFACT_DIR).
    Mcp,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    if matches!(cli.command, Some(Cmd::Mcp)) {
        std::process::exit(mcp().await);
    }

    let cancel = CancellationToken::new();
    for kind in [SignalKind::terminate(), SignalKind::interrupt()] {
        match signal(kind) {
            Ok(mut stream) => {
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    stream.recv().await;
                    cancel.cancel();
                });
            }
            Err(err) => tracing::warn!(%err, "cannot install a signal handler"),
        }
    }

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    // S3 (the `aws` CLI) when a bucket is set; otherwise a local directory, the stand-in for S3 in simulation
    let store: Option<Arc<dyn ArtifactStore>> = match (
        env.get("RATCHET_ARTIFACTS_BUCKET")
            .filter(|b| !b.is_empty()),
        env.get("RATCHET_ARTIFACT_DIR").filter(|d| !d.is_empty()),
    ) {
        (None, Some(dir)) => Some(Arc::new(FsArtifactStore::new(dir))),
        _ => None,
    };
    let options = RunnerMainOptions {
        env,
        store,
        signal: Some(cancel),
        ..RunnerMainOptions::default()
    };
    let code = match runner_main(options).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("ratchet-runner: {err}");
            2
        }
    };
    std::process::exit(code);
}

/// `ratchet-runner mcp`: the optimization tool set on stdio. Stdout carries JSON-RPC only.
async fn mcp() -> i32 {
    use ratchet_agents::{AwsCliArtifactStore, RunnerToolBackend, runner_prefix};
    use ratchet_mcp::{McpServer, ServerInfo, SessionTools, ToolSet};

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let get = |k: &str| env.get(k).filter(|v| !v.is_empty()).cloned();
    let (Some(optimization), Some(agent)) =
        (get("RATCHET_OPTIMIZATION_ID"), get("RATCHET_AGENT_ID"))
    else {
        eprintln!("ratchet-runner mcp: set RATCHET_OPTIMIZATION_ID and RATCHET_AGENT_ID");
        return 2;
    };
    let store: Arc<dyn ArtifactStore> =
        match (get("RATCHET_ARTIFACTS_BUCKET"), get("RATCHET_ARTIFACT_DIR")) {
            (Some(bucket), _) => Arc::new(AwsCliArtifactStore::new(
                bucket,
                None,
                Some(env.clone()),
                get("AWS_REGION"),
            )),
            (None, Some(dir)) => Arc::new(FsArtifactStore::new(dir)),
            (None, None) => {
                eprintln!(
                    "ratchet-runner mcp: set RATCHET_ARTIFACTS_BUCKET or RATCHET_ARTIFACT_DIR"
                );
                return 2;
            }
        };
    let backend = RunnerToolBackend::new(store, runner_prefix(&optimization, &agent));
    let tools = Arc::new(SessionTools::new(ToolSet::Optimization, Arc::new(backend)));
    let server = McpServer::new(
        ServerInfo::ratchet(Some(ToolSet::Optimization.instructions())),
        tools,
    );
    match server.serve(tokio::io::stdin(), tokio::io::stdout()).await {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("ratchet-runner mcp: {err}");
            1
        }
    }
}
