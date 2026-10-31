//! The command line, as clap types.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "ratchet",
    version,
    about = "Ratchet: autonomous optimization of a repo against a benchmark",
    long_about = "Ratchet: autonomous optimization of a repo against a benchmark.\n\n\
        Commands that work on a repo (validate, init, baseline) run offline. The others talk to the ratchetd daemon \
        on its unix socket, the same API the Mac app uses. Add --json to any command for machine-readable output.",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// The ratchetd unix socket (default: the daemon's standard location).
    #[arg(long, global = true, env = "RATCHET_SOCKET", value_name = "PATH")]
    pub socket: Option<PathBuf>,
    /// File holding the daemon's bearer token (default: ratchetd.token next to the socket).
    #[arg(long, global = true, env = "RATCHET_TOKEN_FILE", value_name = "PATH")]
    pub token_file: Option<PathBuf>,
    /// The daemon's bearer token itself.
    #[arg(
        long,
        global = true,
        env = "RATCHET_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    pub token: Option<String>,
    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Validate ratchet.yaml and print a summary (offline).
    Validate {
        /// Path to ratchet.yaml.
        #[arg(default_value = "ratchet.yaml")]
        path: PathBuf,
    },
    /// Add an evaluator template to this repo: ratchet.yaml, RATCHET.md and the evaluator's files (offline).
    ///
    /// Refuses to overwrite existing files unless --force is given. See --list-templates.
    Init(InitArgs),
    /// Run the evaluator command locally N times and print stats and the noise floor (offline).
    ///
    /// The evaluator must print JSON on stdout; the last JSON object is used and objective.metric is looked up in
    /// it by dotted path (e.g. http_req_duration.p95).
    Baseline(BaselineArgs),
    /// List the evaluator templates and the demos (offline).
    Templates,
    /// The daemon and the optimizations at a glance.
    Status,
    /// List optimizations.
    #[command(visible_alias = "ls")]
    List {
        /// KPI window for the summary.
        #[arg(long, value_enum, default_value_t = Range::Days30)]
        range: Range,
    },
    /// One optimization: agents, baseline vs best, calibration, receipt.
    Show { id: String },
    /// Create an optimization and start it.
    Run(RunArgs),
    /// Stop an optimization (the kill switch).
    Stop {
        id: String,
        /// Why (shown in the log).
        #[arg(long)]
        reason: Option<String>,
    },
    /// The markdown report of an optimization.
    Report {
        id: String,
        /// Write the report to this file instead of printing it.
        #[arg(long, short, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Open the pull request with the verified wins.
    Pr { id: String },
    /// Every candidate with its verdict.
    Candidates { id: String },
    /// The ratchet history: baseline and every verified step.
    History {
        id: String,
        /// Plot another metric of the objective's family.
        #[arg(long)]
        metric: Option<String>,
    },
    /// Spend by category and by agent.
    Spend { id: String },
    /// Recent activity: verdicts and verified wins.
    Activity {
        /// Only this optimization.
        #[arg(long)]
        optimization: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Work with one agent.
    Agent {
        #[command(subcommand)]
        action: AgentCmd,
    },
    /// Stream the daemon's events (server-sent events).
    Events(EventsArgs),
    /// Environments (sandboxes): what is running, end now.
    Env {
        #[command(subcommand)]
        action: EnvCmd,
    },
    /// Spend and billing tools.
    Cost {
        #[command(subcommand)]
        action: CostCmd,
    },
    /// The signed-in GitHub account's repos.
    Repos {
        /// Substring of owner/name.
        query: Option<String>,
    },
    /// What Ratchet learns about a repo: its ratchet.yaml, or the templates to add one.
    Inspect {
        /// owner/name, or a local folder.
        target: String,
    },
    /// Overview: spent, value locked in, return, what needs you (provisional).
    Overview {
        #[arg(long, value_enum, default_value_t = OverviewRangeArg::Days90)]
        range: OverviewRangeArg,
    },
    /// Saved GitHub / AWS / agent connectors (provisional).
    Connectors,
}

#[derive(Args, Debug, Default, Clone)]
pub struct InitArgs {
    /// Template id: http-latency | cli-cold-start | test-suite-time | build-size | memory | forecast-backtest | llm-eval
    #[arg(long)]
    pub template: Option<String>,
    /// Legacy alias of --template (perf = cli-cold-start, forecasting = forecast-backtest).
    #[arg(long)]
    pub pack: Option<String>,
    /// Objective metric, a dotted path into the evaluator JSON (default: the template's).
    #[arg(long)]
    pub metric: Option<String>,
    /// minimize | maximize (default: the template's).
    #[arg(long)]
    pub direction: Option<String>,
    /// Estimated USD per unit of improvement per year.
    #[arg(long, value_name = "USD", allow_hyphen_values = true)]
    pub value_usd: Option<f64>,
    /// Directory to write into.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    /// Overwrite existing files.
    #[arg(long)]
    pub force: bool,
    /// Print what would be written, write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// List the templates and the demos, then exit.
    #[arg(long)]
    pub list_templates: bool,
    /// Legacy alias of --list-templates.
    #[arg(long)]
    pub list_packs: bool,
}

#[derive(Args, Debug, Clone)]
pub struct BaselineArgs {
    /// Path to ratchet.yaml.
    #[arg(default_value = "ratchet.yaml")]
    pub path: PathBuf,
    /// Number of runs (default: evaluator.repeats, else 5).
    #[arg(short = 'n', long, value_parser = positive_int)]
    pub repeats: Option<u64>,
    /// Noise floor = max(k x stddev, min-absolute); default 2.
    #[arg(long, value_parser = finite_number, allow_hyphen_values = true)]
    pub k: Option<f64>,
    /// Absolute lower bound for the noise floor; default 0.
    #[arg(long, value_parser = finite_number, allow_hyphen_values = true)]
    pub min_absolute: Option<f64>,
    /// Kill a single run after this many ms.
    #[arg(long, value_parser = positive_int, value_name = "MS")]
    pub timeout: Option<u64>,
}

#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    /// A GitHub repo (owner/name or URL).
    pub repo: Option<String>,
    /// Run a demo in simulation instead of a repo.
    #[arg(long, value_enum, conflicts_with = "repo")]
    pub demo: Option<DemoArg>,
    /// Name of the optimization.
    #[arg(long)]
    pub name: Option<String>,
    /// Branch to start from.
    #[arg(long, default_value = "main")]
    pub base_ref: String,
    /// Agents to run (repeat the flag, or comma-separate).
    #[arg(long = "agent", value_enum, value_delimiter = ',', default_values_t = [AgentArg::ClaudeCode])]
    pub agents: Vec<AgentArg>,
    /// Where the agents run.
    #[arg(long = "where", value_enum)]
    pub agents_where: Option<WhereArg>,
    /// Budget in USD.
    #[arg(long, default_value_t = 50.0, value_parser = finite_number, allow_hyphen_values = true)]
    pub budget: f64,
    /// Time limit: 3600, 90m, 2h, 45s.
    #[arg(long, default_value = "2h", value_parser = parse_duration, value_name = "DURATION")]
    pub time_limit: u64,
    /// Stop after this many evaluations without a verified win.
    #[arg(long, default_value_t = 25)]
    pub plateau: u32,
    /// Create the optimization but do not start it.
    #[arg(long)]
    pub no_start: bool,
    /// Wait until the optimization has ended (exit 1 unless it completed).
    #[arg(long)]
    pub wait: bool,
    /// With --wait: give up after this long.
    #[arg(long, value_parser = parse_duration, value_name = "DURATION")]
    pub wait_timeout: Option<u64>,
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Pause the agent after its current step.
    Pause { id: String },
    /// Resume a paused agent.
    Resume { id: String },
    /// Send the agent a note; it reads it at the start of its next turn.
    Message { id: String, text: String },
    /// The agent's log.
    Log {
        id: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Only events after this id.
        #[arg(long)]
        after: Option<i64>,
    },
}

