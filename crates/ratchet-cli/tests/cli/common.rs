//! Helpers of the CLI tests: `makeRepo` of the TypeScript tests, a runner over `BufferIo`, and an in-process daemon.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratchet_cli::io::BufferIo;
use ratchetd::server::{Daemon, DaemonConfig};
use ratchetd::sim::{SimOptions, SimService};

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", self.stdout))
    }
}

/// Run `ratchet <args>` in-process.
pub async fn ratchet(args: &[&str]) -> Out {
    let io = BufferIo::new();
    let mut argv = vec!["ratchet".to_owned()];
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    let code = ratchet_cli::run(argv, &io).await;
    Out {
        code,
        stdout: io.stdout(),
        stderr: io.stderr(),
    }
}

/// A repo dir with a ratchet.yaml and a fake bench that prints successive values (`testing.ts` `makeRepo`).
pub struct RepoOpts {
    pub values: Vec<f64>,
    pub metric: String,
    pub bench_run: Option<String>,
    /// JSON template for bench output; `$V` is replaced by the value.
    pub template: String,
}

impl RepoOpts {
    pub fn new(values: &[f64]) -> Self {
        Self {
            values: values.to_vec(),
            metric: "lat.p95".into(),
            bench_run: None,
            template: r#"{"lat":{"p95":$V}}"#.into(),
        }
    }

    pub fn metric(mut self, m: &str) -> Self {
        self.metric = m.into();
        self
    }

    pub fn run(mut self, r: &str) -> Self {
        self.bench_run = Some(r.into());
        self
    }

    pub fn template(mut self, t: &str) -> Self {
        self.template = t.into();
        self
    }
}

pub struct Repo {
    pub dir: tempfile::TempDir,
    pub config: PathBuf,
}

impl Repo {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

pub fn make_repo(o: RepoOpts) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let values = o
        .values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let printf = o.template.replace("$V", "%s");
    std::fs::write(
        dir.path().join("bench.sh"),
        format!(
            "f=counter.txt\ni=0\n[ -f \"$f\" ] && i=$(cat \"$f\")\necho $((i+1)) > \"$f\"\n\
             echo 'warming up {{not json}}'\n\
             v=$(echo '{values}' | awk -v i=\"$i\" '{{print $((i % NF) + 1)}}')\n\
             printf '{printf}\\n' \"$v\"\n"
        ),
    )
    .unwrap();
    let run = o.bench_run.unwrap_or_else(|| "sh bench.sh".into());
    let config = dir.path().join("ratchet.yaml");
    std::fs::write(
        &config,
        format!(
            "version: 0\nobjective:\n  metric: {}\n  direction: minimize\n  min_gain: 0.03\nbench:\n  ref: ratchet/bench\n  run: {}\n  repeats: 3\n  hardware: c7i.2xlarge\nguardrails:\n  - run: pytest -q\n  - error_rate: \"< 0.001\"\nprotected: [bench/**, ratchet.yaml]\nagent:\n  instructions: RATCHET.md\n  allowed_instance_types: [c7i.2xlarge]\n",
            o.metric,
            serde_json::to_string(&run).unwrap()
        ),
    )
    .unwrap();
    Repo { dir, config }
}

/// A daemon serving a manual (timer-free) simulation, and the arguments that point the CLI at it.
pub struct Env {
    pub dir: tempfile::TempDir,
    pub daemon: Daemon,
    pub sim: SimService,
}

impl Env {
    pub async fn start() -> Self {
        Self::with(SimOptions {
            auto_run: false,
            github_flow_ms: 0,
            ..SimOptions::default()
        })
        .await
    }

    pub async fn with(opts: SimOptions) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sim = SimService::new(opts).await.unwrap();
        let daemon = Daemon::start(
            Arc::new(sim.clone()),
            DaemonConfig::new(dir.path().join("ratchetd.sock")),
        )
        .await
        .unwrap();
        Self { dir, daemon, sim }
    }

    pub fn socket(&self) -> String {
        self.daemon.socket_path.display().to_string()
    }

    /// `ratchet --socket <sock> <args>`; the token is read from the file next to the socket.
    pub async fn ratchet(&self, args: &[&str]) -> Out {
        let socket = self.socket();
        let mut full = vec!["--socket", socket.as_str()];
        full.extend_from_slice(args);
        ratchet(&full).await
    }

    pub async fn stop(self) {
        self.sim.shutdown();
        self.daemon.stop().await.unwrap();
    }
}
