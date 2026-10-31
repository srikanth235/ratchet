//! The commands that talk to the daemon.

use std::time::Duration;

use ratchet_core::enums::{AgentKind, AgentsWhere};
use ratchet_core::numfmt::to_fixed;
use ratchet_core::ports::{CostReconcileReport, CostReconcileStatus};
use ratchetd::api::*;
use serde_json::json;

use super::{emit_json, table, urlencode};
use crate::cli::{
    AgentArg, AgentCmd, Command, CostCmd, DemoArg, EnvCmd, EventsArgs, RunArgs, WhereArg,
};
use crate::client::{Client, ClientError};
use crate::io::Io;

type R = Result<i32, ClientError>;

const DASH: &str = "-";

pub async fn run(c: &Client, cmd: Command, json: bool, io: &dyn Io) -> R {
    match cmd {
        Command::Status => status(c, json, io).await,
        Command::List { range } => list(c, range.as_str(), json, io).await,
        Command::Show { id } => show(c, &id, json, io).await,
        Command::Run(args) => run_optimization(c, args, json, io).await,
        Command::Stop { id, reason } => {
            let body = StopOptimizationInput { reason };
            c.post_unit(&format!("/v1/optimizations/{id}/stop"), Some(&body))
                .await?;
            done(
                io,
                json,
                json!({"id": id, "stopped": true}),
                format!("stopped {id}\n"),
            )
        }
        Command::Report { id, out } => {
            let r: ReportVm = c.get(&format!("/v1/optimizations/{id}/report")).await?;
            match out {
                Some(path) => {
                    if let Err(e) = std::fs::write(&path, &r.markdown) {
                        io.err(&format!("ratchet: cannot write {}: {e}\n", path.display()));
                        return Ok(1);
                    }
                    done(
                        io,
                        json,
                        json!({"fileName": r.file_name, "path": path.display().to_string()}),
                        format!("wrote {}\n", path.display()),
                    )
                }
                None if json => {
                    emit_json(io, &r);
                    Ok(0)
                }
                None => {
                    io.out(&r.markdown);
                    Ok(0)
                }
            }
        }
        Command::Pr { id } => {
            let pr: PullRequestVm = c
                .post_empty(&format!("/v1/optimizations/{id}/pull-request"))
                .await?;
            done(
                io,
                json,
                serde_json::to_value(&pr).unwrap_or_default(),
                format!("{}\n", pr.url),
            )
        }
        Command::Candidates { id } => candidates(c, &id, json, io).await,
        Command::History { id, metric } => history(c, &id, metric, json, io).await,
        Command::Spend { id } => spend(c, &id, json, io).await,
        Command::Activity {
            optimization,
            limit,
        } => activity(c, optimization, limit, json, io).await,
        Command::Agent { action } => agent(c, action, json, io).await,
        Command::Events(args) => events(c, args, json, io).await,
        Command::Env { action } => env(c, action, json, io).await,
        Command::Cost {
            action: CostCmd::Reconcile { id },
        } => {
            let r: CostReconcileReport = c
                .post_empty(&format!("/v1/optimizations/{id}/spend/reconciliations"))
                .await?;
            if json {
                emit_json(io, &r);
            } else {
                io.out(&format_reconcile(&r));
            }
            Ok(0)
        }
        Command::Repos { query } => repos(c, query, json, io).await,
        Command::Inspect { target } => inspect(c, &target, json, io).await,
        Command::Overview { range } => overview(c, range.as_str(), json, io).await,
        Command::Connectors => connectors(c, json, io).await,
        Command::Validate { .. } | Command::Init(_) | Command::Baseline(_) | Command::Templates => {
            unreachable!("offline commands are dispatched before the client is built")
        }
    }
}

fn done(io: &dyn Io, json: bool, value: serde_json::Value, human: String) -> R {
    if json {
        emit_json(io, &value);
    } else {
        io.out(&human);
    }
    Ok(0)
}

// ---- formatting helpers ----------------------------------------------------------------------------------------

fn num(v: Option<f64>) -> String {
    v.map_or(DASH.into(), |v| {
        ratchet_core::numfmt::precision_trimmed(v, 6)
    })
}

fn money(v: f64) -> String {
    format!("${}", to_fixed(v, 2))
}

