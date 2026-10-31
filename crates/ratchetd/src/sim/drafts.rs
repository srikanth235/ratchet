//! Dynamic environment drafts of the simulation (PROVISIONAL API). The "agent" is a keyword heuristic over the
//! goal; the shapes (resources, cost, checks, revisions, approve and lock) are what the real drafter will return.

use std::collections::HashMap;

use ratchet_core::enums::AgentKind;

use super::Inner;
use crate::api::*;
use crate::service::{ServiceError, ServiceResult};

struct Draft {
    vm: EnvironmentDraftVm,
    files: Vec<EnvFile>,
    rds: bool,
}

#[derive(Default)]
pub struct DraftsState {
    items: HashMap<String, Draft>,
    next: u32,
}

fn invalid(path: &str, message: &str) -> ServiceError {
    ServiceError::InvalidFields {
        message: message.into(),
        fields: vec![FieldError::new(path, message)],
    }
}

fn scope_for(goal: &str) -> EnvScope {
    let g = goal.to_lowercase();
    if [
        "latency",
        "p95",
        "p99",
        "checkout",
        "throughput",
        "query",
        "database",
        "api",
    ]
    .iter()
    .any(|k| g.contains(k))
    {
        EnvScope::Full
    } else if [
        "cold start",
        "startup",
        "bundle",
        "build",
        "test",
        "memory",
        "size",
    ]
    .iter()
    .any(|k| g.contains(k))
    {
        EnvScope::Single
    } else {
        EnvScope::None
    }
}

fn res(id: &str, name: &str, description: &str, machine: &str, hourly_usd: f64) -> EnvResource {
    EnvResource {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        machine: machine.into(),
        hourly_usd,
    }
}

fn resources_for(scope: EnvScope, repo: &str, rds: bool) -> Vec<EnvResource> {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    match scope {
        EnvScope::None => vec![],
        EnvScope::Single => vec![res(
            "bench",
            "Bench host",
            &format!("{name} in the repo's container, with its load generator"),
            "c7i.2xlarge",
            0.36,
        )],
        EnvScope::Full => vec![
            res(
                "api",
                "API",
                &format!("{name} in the repo's container"),
                "c7i.2xlarge",
                0.36,
            ),
            if rds {
                res(
                    "db",
                    "RDS Postgres 16",
                    "2.1 GB of anonymised orders",
                    "db.r6g.large",
                    0.26,
                )
            } else {
                res(
                    "db",
                    "Postgres 16",
                    "2.1 GB of anonymised orders",
                    "r7g.large",
                    0.11,
                )
            },
            res(
                "cache",
                "Redis 7",
                "emptied before each measurement",
                "t4g.medium",
                0.03,
            ),
            res(
                "loadgen",
                "Load generator",
                "k6, on its own host",
                "c7i.large",
                0.09,
            ),
        ],
    }
}

fn files_for(scope: EnvScope, repo: &str, rds: bool) -> Vec<EnvFile> {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let f = |path: &str, content: String| EnvFile {
        path: path.into(),
        content,
    };
    match scope {
        EnvScope::None => vec![],
        EnvScope::Single => vec![
            f("main.tf", format!("# {name}: one bench host\nresource \"aws_instance\" \"bench\" {{\n  instance_type = \"c7i.2xlarge\"\n}}\n")),
            f("Dockerfile", format!("FROM node:22-slim\nWORKDIR /app\nCOPY . .\nRUN npm ci\n# {name}\n")),
        ],
        EnvScope::Full => vec![
            f(
                "main.tf",
                format!(
                    "# {name}: API + database + cache + load generator\nresource \"aws_instance\" \"api\" {{ instance_type = \"c7i.2xlarge\" }}\n{}\nresource \"aws_instance\" \"cache\" {{ instance_type = \"t4g.medium\" }}\nresource \"aws_instance\" \"loadgen\" {{ instance_type = \"c7i.large\" }}\n",
                    if rds {
                        "resource \"aws_db_instance\" \"db\" { instance_class = \"db.r6g.large\" }"
                    } else {
                        "resource \"aws_instance\" \"db\" { instance_type = \"r7g.large\" }"
                    }
                ),
            ),
            f("Dockerfile", format!("FROM node:22-slim\nWORKDIR /app\nCOPY . .\nRUN npm ci\n# {name}\n")),
            f("seed.sh", "#!/bin/sh\nset -eu\npsql \"$DATABASE_URL\" < seed/orders.sql\nredis-cli FLUSHALL\n".into()),
        ],
    }
}

