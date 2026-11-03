//! Cards and error wording: what the chat sends the UI. Words follow the app's glossary (optimization, agents, run) and say what
//! happened and what to do, without blame.

use ratchet_core::ports::PortError;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::*;

/// What `draft_optimization` returns (the fields the card needs).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftFields {
    pub draft_id: String,
    pub repo: String,
    #[serde(default)]
    pub config_branch: String,
    #[serde(default)]
    pub commit: String,
    pub goal: String,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub metric: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub budget_usd: f64,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

fn money(usd: f64) -> String {
    if (usd - usd.round()).abs() < 0.005 {
        format!("${}", usd.round() as i64)
    } else {
        format!("${usd:.2}")
    }
}

fn agent_label(id: &str) -> String {
    HarnessKind::from_id(id).map_or_else(|| id.to_owned(), |k| k.label().to_owned())
}

/// A short title from the goal: its words on one line, at most 60 characters.
fn title_of(goal: &str) -> String {
    let line = goal.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('…');
    }
    t
}

fn row(key: &str, label: &str, value: String, editable: bool, raw: Value) -> CardRowVm {
    CardRowVm {
        key: key.into(),
        label: label.into(),
        value,
        editable,
        raw,
        options: Vec::new(),
    }
}

/// The summary card of a draft. `harnesses` are the harnesses a person could pick for the Agents row (signed in on this Mac).
pub fn draft_card(d: &DraftFields, harnesses: &[HarnessKind]) -> ChatCardVm {
    let direction = match d.direction.as_str() {
        "minimize" => "lower is better",
        "maximize" => "higher is better",
        _ => "",
    };
    let measure = if direction.is_empty() {
        d.metric.clone()
    } else {
        format!("{} · {direction}", d.metric)
    };
    let agents = if d.agents.is_empty() {
        "Claude Code".to_owned()
    } else {
        d.agents
            .iter()
            .map(|a| agent_label(a))
            .collect::<Vec<_>>()
            .join(" → ")
    };
    let mut agents_row = row(
        "agents",
        "Agents",
        format!("{agents} · on this Mac"),
        true,
        json!(d.agents),
    );
    agents_row.options = harnesses
        .iter()
        .map(|h| CardOptionVm {
            value: json!(h.id()),
            label: h.label().to_owned(),
        })
        .collect();
    let schedule = d
        .schedule
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "run once, now".to_owned());
    let limits = format!("up to {}", money(d.budget_usd));
    let mut rows = vec![
        row("goal", "Goal", d.goal.clone(), true, json!(d.goal)),
        row(
            "measure",
            "Measures",
            measure,
            true,
            json!({"metric": d.metric, "direction": d.direction, "template": d.template}),
        ),
        agents_row,
        row(
            "limits",
            "Each run",
            limits.clone(),
            true,
            json!({"budgetUsd": d.budget_usd}),
        ),
        row("schedule", "Schedule", schedule, true, json!(d.schedule)),
        row(
            "output",
            "Output",
            "a pull request with verified wins".into(),
            false,
            Value::Null,
        ),
    ];
    if !d.files.is_empty() {
        rows.push(row(
            "config",
            "Config",
            format!("{} → branch {}", d.files.join(" · "), d.config_branch),
            false,
            json!({"files": d.files, "branch": d.config_branch, "commit": d.commit}),
        ));
    }
    ChatCardVm {
        id: d.draft_id.clone(),
        kind: ChatCardKind::Draft,
        status: ChatCardStatus::Draft,
        title: title_of(&d.goal),
        subtitle: Some(format!("{limits} per run")),
        repo: Some(d.repo.clone()),
        rows,
        optimization: None,
        error: None,
        read_by: None,
        read_at: None,
    }
}

pub fn note_card(
    note_id: &str,
    optimization: &str,
    text: &str,
    status: ChatCardStatus,
) -> ChatCardVm {
    ChatCardVm {
        id: note_id.to_owned(),
        kind: ChatCardKind::Note,
        status,
        title: text.to_owned(),
        subtitle: None,
        repo: None,
        rows: Vec::new(),
        optimization: Some(optimization.to_owned()),
        error: None,
        read_by: None,
        read_at: None,
    }
}