fn result_text(o: &OptimizationVm) -> String {
    match (o.baseline, o.best) {
        (Some(b), Some(best)) if o.verified_wins > 0 => {
            let unit = if o.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", o.unit)
            };
            format!(
                "{} -> {}{unit}{}",
                num(Some(b)),
                num(Some(best)),
                o.gain_pct
                    .map_or(String::new(), |g| format!(" ({}%)", to_fixed(g, 1)))
            )
        }
        (Some(b), _) => format!("{} (no win yet)", num(Some(b))),
        _ => DASH.into(),
    }
}

fn status_word(s: UiStatus) -> &'static str {
    match s {
        UiStatus::Draft => "draft",
        UiStatus::Baselining => "baselining",
        UiStatus::Running => "running",
        UiStatus::Stopping => "stopping",
        UiStatus::Completed => "completed",
        UiStatus::Stopped => "stopped",
        UiStatus::Failed => "failed",
        UiStatus::PrOpen => "pr-open",
    }
}

fn optimizations_table(list: &[OptimizationVm]) -> String {
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|o| {
            vec![
                o.id.clone(),
                o.name.clone(),
                status_word(o.status).to_owned(),
                result_text(o),
                format!("{} / {}", money(o.spend_usd), money(o.budget_usd)),
                format!("{}/{}", o.verified_wins, o.candidate_count),
            ]
        })
        .collect();
    table(&["ID", "NAME", "STATUS", "RESULT", "SPENT", "WINS"], &rows)
}

