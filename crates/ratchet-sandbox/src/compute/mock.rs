//! In-memory `CloudCompute` with the same observable behaviour as `AwsCloudCompute`: id validation, tagged instances, kill
//! switch, ordered delete, idempotent terminate. (TS `compute/mock.ts`)

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use super::aws::{agent_role_name, secret_prefix};
use crate::error::Error;
use crate::util::{DEFAULT_AGENT_ROLE_PATH, assert_valid_id, json_str};
use ratchet_core::enums::InstanceRole;
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{
    AgentIdentity, BenchLaunchSpec, Clock, CloudCompute, InstanceInfo, InstanceState, PortResult,
    RemoteCommand, RemoteCommandResult, RunnerLaunchSpec, SecretRef,
};

type CommandFuture = Pin<Box<dyn Future<Output = PortResult<RemoteCommandResult>> + Send>>;

/// Executes `run_command` scripts; lets a local bench/runner stand in for the instance.
pub type MockCommandHandler =
    Arc<dyn Fn(InstanceInfo, RemoteCommand) -> CommandFuture + Send + Sync>;

/// Wraps a synchronous closure as a [`MockCommandHandler`].
pub fn command_handler<F>(f: F) -> MockCommandHandler
where
    F: Fn(&InstanceInfo, &RemoteCommand) -> RemoteCommandResult + Send + Sync + 'static,
{
    Arc::new(move |inst, cmd| {
        let r = f(&inst, &cmd);
        Box::pin(async move { Ok(r) })
    })
}

#[derive(Clone)]
pub struct MockCloudComputeOptions {
    pub clock: Arc<dyn Clock>,
    /// Executes `run_command` scripts. Default: succeed silently.
    pub command_handler: Option<MockCommandHandler>,
    /// Time from launch to `running`. Default 0 (immediately running).
    pub boot_delay_ms: i64,
    pub account_id: String,
    pub agent_role_path: String,
}

impl Default for MockCloudComputeOptions {
    fn default() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            command_handler: None,
            boot_delay_ms: 0,
            account_id: "000000000000".to_string(),
            agent_role_path: DEFAULT_AGENT_ROLE_PATH.to_string(),
        }
    }
}

impl MockCloudComputeOptions {
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            ..Self::default()
        }
    }
}

struct MockIdentity {
    identity: AgentIdentity,
    revoked: bool,
}

struct MockInstance {
    info: InstanceInfo,
    booted_at: i64,
}

#[derive(Default)]
struct State {
    instances: Vec<MockInstance>,
    roles: Vec<MockIdentity>, // keyed by role ARN, insertion ordered
    secrets: BTreeMap<String, String>,
    seq: u64,
    commands: Vec<(String, RemoteCommand)>,
}

pub struct MockCloudCompute {
    opts: MockCloudComputeOptions,
    state: Mutex<State>,
    handler: Mutex<Option<MockCommandHandler>>,
}

