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
struct Cli {}

#[tokio::main]
async fn main() {
    let _cli = Cli::parse();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

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