fn clock(ms: i64) -> String {
    let s = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

// ---- reads -----------------------------------------------------------------------------------------------------

async fn status(c: &Client, json: bool, io: &dyn Io) -> R {
    let st: DaemonStatus = c.get("/v1/status").await?;
    let snap: OptimizationsSnapshot = c.get("/v1/optimizations").await?;
    if json {
        emit_json(
            io,
            &json!({"daemon": st, "optimizations": snap.optimizations}),
        );
        return Ok(0);
    }
    let engine = match st.engine.kind {
        EngineKind::Sim => "simulation engine",
        EngineKind::Real => "engine",
    };
    io.out(&format!(
        "ratchetd {} ({engine}, {} mode), pid {}, up {}s\nsocket: {}\nledger: {}\n\n",
        st.version,
        st.engine.mode,
        st.pid,
        st.uptime_s,
        st.socket_path.as_deref().unwrap_or(DASH),
        st.ledger_path.as_deref().unwrap_or("in memory"),
    ));
    let running = snap
        .optimizations
        .iter()
        .filter(|o| matches!(o.status, UiStatus::Running | UiStatus::Baselining))
        .count();
    io.out(&format!(
        "{} optimization{}, {running} running\n",
        snap.optimizations.len(),
        if snap.optimizations.len() == 1 {
            ""
        } else {
            "s"
        }
    ));
    if !snap.optimizations.is_empty() {
        io.out(&optimizations_table(&snap.optimizations));
    }
    Ok(0)
}

async fn list(c: &Client, range: &str, json: bool, io: &dyn Io) -> R {
    let snap: OptimizationsSnapshot = c.get(&format!("/v1/optimizations?range={range}")).await?;
    if json {
        emit_json(io, &snap);
        return Ok(0);
    }
    io.out(&optimizations_table(&snap.optimizations));
    for k in &snap.kpis {
        io.out(&format!(
            "{}: {}{} ({})\n",
            k.label,
            k.value,
            k.unit
                .as_deref()
                .filter(|u| !u.is_empty())
                .map_or(String::new(), |u| format!(" {u}")),
            k.change
                .as_ref()
                .map_or(k.foot.clone(), |ch| format!("{}, {}", ch.text, k.foot)),
        ));
    }
    Ok(0)
}

async fn show(c: &Client, id: &str, json: bool, io: &dyn Io) -> R {
    let d: OptimizationDetail = c.get(&format!("/v1/optimizations/{id}")).await?;
    if json {
        emit_json(io, &d);
        return Ok(0);
    }
    let s = &d.summary;
    io.out(&format!(
        "{}  ({})\n  repo        {} ({})\n  status      {}\n  metric      {} {}\n  result      {}\n  spend       {} of {}\n  candidates  {} ({} verified)\n",
        s.name,
        s.id,
        s.repo,
        d.base_ref,
        status_word(s.status),
        match s.direction {
            Better::Lower => "lower is better:",
            Better::Higher => "higher is better:",
        },
        s.metric,
        result_text(s),
        money(s.spend_usd),
        money(s.budget_usd),
        s.candidate_count,
        s.verified_wins,
    ));
    if let Some(v) = &s.value {
        io.out(&format!(
            "  value       ${}/yr ({}){}\n",
            to_fixed(v.usd_per_year, 0),
            v.basis,
            v.return_on_spend
                .map_or(String::new(), |r| format!(", {}x return", to_fixed(r, 1)))
        ));
    }
    if let Some(cal) = &d.calibration {
        io.out(&format!("  calibration {cal}\n"));
    }
    if let Some(url) = &d.pr_url {
        io.out(&format!("  pull request {url}\n"));
    }
    if !d.agents.is_empty() {
        io.out("\nAgents\n");
        let rows: Vec<Vec<String>> = d
            .agents
            .iter()
            .map(|a| {
                vec![
                    a.id.clone(),
                    a.kind.to_string(),
                    a.status.to_string(),
                    format!("{}/{}", a.stats.verified, a.stats.candidates),
                    money(a.stats.spend_usd),
                    a.hypothesis.clone().unwrap_or_default(),
                ]
            })
            .collect();
        io.out(&table(
            &["ID", "AGENT", "STATUS", "WINS", "SPENT", "LATEST"],
            &rows,
        ));
    }
    if !d.compare.is_empty() {
        io.out("\nBaseline vs best\n");
        let rows: Vec<Vec<String>> = d
            .compare
            .iter()
            .map(|r| {
                vec![
                    r.key.clone(),
                    num(r.baseline),
                    num(r.best),
                    r.delta_pct
                        .map_or(DASH.into(), |p| format!("{}%", to_fixed(p, 1))),
                ]
            })
            .collect();
        io.out(&table(&["METRIC", "BASELINE", "BEST", "CHANGE"], &rows));
    }
    if let Some(r) = &d.receipt {
        io.out(&format!("\nEnvironment: {}\n", r.summary));
    }
    Ok(0)
}

async fn candidates(c: &Client, id: &str, json: bool, io: &dyn Io) -> R {
    let list: Vec<CandidateVm> = c.get(&format!("/v1/optimizations/{id}/candidates")).await?;
    if json {
        emit_json(io, &list);
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|x| {
            vec![
                x.sha.clone(),
                x.agent.to_string(),
                x.verdict.to_string(),
                num(x.bench_value),
                x.vs_baseline_pct
                    .map_or(DASH.into(), |p| format!("{}%", to_fixed(p, 1))),
                x.message.clone(),
            ]
        })
        .collect();
    io.out(&table(
        &["SHA", "AGENT", "VERDICT", "BENCH", "VS BASELINE", "MESSAGE"],
        &rows,
    ));
    Ok(0)
}

async fn history(c: &Client, id: &str, metric: Option<String>, json: bool, io: &dyn Io) -> R {
    let q = metric.map_or(String::new(), |m| format!("?metric={}", urlencode(&m)));
    let s: RatchetSeries = c.get(&format!("/v1/optimizations/{id}/ratchet{q}")).await?;
    if json {
        emit_json(io, &s);
        return Ok(0);
    }
    io.out(&format!(
        "{} ({} is better)\n",
        s.metric,
        match s.direction {
            Better::Lower => "lower",
            Better::Higher => "higher",
        }
    ));
    let rows: Vec<Vec<String>> = s
        .steps
        .iter()
        .enumerate()
        .map(|(i, p)| {
            vec![
                if i == 0 {
                    "baseline".to_owned()
                } else {
                    format!("win {i}")
                },
                format!("{} min", to_fixed(p.t, 0)),
                num(Some(p.value)),
            ]
        })
        .collect();
    io.out(&table(&["STEP", "AT", "VALUE"], &rows));
    Ok(0)
}

async fn spend(c: &Client, id: &str, json: bool, io: &dyn Io) -> R {
    let s: SpendVm = c.get(&format!("/v1/optimizations/{id}/spend")).await?;
    if json {
        emit_json(io, &s);
        return Ok(0);
    }
    io.out(&format!(
        "total {} of {} budget ({} estimated, {} reconciled)\n",
        money(s.total),
        money(s.budget_usd),
        money(s.estimated),
        money(s.actual)
    ));
    let rows: Vec<Vec<String>> = s
        .by_category
        .iter()
        .filter(|l| l.usd != 0.0)
        .map(|l| vec![l.label.clone(), money(l.usd)])
        .collect();
    io.out(&table(&["CATEGORY", "USD"], &rows));
    let rows: Vec<Vec<String>> = s
        .by_agent
        .iter()
        .map(|a| {
            vec![
                a.branch.clone().unwrap_or_else(|| "(optimization)".into()),
                a.kind.map_or(DASH.into(), |k| k.to_string()),
                money(a.usd),
            ]
        })
        .collect();
    io.out(&format!(
        "\n{}",
        table(&["AGENT BRANCH", "KIND", "USD"], &rows)
    ));
    Ok(0)
}

async fn activity(c: &Client, opt: Option<String>, limit: u32, json: bool, io: &dyn Io) -> R {
    let mut q = format!("?limit={limit}");
    if let Some(o) = opt {
        q.push_str(&format!("&optimizationId={}", urlencode(&o)));
    }
    let items: Vec<ActivityVm> = c.get(&format!("/v1/activity{q}")).await?;
    if json {
        emit_json(io, &items);
        return Ok(0);
    }
    for a in &items {
        io.out(&format!("{:>9}  {}  {}\n", a.at, a.optimization_id, a.text));
    }
    Ok(0)
}

async fn repos(c: &Client, query: Option<String>, json: bool, io: &dyn Io) -> R {
    let q = query.map_or(String::new(), |q| format!("?query={}", urlencode(&q)));
    let list: Vec<RepoVm> = c.get(&format!("/v1/github/repos{q}")).await?;
    if json {
        emit_json(io, &list);
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|r| {
            vec![
                r.full_name.clone(),
                if r.private { "private" } else { "public" }.to_owned(),
                r.default_branch.clone(),
                r.description.clone().unwrap_or_default(),
            ]
        })
        .collect();
    io.out(&table(
        &["REPO", "VISIBILITY", "BRANCH", "DESCRIPTION"],
        &rows,
    ));
    Ok(0)
}

async fn inspect(c: &Client, target: &str, json: bool, io: &dyn Io) -> R {
    let looks_local = target.starts_with(['/', '.', '~']) || std::path::Path::new(target).is_dir();
    let source = if looks_local {
        let abs = std::path::absolute(target).unwrap_or_else(|_| target.into());
        RepoSource::Local {
            path: abs.display().to_string(),
        }
    } else {
        RepoSource::Github {
            repo: target.to_owned(),
        }
    };
    let r: RepoInspectVm = c
        .post("/v1/repos/inspections", &InspectRepoInput { source })
        .await?;
    if json {
        emit_json(io, &r);
        return Ok(0);
    }
    io.out(&format!(
        "{} ({})\n",
        r.repo.as_deref().unwrap_or(target),
        r.base_ref
    ));
    if let Some(p) = &r.problem {
        io.out(&format!("  problem: {p}\n"));
        return Ok(1);
    }
    match (&r.config, r.errors.is_empty()) {
        (Some(cfg), _) => io.out(&format!(
            "  ratchet.yaml: {} {} ({} evaluator, {} guardrail{})\n",
            cfg.direction,
            cfg.metric,
            cfg.evaluator.kind,
            cfg.guardrails.len(),
            if cfg.guardrails.len() == 1 { "" } else { "s" }
        )),
        (None, false) => {
            io.out("  ratchet.yaml is invalid:\n");
            for e in &r.errors {
                io.out(&format!("    - {e}\n"));
            }
            return Ok(1);
        }
        (None, true) => {
            io.out("  no ratchet.yaml; add one with `ratchet init --template <id>`:\n");
            for t in &r.templates {
                io.out(&format!("    {}  {}\n", t.id, t.title));
            }
        }
    }
    Ok(0)
}

async fn overview(c: &Client, range: &str, json: bool, io: &dyn Io) -> R {
    let o: OverviewVm = c.get(&format!("/v1/overview?range={range}")).await?;
    if json {
        emit_json(io, &o);
        return Ok(0);
    }
    io.out(&format!(
        "spent {}   value locked in ${}/yr   return {}\n",
        money(o.spent.usd),
        to_fixed(o.value_locked_in.usd_per_year, 0),
        o.return_on_spend
            .ratio
            .map_or(DASH.into(), |r| format!("{}x", to_fixed(r, 0)))
    ));
    if o.needs_you.is_empty() {
        io.out("\nNothing is waiting on you.\n");
    } else {
        io.out("\nNeeds you\n");
        for n in &o.needs_you {
            io.out(&format!("  {}: {}\n", n.title, n.subtitle));
        }
    }
    let rows: Vec<Vec<String>> = o
        .rows
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                r.state_text.clone(),
                r.result_text.clone(),
                money(r.spent_usd),
                r.value_usd_per_year
                    .map_or(DASH.into(), |v| format!("${}", to_fixed(v, 0))),
            ]
        })
        .collect();
    io.out(&format!(
        "\n{}",
        table(
            &["OPTIMIZATION", "STATE", "RESULT", "SPENT", "VALUE/YR"],
            &rows
        )
    ));
    Ok(0)
}

