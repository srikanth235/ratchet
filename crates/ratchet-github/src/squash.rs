//! The squash commit message for a pull request of verified wins.

use crate::error::{Error, Result};
use crate::numfmt::{num, to_fixed};
use ratchet_core::enums::{AgentKind, Direction};
use ratchet_core::gain_percent;

#[derive(Debug, Clone, PartialEq)]
pub struct SquashWin {
    pub sha: String,
    pub agent_kind: AgentKind,
    /// Candidate commit message; the first line is used as the change summary.
    pub message: String,
    pub from_value: f64,
    pub to_value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SquashPlanInput {
    pub metric: String,
    pub direction: Direction,
    /// Verified wins in ratchet order.
    pub wins: Vec<SquashWin>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SquashPlan {
    pub title: String,
    /// Full commit message: title, body and a `Co-authored-by` trailer block.
    pub message: String,
    /// Verified SHAs in ratchet order.
    pub shas: Vec<String>,
    /// SHA whose tree becomes the squash commit. Later wins are rebased onto earlier ones, so the final verified SHA
    /// already contains every earlier win.
    pub final_sha: String,
    pub co_authors: Vec<String>,
}

fn co_author(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "Claude Code <noreply@anthropic.com>",
        AgentKind::Codex => "Codex <noreply@openai.com>",
        AgentKind::Grok => "Grok <noreply@x.ai>",
        AgentKind::Mock => "Ratchet Mock Agent <mock-agent@ratchet.invalid>",
    }
}

/// First line of a commit message, or `(no message)`.
pub fn subject_of(message: &str) -> String {
    let first = message.split('\n').next().unwrap_or("").trim();
    if first.is_empty() {
        "(no message)".into()
    } else {
        first.into()
    }
}

fn pct(direction: Direction, from: f64, to: f64) -> String {
    match gain_percent(direction, from, to) {
        None => "n/a".into(),
        Some(g) => format!(
            "{}{}%",
            if g >= 0.0 { "" } else { "-" },
            to_fixed(g.abs(), 1)
        ),
    }
}

/// Build the squash commit message and the SHAs it covers.
pub fn build_squash_plan(input: &SquashPlanInput) -> Result<SquashPlan> {
    let wins = &input.wins;
    let (Some(first), Some(last)) = (wins.first(), wins.last()) else {
        return Err(Error::invalid("cannot squash: no verified wins"));
    };
    let noun = if wins.len() == 1 {
        "verified win"
    } else {
        "verified wins"
    };
    let title = format!(
        "ratchet: {} {} → {} ({}, {} {noun})",
        input.metric,
        num(first.from_value),
        num(last.to_value),
        pct(input.direction, first.from_value, last.to_value),
        wins.len()
    );
    let body: Vec<String> = wins
        .iter()
        .map(|w| {
            format!(
                "- {} ({}, {}, {})",
                subject_of(&w.message),
                w.agent_kind.as_str(),
                &w.sha[..w.sha.len().min(8)],
                pct(input.direction, w.from_value, w.to_value)
            )
        })
        .collect();
    let mut co_authors: Vec<String> = Vec::new();
    for w in wins {
        let c = co_author(w.agent_kind).to_string();
        if !co_authors.contains(&c) {
            co_authors.push(c);
        }
    }
    let mut lines = vec![title.clone(), String::new()];
    lines.extend(body);
    lines.push(String::new());
    lines.extend(co_authors.iter().map(|c| format!("Co-authored-by: {c}")));
    Ok(SquashPlan {
        title,
        message: lines.join("\n"),
        shas: wins.iter().map(|w| w.sha.clone()).collect(),
        final_sha: last.sha.clone(),
        co_authors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(c: char, kind: AgentKind, message: &str, from: f64, to: f64) -> SquashWin {
        SquashWin {
            sha: c.to_string().repeat(40),
            agent_kind: kind,
            message: message.into(),
            from_value: from,
            to_value: to,
        }
    }

    // squash.test.ts: builds message, trailers and sha list in ratchet order
    #[test]
    fn builds_message_trailers_and_sha_list_in_ratchet_order() {
        let plan = build_squash_plan(&SquashPlanInput {
            metric: "http_req_duration.p95".into(),
            direction: Direction::Minimize,
            wins: vec![
                win(
                    'a',
                    AgentKind::ClaudeCode,
                    "Cache tokenizer\n\nbody",
                    400.0,
                    300.0,
                ),
                win('b', AgentKind::Codex, "Index postings", 300.0, 200.0),
                win('c', AgentKind::ClaudeCode, "Pool buffers", 200.0, 180.0),
            ],
        })
        .unwrap();
        assert_eq!(
            plan.shas,
            vec!["a".repeat(40), "b".repeat(40), "c".repeat(40)]
        );
        assert_eq!(plan.final_sha, "c".repeat(40));
        assert_eq!(
            plan.message,
            "ratchet: http_req_duration.p95 400 → 180 (55.0%, 3 verified wins)\n\
             \n\
             - Cache tokenizer (claude-code, aaaaaaaa, 25.0%)\n\
             - Index postings (codex, bbbbbbbb, 33.3%)\n\
             - Pool buffers (claude-code, cccccccc, 10.0%)\n\
             \n\
             Co-authored-by: Claude Code <noreply@anthropic.com>\n\
             Co-authored-by: Codex <noreply@openai.com>"
        );
        assert_eq!(plan.co_authors.len(), 2);
    }

    // squash.test.ts: rejects an empty win list
    #[test]
    fn rejects_an_empty_win_list() {
        let err = build_squash_plan(&SquashPlanInput {
            metric: "m".into(),
            direction: Direction::Minimize,
            wins: vec![],
        })
        .unwrap_err();
        assert!(err.to_string().contains("no verified wins"));
    }

    #[test]
    fn subject_falls_back_when_empty() {
        assert_eq!(subject_of("  \nbody"), "(no message)");
        assert_eq!(subject_of("Do it\n\nmore"), "Do it");
    }
}
