//! Cloud compute: the AWS implementation, its user data, and the in-memory mock.

pub mod aws;
pub mod mock;
pub mod user_data;

pub use aws::{
    AGENT_POLICY_NAME, AwsClients, AwsCloudCompute, AwsCloudComputeOptions, AwsComputeConfig,
    KILL_SWITCH_POLICY_NAME, agent_role_name, bench_profile_for, list_agent_roles, secret_prefix,
    to_instance_info,
};
pub use mock::{MockCloudCompute, MockCloudComputeOptions, MockCommandHandler, command_handler};
pub use user_data::{
    BENCH_OS_USER, CANDIDATE_ENV_KEEP, CANDIDATE_EXEC_PATH, CANDIDATE_EXEC_SCRIPT,
    CANDIDATE_LAUNCHER_PATH, CANDIDATE_LAUNCHER_SCRIPT, CANDIDATE_SUDOERS, DEPS_OS_USER,
    HOLDOUT_OS_USER, RunnerNetworkOptions, bench_user_data, deps_user_data, runner_user_data,
};
