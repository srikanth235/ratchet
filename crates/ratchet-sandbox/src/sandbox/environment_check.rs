//! "Plans cleanly in your AWS account": `tofu validate` + `tofu plan` on a drafted module. (TS
//! `sandbox/environment-check.ts`)
//!
//! Materializes the draft exactly as the sandbox provider would, then runs `tofu init`, `tofu validate` and
//! `tofu plan` (no refresh, no lock, nothing created) with the user's AWS profile. The plan reads the bench image by
//! tag in the target region, so a missing image or bad credentials surface here, before the user approves, instead of
//! at run start.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use ratchet_core::ports::{
    PortError, PortResult, SandboxEnvironment, SandboxEnvironmentFile, SandboxEnvironmentKind,
};
use ratchet_env::{
    EnvironmentChecker, EnvironmentPlanInput, EnvironmentPlanOutcome, hash_environment_files,
};
use serde_json::json;

use super::environment::materialize_drafted_environment;
use super::process::{ProcessRunner, ProcessSpec, TokioProcessRunner};
use crate::error::Error;

pub struct TofuEnvironmentCheckerOptions {
    pub tofu_binary: PathBuf,
    /// `infra/aws`: the base module directory is `../modules/sandbox` next to it unless `base_module_dir` says otherwise.
    pub module_dir: PathBuf,
    pub base_module_dir: Option<PathBuf>,
    /// Where scratch directories go (default: the OS temp dir).
    pub work_root: Option<PathBuf>,
    pub runner: Arc<dyn ProcessRunner>,
    /// Extra environment for tofu (AWS_PROFILE and the like are set on top).
    pub env: BTreeMap<String, String>,
    /// The bench image the plan looks up by tag (the app version), or an explicit AMI id.
    pub image_version: Option<String>,
    pub machine_image_id: Option<String>,
}

impl TofuEnvironmentCheckerOptions {
    pub fn new(tofu_binary: impl Into<PathBuf>, module_dir: impl Into<PathBuf>) -> Self {
        Self {
            tofu_binary: tofu_binary.into(),
            module_dir: module_dir.into(),
            base_module_dir: None,
            work_root: None,
            runner: Arc::new(TokioProcessRunner),
            env: BTreeMap::new(),
            image_version: None,
            machine_image_id: None,
        }
    }
}

pub struct TofuEnvironmentChecker {
    o: TofuEnvironmentCheckerOptions,
}

const COMMON: [&str; 2] = ["-input=false", "-no-color"];
const PREFIX: &str = "infra/ratchet/";

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.trim().split('\n').collect();
    let last = &lines[lines.len().saturating_sub(12)..];
    let joined = last.join(" ");
    let n = joined.chars().count();
    joined.chars().skip(n.saturating_sub(600)).collect()
}

impl TofuEnvironmentChecker {
    pub fn new(o: TofuEnvironmentCheckerOptions) -> Self {
        Self { o }
    }

    async fn run(
        &self,
        cwd: &std::path::Path,
        env: &BTreeMap<String, String>,
        args: Vec<String>,
        ok_codes: &[i32],
    ) -> PortResult<(bool, super::process::ProcessResult)> {
        let spec = ProcessSpec {
            command: self.o.tofu_binary.to_string_lossy().into_owned(),
            args,
            cwd: cwd.to_path_buf(),
            env: env.clone(),
        };
        let r = self.o.runner.run(&spec).await?;
        Ok((ok_codes.contains(&r.exit_code), r))
    }

    async fn check(&self, input: &EnvironmentPlanInput) -> PortResult<EnvironmentPlanOutcome> {
        let scratch = self.o.work_root.clone().unwrap_or_else(std::env::temp_dir);
        tokio::fs::create_dir_all(&scratch).await?;
        let root = tempfile::Builder::new()
            .prefix("ratchet-plan-check-")
            .tempdir_in(&scratch)?;
        let result = self.check_in(root.path(), input).await;
        let _ = root.close();
        result
    }