impl MockCloudCompute {
    pub fn new(opts: MockCloudComputeOptions) -> Self {
        let handler = opts.command_handler.clone();
        Self {
            opts,
            state: Mutex::new(State::default()),
            handler: Mutex::new(handler),
        }
    }

    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self::new(MockCloudComputeOptions::with_clock(clock))
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("mock state poisoned")
    }

    /// Replace the command handler (TS `commandHandler` is assignable).
    pub fn set_command_handler(&self, handler: Option<MockCommandHandler>) {
        *self.handler.lock().expect("handler poisoned") = handler;
    }

    /// Every `run_command` call so far.
    pub fn commands(&self) -> Vec<(String, RemoteCommand)> {
        self.st().commands.clone()
    }

    /// Simulation helper: an experiment instance an agent launched itself (tags opt+agent only).
    pub fn simulate_experiment(
        &self,
        optimization_id: &str,
        agent_id: &str,
        instance_type: &str,
    ) -> InstanceInfo {
        self.add(
            InstanceRole::Experiment,
            optimization_id,
            Some(agent_id),
            instance_type,
        )
    }

    /// Simulation helper: guest-initiated shutdown (terminates, as `InstanceInitiatedShutdownBehavior=terminate`).
    pub fn simulate_shutdown(&self, instance_id: &str) {
        self.terminate_now(&[instance_id.to_string()]);
    }

    /// Simulation helper: an instance that ended up merely stopped (e.g. launched without terminate-on-shutdown).
    pub fn simulate_stopped(&self, instance_id: &str) {
        let now = self.opts.clock.now();
        let mut st = self.st();
        if let Some(i) = st.instances.iter_mut().find(|i| i.info.id == instance_id)
            && i.info.state != InstanceState::Terminated
        {
            i.info.state = InstanceState::Stopped;
            i.info.terminated_at = Some(now);
        }
    }

    pub fn is_revoked(&self, identity: &AgentIdentity) -> bool {
        self.st()
            .roles
            .iter()
            .find(|r| r.identity.role_ref == identity.role_ref)
            .is_some_and(|r| r.revoked)
    }

    pub fn identities(&self) -> Vec<AgentIdentity> {
        self.st().roles.iter().map(|r| r.identity.clone()).collect()
    }

    pub fn secret_names(&self) -> Vec<String> {
        self.st().secrets.keys().cloned().collect()
    }

    pub fn secret_value(&self, reference: &str) -> Option<String> {
        self.st().secrets.get(reference).cloned()
    }

    fn add(
        &self,
        role: InstanceRole,
        optimization_id: &str,
        agent_id: Option<&str>,
        instance_type: &str,
    ) -> InstanceInfo {
        let now = self.opts.clock.now();
        let mut st = self.st();
        st.seq += 1;
        let id = format!("i-mock{:012x}", st.seq);
        let info = InstanceInfo {
            id,
            role,
            optimization_id: optimization_id.into(),
            agent_id: agent_id.map(AgentId::from),
            instance_type: instance_type.to_string(),
            state: InstanceState::Pending,
            launched_at: now,
            terminated_at: None,
        };
        st.instances.push(MockInstance {
            info,
            booted_at: now + self.opts.boot_delay_ms,
        });
        let idx = st.instances.len() - 1;
        self.view(&mut st.instances[idx])
    }

    fn terminate_now(&self, ids: &[String]) {
        let now = self.opts.clock.now();
        let mut st = self.st();
        for id in ids {
            if let Some(i) = st.instances.iter_mut().find(|i| &i.info.id == id) {
                if i.info.state == InstanceState::Terminated {
                    continue; // idempotent
                }
                i.info.state = InstanceState::Terminated;
                i.info.terminated_at = Some(now);
            }
        }
    }

    fn view(&self, i: &mut MockInstance) -> InstanceInfo {
        if i.info.state == InstanceState::Pending && self.opts.clock.now() >= i.booted_at {
            i.info.state = InstanceState::Running;
        }
        i.info.clone()
    }
}

impl Default for MockCloudCompute {
    fn default() -> Self {
        Self::new(MockCloudComputeOptions::default())
    }
}

#[async_trait]
impl CloudCompute for MockCloudCompute {
    async fn create_agent_identity(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<AgentIdentity> {
        let (optimization_id, agent_id) = (optimization_id.as_str(), agent_id.as_str());
        assert_valid_id("optimization", optimization_id)?;
        assert_valid_id("agent", agent_id)?;
        let name = agent_role_name(optimization_id, agent_id)?;
        let path = &self.opts.agent_role_path;
        let acct = &self.opts.account_id;
        let identity = AgentIdentity {
            optimization_id: optimization_id.into(),
            agent_id: agent_id.into(),
            role_ref: format!("arn:aws:iam::{acct}:role{path}{name}"),
            instance_profile_ref: format!("arn:aws:iam::{acct}:instance-profile{path}{name}"),
        };
        let mut st = self.st();
        if let Some(existing) = st
            .roles
            .iter()
            .find(|r| r.identity.role_ref == identity.role_ref)
        {
            return Ok(existing.identity.clone()); // idempotent, like the real one
        }
        st.roles.push(MockIdentity {
            identity: identity.clone(),
            revoked: false,
        });
        Ok(identity)
    }

    async fn revoke_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        let mut st = self.st();
        match st
            .roles
            .iter_mut()
            .find(|r| r.identity.role_ref == identity.role_ref)
        {
            Some(r) => {
                r.revoked = true;
                Ok(())
            }
            None => Err(Error::aws("NoSuchEntity", format!("role {}", identity.role_ref)).into()),
        }
    }

