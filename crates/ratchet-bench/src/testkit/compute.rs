//! `CloudCompute` fake whose "bench instance" is a local bash (port of `testkit/compute.ts`): `run_command` writes the
//! generated script to a file and executes it, so the real scripts are exercised. Like SSM, stdout is truncated to 24,000
//! characters.
//!
//! `s3_dir` maps `s3://<bucket>/<key>` to `<s3_dir>/<bucket>/<key>` through an `aws` shim on PATH.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use async_trait::async_trait;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use ratchet_core::ports::{
    AgentIdentity, BenchLaunchPurpose, BenchLaunchSpec, CloudCompute, InstanceInfo, InstanceState, PortError,
    PortResult, RemoteCommand, RemoteCommandResult, RunnerLaunchSpec, SecretRef,
};
use ratchet_core::{AgentId, InstanceRole, OptimizationId};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct LocalFakeCompute {
    scratch: PathBuf,
    shim: PathBuf,
    extra_env: BTreeMap<String, String>,
    launched: Mutex<Vec<BenchLaunchSpec>>,
    terminated: Mutex<Vec<String>>,
    commands: Mutex<Vec<(String, String)>>,
    timeline: Mutex<Vec<String>>,
    fail_terminate: Mutex<bool>,
    not_ready_for: AtomicU32,
    fail_launch: Mutex<Option<(BenchLaunchPurpose, String)>>,
    children: Mutex<HashMap<u32, String>>,
    counter: AtomicUsize,
}

impl LocalFakeCompute {
    pub fn new(scratch: impl Into<PathBuf>, extra_env: &[(&str, &str)], s3_dir: Option<&Path>) -> Self {
        let scratch = scratch.into();
        let shim = scratch.join("shim");
        fs::create_dir_all(&shim).expect("create the shim dir");
        let me = LocalFakeCompute {
            scratch,
            shim,
            extra_env: extra_env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            launched: Mutex::new(Vec::new()),
            terminated: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            timeline: Mutex::new(Vec::new()),
            fail_terminate: Mutex::new(false),
            not_ready_for: AtomicU32::new(0),
            fail_launch: Mutex::new(None),
            children: Mutex::new(HashMap::new()),
            counter: AtomicUsize::new(0),
        };
        if let Some(s3) = s3_dir {
            let s3 = s3.to_string_lossy();
            me.add_shim(
                "aws",
                &format!(
                    "# aws s3 cp [--only-show-errors] src dst: one side is s3://bucket/key (download or upload)\n\
args=(); for a in \"$@\"; do case \"$a\" in --*) ;; *) args+=(\"$a\");; esac; done\n\
src=\"${{args[2]}}\"; dst=\"${{args[3]}}\"\n\
if [[ \"$dst\" == s3://* ]]; then mkdir -p \"$(dirname \"{s3}/${{dst#s3://}}\")\"; cp -- \"$src\" \"{s3}/${{dst#s3://}}\"; else cp -- \"{s3}/${{src#s3://}}\" \"$dst\"; fi"
                ),
            );
        }
        me
    }

    /// Install an executable shim ahead of the real binary on the fake instance's PATH.
    pub fn add_shim(&self, name: &str, body: &str) {
        let p = self.shim.join(name);
        fs::write(&p, format!("#!/bin/bash\n{body}\n")).expect("write the shim");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod the shim");
    }

    /// Absolute path of a shim installed with `add_shim` (for options that take a path, like the candidate launcher).
    pub fn shim_path(&self, name: &str) -> String {
        self.shim.join(name).to_string_lossy().into_owned()
    }

    pub fn launched(&self) -> Vec<BenchLaunchSpec> {
        lock(&self.launched).clone()
    }

    pub fn terminated(&self) -> Vec<String> {
        lock(&self.terminated).clone()
    }

    /// `(instance id, script)` of every command that was run.
    pub fn commands(&self) -> Vec<(String, String)> {
        lock(&self.commands).clone()
    }

    pub fn scripts(&self) -> Vec<String> {
        lock(&self.commands).iter().map(|(_, s)| s.clone()).collect()
    }

    pub fn clear_commands(&self) {
        lock(&self.commands).clear();
    }

    /// `launch:<purpose>` / `terminate:<id>` in the order they happened.
    pub fn timeline(&self) -> Vec<String> {
        lock(&self.timeline).clone()
    }

    pub fn clear_timeline(&self) {
        lock(&self.timeline).clear();
    }

    pub fn set_fail_terminate(&self, v: bool) {
        *lock(&self.fail_terminate) = v;
    }

    /// Make `run_command` fail this many times before working (instance not yet registered with SSM).
    pub fn set_not_ready_for(&self, n: u32) {
        self.not_ready_for.store(n, Ordering::SeqCst);
    }

    /// Make launching an instance with this purpose fail with this message.
    pub fn fail_launch(&self, purpose: BenchLaunchPurpose, message: &str) {
        *lock(&self.fail_launch) = Some((purpose, message.to_owned()));
    }