    async fn check_in(
        &self,
        root: &std::path::Path,
        input: &EnvironmentPlanInput,
    ) -> PortResult<EnvironmentPlanOutcome> {
        let rel: Vec<SandboxEnvironmentFile> = input
            .files
            .iter()
            .map(|f| SandboxEnvironmentFile {
                path: f.path.strip_prefix(PREFIX).unwrap_or(&f.path).to_owned(),
                content: f.content.clone(),
            })
            .collect();
        let hash =
            hash_environment_files(rel.iter().map(|f| (f.path.as_str(), f.content.as_str())));
        let env = SandboxEnvironment {
            kind: SandboxEnvironmentKind::Drafted,
            scope: input.scope,
            path: "infra/ratchet".into(),
            commit: "draft".into(),
            hash: hash.clone(),
            files: rel,
        };
        let dest = root.join("module");
        let base = self
            .o
            .base_module_dir
            .clone()
            .unwrap_or_else(|| super::tofu::default_base_module_dir(&self.o.module_dir));
        match materialize_drafted_environment(&env, &base, &dest).await {
            Ok(_) => {}
            Err(Error::EnvironmentRejected(e)) => {
                return Ok(EnvironmentPlanOutcome {
                    ok: false,
                    detail: e.problems.join("; "),
                });
            }
            Err(e) => return Err(e.into()),
        }
        let vars = json!({ "ratchet": {
            "sandbox_id": "plan-check",
            "region": input.region,
            "vpc_id": "vpc-0plancheck000000",
            "bench_subnet_id": "subnet-0plancheck00000",
            "bench_security_group_id": "sg-0plancheck000000",
            "artifacts_bucket": "ratchet-plan-check-artifacts",
            "eval_instance_profile_name": "ratchet-plan-check-bench-runner",
            "environment_hash": hash,
            "machine_image_id": self.o.machine_image_id.clone().unwrap_or_default(),
            "image_version": self.o.image_version.clone().unwrap_or_default(),
            "tags": {},
        }});
        let vars_path = root.join("vars.tfvars.json");
        tokio::fs::write(&vars_path, serde_json::to_string(&vars)?).await?;
        let mut tf_env = self.o.env.clone();
        if let Some(p) = &input.aws_profile {
            if !p.is_empty() {
                tf_env.insert("AWS_PROFILE".into(), p.clone());
            }
        }
        tf_env.insert("AWS_REGION".into(), input.region.clone());
        tf_env.insert(
            "TF_DATA_DIR".into(),
            root.join(".terraform-data").to_string_lossy().into_owned(),
        );
        tf_env.insert("TF_IN_AUTOMATION".into(), "1".into());
        tf_env.insert("TF_INPUT".into(), "0".into());
        tf_env.insert("CHECKPOINT_DISABLE".into(), "1".into());
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();

        let (ok, init) = self
            .run(
                &dest,
                &tf_env,
                a(&["init", COMMON[0], COMMON[1], "-backend=false"]),
                &[0],
            )
            .await?;
        if !ok {
            return Ok(fail(format!(
                "tofu init failed: {}",
                tail(pick(&init.stderr, &init.stdout))
            )));
        }
        let (ok, validate) = self
            .run(&dest, &tf_env, a(&["validate", "-no-color"]), &[0])
            .await?;
        if !ok {
            return Ok(fail(format!(
                "tofu validate failed: {}",
                tail(pick(&validate.stderr, &validate.stdout))
            )));
        }
        let mut plan_args = a(&[
            "plan",
            COMMON[0],
            COMMON[1],
            "-refresh=false",
            "-lock=false",
            "-detailed-exitcode",
        ]);
        plan_args.push(format!("-var-file={}", vars_path.display()));
        let (ok, plan) = self.run(&dest, &tf_env, plan_args, &[0, 2]).await?;
        if !ok {
            let who = match &input.aws_profile {
                Some(p) if !p.is_empty() => format!("profile {p}"),
                _ => "your AWS account".to_owned(),
            };
            return Ok(fail(format!(
                "tofu plan failed in {who}: {}",
                tail(pick(&plan.stderr, &plan.stdout))
            )));
        }
        static PLAN_ADDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"Plan: (\d+) to add").expect("static regex")
        });
        let adds = PLAN_ADDS
            .captures(&plan.stdout)
            .map(|c| format!(" ({} resources to add)", &c[1]))
            .unwrap_or_default();
        Ok(EnvironmentPlanOutcome {
            ok: true,
            detail: format!("tofu validate and plan succeeded{adds} in {}", input.region),
        })
    }
}

/// `stderr || stdout` (JavaScript `||`: the first one that is not empty).
fn pick<'a>(stderr: &'a str, stdout: &'a str) -> &'a str {
    if stderr.is_empty() { stdout } else { stderr }
}

fn fail(detail: String) -> EnvironmentPlanOutcome {
    EnvironmentPlanOutcome { ok: false, detail }
}

#[async_trait]
impl EnvironmentChecker for TofuEnvironmentChecker {
    async fn plan(&self, input: &EnvironmentPlanInput) -> PortResult<EnvironmentPlanOutcome> {
        self.check(input).await.map_err(PortError::from)
    }
}