fn summary_for(scope: EnvScope) -> (String, String) {
    match scope {
        EnvScope::Full => (
            "This task needs the full stack: the API, Postgres with seeded orders, Redis, and a separate load generator.".into(),
            "p95 depends on database queries, so a single machine wouldn't measure it. Use a smaller setup to try one machine.".into(),
        ),
        EnvScope::Single => (
            "This task needs one machine: the code runs in the repo's container next to its load generator.".into(),
            "Nothing in the measured path talks to another service, so one host measures it. Ask for more if the metric depends on a database.".into(),
        ),
        EnvScope::None => (
            "This task needs no extra environment: the evaluator runs on the bench host as it is.".into(),
            "The evaluator is self-contained, so nothing has to be created besides the bench host.".into(),
        ),
    }
}

fn checks() -> Vec<EnvCheck> {
    [
        (
            "isolated",
            "Isolated",
            "no route to anything outside the sandbox except the allowed hosts",
        ),
        (
            "sealed",
            "Results sealed",
            "the holdout and the verdicts are out of the agents' reach",
        ),
        (
            "hardware",
            "Same hardware every time",
            "machines are pinned by type",
        ),
        (
            "teardown",
            "Full teardown",
            "every resource is tagged and removed with the sandbox",
        ),
    ]
    .iter()
    .map(|(id, label, detail)| EnvCheck {
        id: (*id).into(),
        label: (*label).into(),
        state: CheckState::Ok,
        detail: (*detail).into(),
    })
    .collect()
}

fn cost_for(resources: &[EnvResource]) -> EnvCost {
    let hourly: f64 = resources.iter().map(|r| r.hourly_usd).sum();
    let hourly = (hourly * 100.0).round() / 100.0;
    let run_hours = 2.0;
    EnvCost {
        hourly_usd: hourly,
        ready_minutes: match resources.len() {
            0 => 0,
            1 => 3,
            _ => 6,
        },
        run_hours,
        estimated_run_usd: (hourly * run_hours * 100.0).round() / 100.0,
    }
}

fn rebuild(d: &mut Draft, scope: EnvScope) {
    let (summary, rationale) = summary_for(scope);
    d.vm.scope = scope;
    d.vm.summary = summary;
    d.vm.rationale = rationale;
    d.vm.resources = resources_for(scope, &d.vm.repo, d.rds);
    d.files = files_for(scope, &d.vm.repo, d.rds);
    d.vm.file_paths = d.files.iter().map(|f| f.path.clone()).collect();
    d.vm.cost = cost_for(&d.vm.resources);
    d.vm.checks = checks();
    d.vm.checks_passed = d.vm.checks.iter().all(|c| c.state == CheckState::Ok);
}

impl Inner {
    pub(crate) fn create_draft(
        &self,
        input: CreateEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        let repo =
            super::repo_slug(&input.repo).ok_or_else(|| invalid("repo", "use owner/name"))?;
        let goal = input.goal.trim().to_owned();
        if goal.is_empty() || goal.chars().count() > 500 {
            return Err(invalid("goal", "the goal must be 1 to 500 characters"));
        }
        let scope = input.scope.unwrap_or_else(|| scope_for(&goal));
        let now = self.clock.now();
        let mut st = self.st();
        st.drafts.next += 1;
        let id = format!("envd_{}", st.drafts.next);
        let mut d = Draft {
            vm: EnvironmentDraftVm {
                id: id.clone(),
                status: EnvironmentDraftStatus::Ready,
                repo,
                base_ref: input.base_ref,
                goal,
                scope,
                summary: String::new(),
                rationale: String::new(),
                provenance: DraftProvenance {
                    agent: input.agent.unwrap_or(AgentKind::ClaudeCode),
                    duration_s: 120,
                    cost_usd: 0.31,
                },
                resources: vec![],
                file_paths: vec![],
                cost: cost_for(&[]),
                checks: vec![],
                checks_passed: false,
                revision: 1,
                approved_at: None,
                pr_url: None,
                created_at: now,
            },
            files: vec![],
            rds: false,
        };
        rebuild(&mut d, scope);
        let vm = d.vm.clone();
        st.drafts.items.insert(id.clone(), d);
        drop(st);
        self.emit(ApiEvent::EnvironmentDraft {
            draft_id: id,
            status: EnvironmentDraftStatus::Ready,
        });
        Ok(vm)
    }