/// The first-use notice (what leaves the Mac).
pub fn data_notice(kind: HarnessKind) -> DataNoticeVm {
    DataNoticeVm {
        harness: kind,
        text: format!(
            "First time: your messages, the repo name and the files it reads go to {}, signed in on this Mac. Ratchet sends nothing else.",
            kind.label()
        ),
    }
}

/// `mcp__ratchet__draft_optimization` → `draft_optimization`.
pub fn tool_name(raw: &str) -> String {
    raw.strip_prefix("mcp__ratchet__").unwrap_or(raw).to_owned()
}

/// A one-line label for a tool step when the CLI gave none.
pub fn tool_label(name: &str) -> String {
    match name {
        "draft_optimization" => "Drafted an optimization".into(),
        "create_optimization" => "Created the optimization".into(),
        "list_repos" => "Looked at your repositories".into(),
        "list_templates" => "Looked at the templates".into(),
        "list_optimizations" => "Looked at your optimizations".into(),
        "get_optimization" => "Read the optimization".into(),
        "get_run" => "Read the run".into(),
        "get_overview" => "Read the overview".into(),
        "explain_result" => "Read the result".into(),
        "run_now" => "Started a run".into(),
        "pause" => "Paused the optimization".into(),
        "resume" => "Resumed the optimization".into(),
        "add_steering_note" => "Sent a steering note".into(),
        "open_pr" => "Opened the pull request".into(),
        other => other.replace('_', " "),
    }
}

/// Why a turn did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    RateLimited { resets_at_ms: Option<i64> },
    AuthExpired,
    Unavailable(String),
    Failed(String),
}

impl Failure {
    pub fn from_port(e: &PortError) -> Self {
        Self::Failed(first_line(&e.to_string()))
    }
}

/// A failed turn, in words for the person. `others` are the harnesses a chat could switch to (signed in, not this one).
pub fn error_for(
    harness: HarnessKind,
    failure: &Failure,
    others: &[HarnessKind],
    now_ms: i64,
) -> ChatErrorVm {
    let name = harness.label();
    let switch_to = others.first().copied();
    let or_with = |lead: &str| switch_to.map_or(String::new(), |o| format!("{lead}{}.", o.label()));
    match failure {
        Failure::RateLimited { resets_at_ms } => {
            let (when, can_retry_now) = match resets_at_ms {
                Some(t) if *t > now_ms => (format!(" It resets at {}.", clock(*t)), false),
                Some(_) => (" Its limit should have reset.".to_owned(), true),
                None => (" It did not say when it resets.".to_owned(), false),
            };
            let mut fixes = Vec::new();
            if can_retry_now {
                fixes.push(ChatFix::Retry);
            }
            if switch_to.is_some() {
                fixes.push(ChatFix::SwitchHarness);
            }
            if fixes.is_empty() {
                fixes.push(ChatFix::Retry);
            }
            ChatErrorVm {
                code: ChatErrorCode::RateLimited,
                harness,
                title: format!("{name} reached its usage limit"),
                message: format!(
                    "{name} cannot answer until the limit lifts.{when}{}",
                    or_with(" You can also continue with ")
                ),
                retry: Some(ChatRetryVm {
                    resets_at: *resets_at_ms,
                    can_retry_now,
                }),
                fixes,
                switch_to,
            }
        }
        Failure::AuthExpired => {
            let mut fixes = vec![ChatFix::SignIn];
            if switch_to.is_some() {
                fixes.push(ChatFix::SwitchHarness);
            }
            ChatErrorVm {
                code: ChatErrorCode::AuthExpired,
                harness,
                title: format!("{name} needs you to sign in again"),
                message: format!(
                    "{name} rejected its login on this Mac. Sign in again in Customize, then send your message again.{}",
                    or_with(" Or continue with ")
                ),
                retry: Some(ChatRetryVm {
                    resets_at: None,
                    can_retry_now: false,
                }),
                fixes,
                switch_to,
            }
        }
        Failure::Unavailable(why) => ChatErrorVm {
            code: ChatErrorCode::Unavailable,
            harness,
            title: format!("{name} is not ready"),
            message: why.clone(),
            retry: Some(ChatRetryVm {
                resets_at: None,
                can_retry_now: false,
            }),
            fixes: vec![ChatFix::SignIn],
            switch_to,
        },
        Failure::Failed(why) => ChatErrorVm {
            code: ChatErrorCode::Failed,
            harness,
            title: format!("{name} stopped before it finished"),
            message: format!("{} Try again, or start a new chat.", sentence(why)),
            retry: Some(ChatRetryVm {
                resets_at: None,
                can_retry_now: true,
            }),
            fixes: vec![ChatFix::Retry],
            switch_to,
        },
    }
}

