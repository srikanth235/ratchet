//! The sandbox provider: OpenTofu as a subprocess, the `infra:` override with contract validation, and the mock.

pub mod infra_override;
pub mod mock;
pub mod process;
pub mod tofu;

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
