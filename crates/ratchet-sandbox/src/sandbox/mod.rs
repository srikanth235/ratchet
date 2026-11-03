//! The sandbox provider: OpenTofu as a subprocess, the `infra:` override with contract validation, and the mock.

pub mod environment;
pub mod environment_check;
pub mod infra_override;
pub mod mock;
pub mod process;
pub mod tofu;

pub use environment::{
    BASE_MODULE_DIRNAME, EnvironmentRejectedError, EnvironmentVarsOptions, GENERATED_INPUTS_FILE,
    GENERATED_INPUTS_TF, GENERATED_OUTPUTS_FILE, GENERATED_OUTPUTS_TF, assert_drafted_environment,
    environment_vars, materialize_custom_environment, materialize_drafted_environment,
    parse_environment_outputs,
};
pub use environment_check::{TofuEnvironmentChecker, TofuEnvironmentCheckerOptions};
pub use infra_override::{
    ContractCheck, ContractCheckOptions, ContractViolationError, GitSource, InfraOverrideError,
    MaterializedInfra, assert_sandbox_contract, check_sandbox_contract, materialize_infra_override,
    read_infra_override,
};
pub use mock::{MockSandboxProvider, MockSandboxProviderOptions, SANDBOX_RESOURCES};
pub use process::{ProcessResult, ProcessRunner, ProcessSpec, TokioProcessRunner};
pub use tofu::{
    AwsSandboxOutputs, IamRoleLister, RuntimeRoleLister, SharedRoles, TeardownBlockedError,
    TofuError, TofuSandboxProvider, TofuSandboxProviderOptions, map_actions, parse_tofu_outputs,
    summarize_plan,
};