fn first_line(s: &str) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out: String = line.chars().take(200).collect();
    if line.chars().count() > 200 {
        out.push('…');
    }
    out
}

fn sentence(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "It gave no reason.".to_owned();
    }
    if s.ends_with(['.', '!', '?']) {
        s.to_owned()
    } else {
        format!("{s}.")
    }
}

/// `HH:MM UTC` of an epoch-ms time, for the sentence. The app shows its own local time from `resetsAt`.
fn clock(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let day = secs.rem_euclid(86_400);
    format!("{:02}:{:02} UTC", day / 3600, (day % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> DraftFields {
        serde_json::from_value(json!({
            "draftId": "draft_1", "repo": "acme/shop", "configBranch": "ratchet/config", "commit": "abc",
            "goal": "Make checkout p95 faster", "template": "http-latency", "metric": "p95_ms", "direction": "minimize",
            "schedule": "every night at 02:00", "budgetUsd": 50.0, "agents": ["claude-code", "codex"],
            "files": ["ratchet.yaml", "RATCHET.md"]
        }))
        .unwrap()
    }

    #[test]
    fn a_draft_card_has_one_editable_row_per_setting() {
        let c = draft_card(&draft(), &[HarnessKind::ClaudeCode, HarnessKind::Codex]);
        assert_eq!(c.id, "draft_1");
        assert_eq!(c.status, ChatCardStatus::Draft);
        assert_eq!(c.title, "Make checkout p95 faster");
        let keys: Vec<_> = c.rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "goal", "measure", "agents", "limits", "schedule", "output", "config"
            ]
        );
        let agents = &c.rows[2];
        assert_eq!(agents.value, "Claude Code → Codex · on this Mac");
        assert_eq!(agents.options.len(), 2);
        assert!(agents.editable);
        assert_eq!(c.rows[3].value, "up to $50");
        assert_eq!(c.rows[3].raw, json!({"budgetUsd": 50.0}));
        assert!(!c.rows[5].editable, "the output is fixed");
        assert_eq!(c.subtitle.as_deref(), Some("up to $50 per run"));
    }

    #[test]
    fn errors_say_what_happened_and_what_to_press() {
        let r = error_for(
            HarnessKind::ClaudeCode,
            &Failure::RateLimited {
                resets_at_ms: Some(1_000_000 + 3_600_000),
            },
            &[HarnessKind::Codex],
            1_000_000,
        );
        assert_eq!(r.code, ChatErrorCode::RateLimited);
        assert!(r.title.contains("Claude Code"));
        assert!(r.message.contains("resets at 01:16 UTC"), "{}", r.message);
        assert!(r.message.contains("continue with Codex"));
        assert_eq!(r.fixes, [ChatFix::SwitchHarness]);
        assert_eq!(r.switch_to, Some(HarnessKind::Codex));
        assert!(!r.retry.unwrap().can_retry_now);

        let past = error_for(
            HarnessKind::Codex,
            &Failure::RateLimited {
                resets_at_ms: Some(5),
            },
            &[],
            10,
        );
        assert_eq!(past.fixes, [ChatFix::Retry]);

        let a = error_for(HarnessKind::Grok, &Failure::AuthExpired, &[], 0);
        assert_eq!(a.code, ChatErrorCode::AuthExpired);
        assert_eq!(a.fixes, [ChatFix::SignIn]);
        assert!(a.message.starts_with("Grok rejected its login"));

        let f = error_for(HarnessKind::Grok, &Failure::Failed("exit 1".into()), &[], 0);
        assert!(f.message.contains("exit 1."));
        assert_eq!(f.fixes, [ChatFix::Retry]);
    }

    #[test]
    fn tool_names_lose_the_server_prefix() {
        assert_eq!(
            tool_name("mcp__ratchet__draft_optimization"),
            "draft_optimization"
        );
        assert_eq!(tool_name("Read"), "Read");
        assert_eq!(tool_label("add_steering_note"), "Sent a steering note");
    }
}