    async fn delete_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        self.st()
            .roles
            .retain(|r| r.identity.role_ref != identity.role_ref); // missing is fine (idempotent)
        Ok(())
    }

    async fn put_secret(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
        name: &str,
        value: &str,
    ) -> PortResult<SecretRef> {
        let (optimization_id, agent_id) = (optimization_id.as_str(), agent_id.as_str());
        assert_valid_id("optimization", optimization_id)?;
        assert_valid_id("agent", agent_id)?;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        {
            return Err(Error::invalid(format!("invalid secret name {}", json_str(name))).into());
        }
        let reference = format!("{}{name}", secret_prefix(optimization_id, agent_id)?);
        self.st()
            .secrets
            .insert(reference.clone(), value.to_string());
        Ok(SecretRef {
            name: name.to_string(),
            reference,
        })
    }

    async fn delete_secrets(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<()> {
        let prefix = secret_prefix(optimization_id.as_str(), agent_id.as_str())?;
        self.st().secrets.retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    async fn launch_runner(&self, spec: &RunnerLaunchSpec) -> PortResult<InstanceInfo> {
        assert_valid_id("optimization", spec.optimization_id.as_str())?;
        assert_valid_id("agent", spec.agent_id.as_str())?;
        if !self
            .st()
            .roles
            .iter()
            .any(|r| r.identity.role_ref == spec.identity.role_ref)
        {
            return Err(Error::aws(
                "InvalidParameterValue",
                format!(
                    "Invalid IAM Instance Profile {}",
                    spec.identity.instance_profile_ref
                ),
            )
            .into());
        }
        Ok(self.add(
            InstanceRole::AgentRunner,
            spec.optimization_id.as_str(),
            Some(spec.agent_id.as_str()),
            &spec.instance_type,
        ))
    }

    async fn launch_bench_instance(&self, spec: &BenchLaunchSpec) -> PortResult<InstanceInfo> {
        assert_valid_id("optimization", spec.optimization_id.as_str())?;
        Ok(self.add(
            InstanceRole::Bench,
            spec.optimization_id.as_str(),
            None,
            &spec.instance_type,
        ))
    }

    async fn terminate(&self, instance_ids: &[String]) -> PortResult<()> {
        self.terminate_now(instance_ids);
        Ok(())
    }

    async fn list_tagged(
        &self,
        optimization_id: Option<&OptimizationId>,
    ) -> PortResult<Vec<InstanceInfo>> {
        let mut st = self.st();
        let mut out = vec![];
        for i in st.instances.iter_mut() {
            if optimization_id.is_none_or(|o| o.as_str().is_empty() || i.info.optimization_id == *o)
            {
                out.push(self.view(i));
            }
        }
        Ok(out)
    }

    async fn run_command(
        &self,
        instance_id: &str,
        command: &RemoteCommand,
    ) -> PortResult<RemoteCommandResult> {
        let view = {
            let mut st = self.st();
            let Some(i) = st.instances.iter_mut().find(|i| i.info.id == instance_id) else {
                return Err(Error::aws("InvalidInstanceId", instance_id).into());
            };
            let view = self.view(i);
            if view.state != InstanceState::Running {
                return Err(Error::aws(
                    "InvalidInstanceId",
                    format!(
                        "{instance_id} is {}, not running / not SSM-managed",
                        state_name(view.state)
                    ),
                )
                .into());
            }
            st.commands.push((instance_id.to_string(), command.clone()));
            view
        };
        let handler = self.handler.lock().expect("handler poisoned").clone();
        match handler {
            None => Ok(RemoteCommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }),
            Some(h) => h(view, command.clone()).await,
        }
    }
}

fn state_name(s: InstanceState) -> &'static str {
    match s {
        InstanceState::Pending => "pending",
        InstanceState::Running => "running",
        InstanceState::Stopping => "stopping",
        InstanceState::Stopped => "stopped",
        InstanceState::Terminated => "terminated",
    }
}