async fn connectors(c: &Client, json: bool, io: &dyn Io) -> R {
    let list: Vec<ConnectorVm> = c.get("/v1/connectors").await?;
    if json {
        emit_json(io, &list);
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|x| {
            vec![
                x.id.clone(),
                format!("{:?}", x.kind).to_lowercase(),
                x.name.clone(),
                x.status_text.clone(),
                if x.is_default {
                    "default".to_owned()
                } else {
                    String::new()
                },
            ]
        })
        .collect();
    io.out(&table(&["ID", "KIND", "NAME", "STATUS", ""], &rows));
    Ok(0)
}

// ---- agents, events, environments ---------------------------------------------------------------------------------

async fn agent(c: &Client, action: AgentCmd, json: bool, io: &dyn Io) -> R {
    match action {
        AgentCmd::Pause { id } => {
            c.post_unit::<()>(&format!("/v1/agents/{id}/pause"), None)
                .await?;
            done(
                io,
                json,
                json!({"id": id, "status": "paused"}),
                format!("paused {id}\n"),
            )
        }
        AgentCmd::Resume { id } => {
            c.post_unit::<()>(&format!("/v1/agents/{id}/resume"), None)
                .await?;
            done(
                io,
                json,
                json!({"id": id, "status": "running"}),
                format!("resumed {id}\n"),
            )
        }
        AgentCmd::Message { id, text } => {
            c.post_unit(
                &format!("/v1/agents/{id}/messages"),
                Some(&MessageAgentInput { text }),
            )
            .await?;
            done(
                io,
                json,
                json!({"id": id, "delivered": true}),
                format!("sent to {id}\n"),
            )
        }
        AgentCmd::Log { id, limit, after } => {
            let mut q = format!("?limit={limit}");
            if let Some(a) = after {
                q.push_str(&format!("&cursor={a}"));
            }
            let page: AgentEventsPage = c.get(&format!("/v1/agents/{id}/events{q}")).await?;
            if json {
                emit_json(io, &page);
            } else {
                for e in &page.events {
                    io.out(&format!(
                        "#{:<5} {}  {:<8} {}\n",
                        e.id,
                        clock(e.at),
                        e.event_type,
                        e.text
                    ));
                }
            }
            Ok(0)
        }
    }
}

