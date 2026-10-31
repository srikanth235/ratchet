//! `RunnerControl` implementations: in this process (`LocalRunnerControl`, simulation and "This machine") and over SSM and the
//! artifacts bucket (`SsmRunnerControl`, cloud runners).

pub mod local;
pub mod ssm;

pub use local::{AdapterFor, AgentEnvFor, LocalRunnerControl, LocalRunnerControlOptions, WorkdirMode};
pub use ssm::{SsmRunnerControl, SsmRunnerControlOptions};