    /// Terminating an instance kills everything on it, including processes that left the process group (`timeout` does).
    fn kill_tree(pid: u32) {
        let mut parent_of: HashMap<u32, u32> = HashMap::new();
        if let Ok(rd) = fs::read_dir("/proc") {
            for e in rd.filter_map(Result::ok) {
                let Ok(p) = e.file_name().to_string_lossy().parse::<u32>() else {
                    continue;
                };
                if let Ok(stat) = fs::read_to_string(format!("/proc/{p}/stat"))
                    && let Some(i) = stat.rfind(')')
                    && let Some(ppid) = stat[i + 2..].split(' ').nth(1).and_then(|s| s.parse::<u32>().ok())
                {
                    parent_of.insert(p, ppid);
                }
            }
        }
        fn collect(pid: u32, parent_of: &HashMap<u32, u32>, out: &mut Vec<u32>) {
            out.push(pid);
            for (child, _) in parent_of.iter().filter(|(_, pp)| **pp == pid) {
                collect(*child, parent_of, out);
            }
        }
        let mut all = Vec::new();
        collect(pid, &parent_of, &mut all);
        for p in all.into_iter().rev() {
            if let Ok(raw) = i32::try_from(p) {
                let _ = kill(Pid::from_raw(raw), Signal::SIGKILL);
            }
        }
    }
}

#[async_trait]
impl CloudCompute for LocalFakeCompute {
    async fn launch_bench_instance(&self, spec: &BenchLaunchSpec) -> PortResult<InstanceInfo> {
        let purpose = match spec.purpose {
            BenchLaunchPurpose::Baseline => "baseline",
            BenchLaunchPurpose::Bench => "bench",
            BenchLaunchPurpose::Confirm => "confirm",
            BenchLaunchPurpose::Deps => "deps",
        };
        lock(&self.timeline).push(format!("launch:{purpose}"));
        if let Some((p, msg)) = lock(&self.fail_launch).clone()
            && p == spec.purpose
        {
            return Err(PortError::failed(msg));
        }
        lock(&self.launched).push(spec.clone());
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(InstanceInfo {
            id: format!("i-fake{n}"),
            role: InstanceRole::Bench,
            optimization_id: spec.optimization_id.clone(),
            agent_id: None,
            instance_type: spec.instance_type.clone(),
            state: InstanceState::Running,
            launched_at: 0,
            terminated_at: None,
        })
    }

    async fn terminate(&self, ids: &[String]) -> PortResult<()> {
        if *lock(&self.fail_terminate) {
            return Err(PortError::failed("terminate failed"));
        }
        lock(&self.terminated).extend(ids.iter().cloned());
        lock(&self.timeline).extend(ids.iter().map(|i| format!("terminate:{i}")));
        // Only what runs on the terminated instances (concurrent jobs have instances of their own).
        let pids: Vec<u32> = lock(&self.children)
            .iter()
            .filter(|(_, inst)| ids.contains(inst))
            .map(|(pid, _)| *pid)
            .collect();
        for pid in pids {
            Self::kill_tree(pid);
        }
        Ok(())
    }

    async fn run_command(&self, instance_id: &str, command: &RemoteCommand) -> PortResult<RemoteCommandResult> {
        if self.not_ready_for.load(Ordering::SeqCst) > 0 {
            self.not_ready_for.fetch_sub(1, Ordering::SeqCst);
            return Err(PortError::failed("InvalidInstanceId: instance not registered"));
        }
        let index = {
            let mut c = lock(&self.commands);
            c.push((instance_id.to_owned(), command.script.clone()));
            c.len()
        };
        let file = self.scratch.join(format!("script-{index}.sh"));
        fs::write(&file, &command.script).map_err(|e| PortError::failed(e.to_string()))?;
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg(&file)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.shim.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("RATCHET_FAKE_INSTANCE", instance_id);
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        if let Some(env) = &command.env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }
        let child = cmd.spawn().map_err(|e| PortError::failed(e.to_string()))?;
        let pid = child.id();
        if let Some(pid) = pid {
            lock(&self.children).insert(pid, instance_id.to_owned());
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| PortError::failed(e.to_string()))?;
        if let Some(pid) = pid {
            lock(&self.children).remove(&pid);
        }
        let cut = |b: &[u8]| -> String { String::from_utf8_lossy(b).chars().take(24000).collect() };
        Ok(RemoteCommandResult {
            exit_code: out.status.code().unwrap_or(137),
            stdout: cut(&out.stdout),
            stderr: cut(&out.stderr),
        })
    }

    // ---- unused by the bench runner
    async fn create_agent_identity(&self, _: &OptimizationId, _: &AgentId) -> PortResult<AgentIdentity> {
        Err(PortError::failed("not implemented"))
    }

    async fn revoke_agent_identity(&self, _: &AgentIdentity) -> PortResult<()> {
        Ok(())
    }

    async fn delete_agent_identity(&self, _: &AgentIdentity) -> PortResult<()> {
        Ok(())
    }

    async fn put_secret(&self, _: &OptimizationId, _: &AgentId, _: &str, _: &str) -> PortResult<SecretRef> {
        Err(PortError::failed("not implemented"))
    }

    async fn delete_secrets(&self, _: &OptimizationId, _: &AgentId) -> PortResult<()> {
        Ok(())
    }

    async fn launch_runner(&self, _: &RunnerLaunchSpec) -> PortResult<InstanceInfo> {
        Err(PortError::failed("not implemented"))
    }

    async fn list_tagged(&self, _: Option<&OptimizationId>) -> PortResult<Vec<InstanceInfo>> {
        Ok(Vec::new())
    }
}