fn event_line(id: Option<u64>, e: &ApiEvent) -> String {
    let mut v = serde_json::to_value(e).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("type");
    }
    let detail = v
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, x)| match x {
                    serde_json::Value::String(s) => format!("{k}={s}"),
                    other => format!("{k}={other}"),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    format!(
        "{:<6} {:<17} {detail}\n",
        id.map_or(DASH.to_owned(), |i| format!("#{i}")),
        e.name()
    )
}

async fn events(c: &Client, args: EventsArgs, json: bool, io: &dyn Io) -> R {
    let mut stream = c.events(args.last_event_id).await?;
    let mut seen = 0usize;
    let read = async {
        while let Some(m) = stream.next().await {
            let m = m?;
            if json {
                let mut v = serde_json::to_value(&m.event).unwrap_or_default();
                if let (Some(o), Some(id)) = (v.as_object_mut(), m.id) {
                    o.insert("id".into(), id.into());
                }
                io.out(&(serde_json::to_string(&v).unwrap_or_default() + "\n"));
            } else {
                io.out(&event_line(m.id, &m.event));
            }
            if !matches!(m.event, ApiEvent::Hello { .. } | ApiEvent::Resync { .. }) {
                seen += 1;
                if args.count.is_some_and(|n| seen >= n) {
                    break;
                }
            }
        }
        Ok::<(), ClientError>(())
    };
    match args.timeout.map(Duration::from_secs) {
        Some(d) => {
            if let Ok(r) = tokio::time::timeout(d, read).await {
                r?;
            }
        }
        None => read.await?,
    }
    Ok(0)
}