    pub(crate) fn get_draft(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        self.st()
            .drafts
            .items
            .get(id)
            .map(|d| d.vm.clone())
            .ok_or_else(|| ServiceError::not_found("environment draft", id))
    }

    pub(crate) fn draft_files(&self, id: &str) -> ServiceResult<EnvFilesVm> {
        self.st()
            .drafts
            .items
            .get(id)
            .map(|d| EnvFilesVm {
                files: d.files.clone(),
            })
            .ok_or_else(|| ServiceError::not_found("environment draft", id))
    }

    pub(crate) fn revise_draft(
        &self,
        id: &str,
        input: ReviseEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        let request = input.request.trim().to_lowercase();
        if request.is_empty() || request.chars().count() > 1000 {
            return Err(invalid(
                "request",
                "the request must be 1 to 1000 characters",
            ));
        }
        let mut st = self.st();
        let d = st
            .drafts
            .items
            .get_mut(id)
            .ok_or_else(|| ServiceError::not_found("environment draft", id))?;
        if d.vm.status == EnvironmentDraftStatus::Approved {
            return Err(ServiceError::Conflict(
                "the environment is approved and locked; start a new draft to change it".into(),
            ));
        }
        let scope = input.scope.unwrap_or(
            if request.contains("smaller") || request.contains("single") {
                EnvScope::Single
            } else if request.contains("full stack") {
                EnvScope::Full
            } else {
                d.vm.scope
            },
        );
        if request.contains("rds") {
            d.rds = true;
        }
        rebuild(d, scope);
        d.vm.revision += 1;
        d.vm.status = EnvironmentDraftStatus::Ready;
        let vm = d.vm.clone();
        drop(st);
        self.emit(ApiEvent::EnvironmentDraft {
            draft_id: id.to_owned(),
            status: EnvironmentDraftStatus::Revising,
        });
        self.emit(ApiEvent::EnvironmentDraft {
            draft_id: id.to_owned(),
            status: EnvironmentDraftStatus::Ready,
        });
        Ok(vm)
    }

    pub(crate) fn approve_draft(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        let now = self.clock.now();
        let mut st = self.st();
        let d = st
            .drafts
            .items
            .get_mut(id)
            .ok_or_else(|| ServiceError::not_found("environment draft", id))?;
        if d.vm.status == EnvironmentDraftStatus::Approved {
            return Ok(d.vm.clone());
        }
        if d.vm.status != EnvironmentDraftStatus::Ready || !d.vm.checks_passed {
            return Err(ServiceError::Conflict(
                "the draft is not ready: it must pass Ratchet's checks before it can be approved"
                    .into(),
            ));
        }
        d.vm.status = EnvironmentDraftStatus::Approved;
        d.vm.approved_at = Some(now);
        let vm = d.vm.clone();
        drop(st);
        self.emit(ApiEvent::EnvironmentDraft {
            draft_id: id.to_owned(),
            status: EnvironmentDraftStatus::Approved,
        });
        Ok(vm)
    }

    pub(crate) fn open_draft_pr(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        let n = 300 + (self.rng().next_f64() * 600.0) as u32;
        let mut st = self.st();
        let d = st
            .drafts
            .items
            .get_mut(id)
            .ok_or_else(|| ServiceError::not_found("environment draft", id))?;
        if d.files.is_empty() {
            return Err(ServiceError::Conflict(
                "this environment adds no files to the repo".into(),
            ));
        }
        if d.vm.pr_url.is_none() {
            d.vm.pr_url = Some(format!("https://github.com/{}/pull/{n}", d.vm.repo));
        }
        Ok(d.vm.clone())
    }

    pub(crate) fn require_approved_draft(&self, id: &str) -> ServiceResult<()> {
        match self.st().drafts.items.get(id).map(|d| d.vm.status) {
            None => Err(ServiceError::not_found("environment draft", id)),
            Some(EnvironmentDraftStatus::Approved) => Ok(()),
            Some(_) => Err(ServiceError::NotReady(
                "approve the environment before starting the optimization".into(),
            )),
        }
    }

    /// Drafts waiting for the user's approval.
    pub(crate) fn drafts_waiting(&self) -> Vec<EnvironmentDraftVm> {
        let st = self.st();
        let mut v: Vec<_> = st
            .drafts
            .items
            .values()
            .filter(|d| d.vm.status == EnvironmentDraftStatus::Ready)
            .map(|d| d.vm.clone())
            .collect();
        v.sort_by_key(|d| d.created_at);
        v
    }
}
