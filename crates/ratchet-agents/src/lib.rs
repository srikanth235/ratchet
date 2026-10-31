//! Agent adapters and the runner: everything that makes Claude Code and Codex work on a repository for Ratchet.
//!
//! - `claude_code`, `codex`, `cli_agent`, `process`: adapters that spawn the CLIs and map their streams to `AgentEvent`s
//! - `detect`, `local_agents`: finds the CLIs installed and signed in on this machine and runs them in git worktrees
//! - `git`, `workdir`, `context`: git helpers, per-agent worktrees, the brief and context the agent sees
//! - `inbox`, `artifacts`: the engine/runner message channels (in memory, filesystem, S3 through the `aws` CLI)
//! - `runner_loop`, `control`: the loop that owns one agent on one branch, and how the engine drives it (local or SSM)
//! - `scripted`, `demo_scripts`, `testing`, `clock`: simulation and test doubles
//! - `runner_main` and the `ratchet-runner` binary
//!
//! The ports (`AgentAdapter`, `RunnerControl`, `RunnerInbox`, `Clock`, `ArtifactStore`, `CloudCompute`) and the data they exchange
//! are `ratchet-core`'s; the filesystem artifact store is `ratchet-sandbox`'s.

pub mod artifacts;
pub mod claude_code;
pub mod cli_agent;
pub mod clock;
pub mod codex;
pub mod context;
pub mod control;
pub mod demo_scripts;
pub mod detect;
pub mod git;
pub mod inbox;
pub mod local_agents;
pub mod process;
pub mod runner_loop;
pub mod runner_main;
pub mod scripted;
pub mod testing;
pub mod workdir;

pub use artifacts::{
    ArtifactEventSink, ArtifactInbox, AwsCliArtifactStore, EventSinkOptions, config_key, done_key,
    event_key, inbox_key, last_seq, read_events, runner_prefix, seq_of,
};
pub use claude_code::{
    ClaudeCodeAdapter, ClaudeCodeOptions, ClaudePermissionMode, ClaudeResult, ClaudeStreamParser,
    DEFAULT_CLAUDE_TOOLS, PromptVia,
};
pub use cli_agent::{CliAgent, CliAgentOptions};
pub use clock::{AcceleratedClock, ManualClock};
pub use codex::{
    CodexAdapter, CodexOptions, CodexPricing, CodexSandbox, CodexStreamParser, CodexTurnResult,
};
pub use context::{
    ContextInput, HistoryEntry, ResumeInput, build_context, build_initial_prompt,
    build_resume_message, format_duration, improvement_pct,
};
pub use control::{
    LocalRunnerControl, LocalRunnerControlOptions, SsmRunnerControl, SsmRunnerControlOptions,
    WorkdirMode,
};
pub use demo_scripts::{
    DemoVariant, load_demo_cli_script, load_demo_forecast_script, load_demo_search_script,
    load_script,
};
pub use detect::{
    DetectFs, DetectOptions, HarnessAuth, HarnessInfo, HarnessKind, LoginPath, NodeDetectFs,
    Platform, detect_harnesses, fallback_path_dirs, local_agent_env, parse_version,
    resolve_login_path,
};
pub use git::{
    CANDIDATE_TRAILER, ExecGit, Git, GitCall, GitError, GitResult, GitRunner, exec_git,
    has_candidate_trailer, with_candidate_trailer,
};
pub use inbox::MemoryInbox;
pub use local_agents::{
    LocalAdapterOptions, LocalAgentControlOptions, create_local_agent_control,
    local_adapter_factory, local_adapter_for, local_agent_problem,
};
pub use process::{
    LineSplitter, RunProcessOptions, RunProcessResult, Spawner, TokioSpawner, run_process,
};
pub use runner_loop::{
    LoopTuning, RunnerLoopOptions, RunnerLoopResult, RunnerStopReason, run_runner_loop,
};
pub use runner_main::{
    AGENT_ENV_PASSTHROUGH, RunnerMainError, RunnerMainOptions, parse_secret_refs, runner_main,
    validate_config,
};
pub use scripted::{
    AgentScript, ScriptAttempt, ScriptStep, ScriptedAgent, ScriptedAgentOptions, StepCondition,
    price_usage,
};
pub use testing::{FakeCall, FakeProcessSpec, FakeSpawner};
pub use workdir::{
    add_worktree, ensure_local_clone, git_auth_env, prepare_workdir, remove_worktree,
};