async fn env(c: &Client, action: EnvCmd, json: bool, io: &dyn Io) -> R {
    let v: EnvironmentsVm = match &action {
        EnvCmd::List => c.get("/v1/environments").await?,
        EnvCmd::End { sandbox } => {
            c.post_empty(&format!("/v1/environments/{sandbox}/end"))
                .await?
        }
    };
    if json {
        emit_json(io, &v);
        return Ok(0);
    }
    if let EnvCmd::End { sandbox } = &action {
        io.out(&format!("ended {sandbox}\n"));
        if let Some(r) = v.last_receipt.as_ref().filter(|r| &r.sandbox_id == sandbox) {
            io.out(&format!("{}\n", r.summary));
        }
        return Ok(0);
    }
    if v.environments.is_empty() {
        io.out("no environment is running\n");
    } else {
        let rows: Vec<Vec<String>> = v
            .environments
            .iter()
            .map(|e| {
                vec![
                    e.sandbox_id.clone(),
                    format!("{:?}", e.phase).to_lowercase(),
                    e.region.clone().unwrap_or_default(),
                    e.active_runs.join(","),
                    money(e.spend_usd),
                    if e.can_end {
                        "can end".to_owned()
                    } else {
                        String::new()
                    },
                ]
            })
            .collect();
        io.out(&table(
            &["ENVIRONMENT", "PHASE", "REGION", "RUNS", "SPENT", ""],
            &rows,
        ));
    }
    if let Some(r) = &v.last_receipt {
        io.out(&format!("last teardown: {}\n", r.summary));
    }
    Ok(0)
}

// ---- cost reconcile (port of formatReconcileReport) -------------------------------------------------------------

fn usd4(n: f64) -> String {
    format!(
        "{}${}",
        if n < 0.0 { "-" } else { "" },
        to_fixed(n.abs(), 4)
    )
}

fn signed4(n: f64) -> String {
    format!(
        "{}${}",
        if n < 0.0 { "-" } else { "+" },
        to_fixed(n.abs(), 4)
    )
}

pub fn format_reconcile(r: &CostReconcileReport) -> String {
    let headline = match r.status {
        CostReconcileStatus::Pending => format!(
            "pending: nothing was written. {}",
            r.reason.as_deref().unwrap_or("")
        )
        .trim()
        .to_owned(),
        CostReconcileStatus::Unchanged => {
            "unchanged: the ledger already matches billing".to_owned()
        }
        CostReconcileStatus::Reconciled => format!(
            "reconciled: {} correction row(s) written, {} in total",
            r.lines.iter().filter(|l| l.written).count(),
            signed4(r.written_usd)
        ),
    };
    let mut out = vec![format!("Cost reconcile {}: {headline}", r.optimization_id)];
    if !r.lines.is_empty() {
        out.push(String::new());
        out.push(format!(
            "  {:<14} {:>10} {:>10} {:>10}",
            "category", "ledger", "billed", "delta"
        ));
        for l in &r.lines {
            out.push(format!(
                "  {:<14} {:>10} {:>10} {:>10}{}",
                l.category.as_str(),
                usd4(l.ledger_usd),
                usd4(l.actual_usd),
                signed4(l.delta_usd),
                if l.written { "  written" } else { "" }
            ));
        }
    }
    out.join("\n") + "\n"
}

// ---- run ----------------------------------------------------------------------------------------------------------