#[derive(Args, Debug, Clone, Default)]
pub struct EventsArgs {
    /// Resume after this event id.
    #[arg(long)]
    pub last_event_id: Option<u64>,
    /// Stop after this many events (hello and resync are not counted).
    #[arg(long)]
    pub count: Option<usize>,
    /// Stop after this long.
    #[arg(long, value_parser = parse_duration, value_name = "DURATION")]
    pub timeout: Option<u64>,
}

#[derive(Subcommand, Debug)]
pub enum EnvCmd {
    /// What is running and what it costs.
    List,
    /// Tear an environment down now (refused while a run is active).
    End { sandbox: String },
}

#[derive(Subcommand, Debug)]
pub enum CostCmd {
    /// Compare the ledger's estimates with billing and record correction rows. Idempotent.
    Reconcile { id: String },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    #[value(name = "7d")]
    Days7,
    #[value(name = "30d")]
    Days30,
    #[value(name = "90d")]
    Days90,
}

impl Range {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::Days90 => "90d",
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewRangeArg {
    #[value(name = "30d")]
    Days30,
    #[value(name = "90d")]
    Days90,
    #[value(name = "all")]
    All,
}

impl OverviewRangeArg {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Days30 => "30d",
            Self::Days90 => "90d",
            Self::All => "all",
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoArg {
    #[value(name = "demo-search")]
    Search,
    #[value(name = "demo-cli")]
    Cli,
    #[value(name = "demo-forecast")]
    Forecast,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentArg {
    #[value(name = "claude-code")]
    ClaudeCode,
    #[value(name = "codex")]
    Codex,
    #[value(name = "mock")]
    Mock,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhereArg {
    Local,
    Cloud,
}

fn finite_number(v: &str) -> Result<f64, String> {
    let n: f64 = v
        .trim()
        .parse()
        .map_err(|_| "must be a number".to_owned())?;
    if v.trim().is_empty() || !n.is_finite() {
        return Err("must be a number".into());
    }
    Ok(n)
}

fn positive_int(v: &str) -> Result<u64, String> {
    let n = finite_number(v)?;
    if n.fract() != 0.0 || n < 1.0 {
        return Err("must be a positive integer".into());
    }
    Ok(n as u64)
}

/// `3600`, `45s`, `90m`, `2h`, `1d` -> seconds.
pub fn parse_duration(v: &str) -> Result<u64, String> {
    let v = v.trim();
    let (num, mult) = match v.chars().last() {
        Some('s') => (&v[..v.len() - 1], 1),
        Some('m') => (&v[..v.len() - 1], 60),
        Some('h') => (&v[..v.len() - 1], 3600),
        Some('d') => (&v[..v.len() - 1], 86_400),
        _ => (v, 1),
    };
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| "use a duration such as 3600, 90m or 2h".to_owned())?;
    if !n.is_finite() || n <= 0.0 {
        return Err("a duration must be positive".into());
    }
    Ok((n * mult as f64).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("3600"), Ok(3600));
        assert_eq!(parse_duration("45s"), Ok(45));
        assert_eq!(parse_duration("90m"), Ok(5400));
        assert_eq!(parse_duration("2h"), Ok(7200));
        assert_eq!(parse_duration("1.5h"), Ok(5400));
        assert_eq!(parse_duration("1d"), Ok(86_400));
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("-1h").is_err());
        assert!(parse_duration("0").is_err());
    }

    #[test]
    fn numbers() {
        assert_eq!(positive_int("3"), Ok(3));
        assert!(positive_int("0").is_err());
        assert!(positive_int("1.5").is_err());
        assert!(positive_int("abc").is_err());
        assert!(finite_number("inf").is_err());
        assert!(finite_number("").is_err());
        assert_eq!(finite_number("-2.5"), Ok(-2.5));
    }

    #[test]
    fn the_clap_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