fn agent_kind(a: AgentArg) -> AgentKind {
    match a {
        AgentArg::ClaudeCode => AgentKind::ClaudeCode,
        AgentArg::Codex => AgentKind::Codex,
        AgentArg::Mock => AgentKind::Mock,
    }
}

fn is_terminal(s: UiStatus) -> bool {
    matches!(
        s,
        UiStatus::Completed | UiStatus::Stopped | UiStatus::Failed | UiStatus::PrOpen
    )
}

async fn run_optimization(c: &Client, a: RunArgs, json: bool, io: &dyn Io) -> R {
    let demo = a.demo.map(|d| match d {
        DemoArg::Search => DemoId::Search,
        DemoArg::Cli => DemoId::Cli,
        DemoArg::Forecast => DemoId::Forecast,
    });
    if demo.is_none() && a.repo.is_none() {
        io.err("ratchet: run needs a repo (owner/name) or --demo <demo-search|demo-cli|demo-forecast>\n");
        return Ok(2);
    }
    let input = CreateOptimizationInput {
        name: a.name.clone(),
        repo_url: a.repo.clone().unwrap_or_default(),
        base_ref: a.base_ref.clone(),
        demo,
        agents: a.agents.iter().copied().map(agent_kind).collect(),
        agents_where: a.agents_where.map(|w| match w {
            WhereArg::Local => AgentsWhere::Local,
            WhereArg::Cloud => AgentsWhere::Cloud,
        }),
        budget_usd: a.budget,
        time_limit_seconds: a.time_limit,
        plateau_evals: a.plateau,
        connectors: None,
        environment_draft_id: None,
    };
    // Where the event stream resumes from, so --wait cannot miss the end of a fast run.
    let from = if a.wait {
        Some(c.get::<DaemonStatus>("/v1/status").await?.last_event_id)
    } else {
        None
    };
    let created: CreatedOptimization = c.post("/v1/optimizations", &input).await?;
    let id = created.id;
    if !a.no_start {
        c.post_unit::<()>(&format!("/v1/optimizations/{id}/start"), None)
            .await?;
    }
    if !a.wait || a.no_start {
        return done(
            io,
            json,
            json!({"id": id, "started": !a.no_start}),
            format!(
                "{id}{}\n",
                if a.no_start {
                    " (created, not started)"
                } else {
                    " started"
                }
            ),
        );
    }

    // --wait: follow the stream until the optimization ended
    let mut stream = c.events(from).await?;
    let follow = async {
        // it may already be over
        let d: OptimizationDetail = c.get(&format!("/v1/optimizations/{id}")).await?;
        if is_terminal(d.summary.status) {
            return Ok::<(), ClientError>(());
        }
        while let Some(m) = stream.next().await {
            match m?.event {
                ApiEvent::Optimization {
                    optimization_id,
                    status,
                } if optimization_id == id => {
                    if !json {
                        io.out(&format!("{id}: {status}\n"));
                    }
                    if status.is_terminal() {
                        break;
                    }
                }
                ApiEvent::Ratchet {
                    optimization_id,
                    from,
                    to,
                } if optimization_id == id && !json => {
                    io.out(&format!(
                        "{id}: verified win {} -> {}\n",
                        num(Some(from)),
                        num(Some(to))
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    };
    let timed_out = match a.wait_timeout.map(Duration::from_secs) {
        Some(l) => match tokio::time::timeout(l, follow).await {
            Ok(r) => {
                r?;
                false
            }
            Err(_) => true,
        },
        None => {
            follow.await?;
            false
        }
    };
    let d: OptimizationDetail = c.get(&format!("/v1/optimizations/{id}")).await?;
    let ok = matches!(d.summary.status, UiStatus::Completed | UiStatus::PrOpen);
    if json {
        emit_json(
            io,
            &json!({"id": id, "status": d.summary.status, "timedOut": timed_out, "optimization": d.summary}),
        );
    } else {
        if timed_out {
            io.err(&format!(
                "ratchet: gave up waiting for {id}; it is still {}\n",
                status_word(d.summary.status)
            ));
        }
        io.out(&format!(
            "{id}: {} \u{b7} {} \u{b7} {} spent\n",
            status_word(d.summary.status),
            result_text(&d.summary),
            money(d.summary.spend_usd)
        ));
    }
    Ok(i32::from(!ok))
}
