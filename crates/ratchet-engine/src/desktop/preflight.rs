//! Is every harness on an optimization's list able to run where the optimization runs? Checked before the optimization is
//! created or started, so a signed-out harness is refused with its reason instead of starting and stalling.

use ratchet_agents::detect::{HarnessAuth, HarnessInfo, HarnessKind};
use ratchet_core::enums::{AgentKind, AgentsWhere};

/// One harness that cannot run, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProblem {
    pub agent: AgentKind,
    /// A sentence naming the harness, what is wrong and where to fix it.
    pub reason: String,
}

fn label(kind: AgentKind) -> Option<&'static str> {
    match kind {
        AgentKind::ClaudeCode => Some("Claude Code"),
        AgentKind::Codex => Some("Codex"),
        AgentKind::Grok => Some("Grok"),
        AgentKind::Mock => None,
    }
}

fn vendor(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "Anthropic",
        AgentKind::Codex => "OpenAI",
        AgentKind::Grok => "xAI",
        AgentKind::Mock => "",
    }
}

fn harness_kind(kind: AgentKind) -> Option<HarnessKind> {
    match kind {
        AgentKind::ClaudeCode => Some(HarnessKind::ClaudeCode),
        AgentKind::Codex => Some(HarnessKind::Codex),
        AgentKind::Grok => Some(HarnessKind::Grok),
        AgentKind::Mock => None,
    }
}

/// The problems of `agents` for a run `where_`. Empty = ready.
///
/// - `Local`: each harness must be installed on this machine and signed in (a login or an API key in the environment).
/// - `Cloud`: each harness needs the API key its runner receives (`has_cloud_key`); a login on this machine does not travel.
///
/// Scripted (`mock`) agents need nothing.
pub fn agent_problems(
    agents: &[AgentKind],
    where_: AgentsWhere,
    harnesses: &[HarnessInfo],
    has_cloud_key: impl Fn(AgentKind) -> bool,
) -> Vec<AgentProblem> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for &agent in agents {
        let (Some(name), Some(kind)) = (label(agent), harness_kind(agent)) else {
            continue;
        };
        if seen.contains(&agent) {
            continue;
        }
        seen.push(agent);
        let reason = match where_ {
            AgentsWhere::Local => match harnesses.iter().find(|h| h.kind == kind) {
                None => Some(format!(
                    "{name} is not installed on this machine (Settings > Agents)."
                )),
                Some(h) if !h.installed => Some(format!(
                    "{name} is not installed on this machine (Settings > Agents)."
                )),
                Some(h) if h.auth == HarnessAuth::SignedOut => {
                    Some(format!("{name} is signed out. {}", h.hint))
                }
                Some(_) => None,
            },
            AgentsWhere::Cloud => (!has_cloud_key(agent)).then(|| {
                format!(
                    "{name} cannot run in the cloud without an {} API key (Settings > Agents).",
                    vendor(agent)
                )
            }),
        };
        if let Some(reason) = reason {
            out.push(AgentProblem { agent, reason });
        }
    }
    out
}

/// The problems as one line for an error message.
pub fn describe(problems: &[AgentProblem]) -> String {
    problems
        .iter()
        .map(|p| p.reason.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(kind: HarnessKind, installed: bool, auth: HarnessAuth, hint: &str) -> HarnessInfo {
        HarnessInfo {
            kind,
            installed,
            version: installed.then(|| "1.0.0".to_owned()),
            path: None,
            auth,
            hint: hint.to_owned(),
        }
    }

    fn found() -> Vec<HarnessInfo> {
        vec![
            h(HarnessKind::ClaudeCode, true, HarnessAuth::SignedIn, "ok"),
            h(
                HarnessKind::Codex,
                true,
                HarnessAuth::SignedOut,
                "Run `codex login` (ChatGPT account or API key), or set OPENAI_API_KEY.",
            ),
            h(HarnessKind::Grok, false, HarnessAuth::SignedOut, "install"),
        ]
    }

    #[test]
    fn a_signed_out_local_harness_is_refused_with_its_sign_in_hint() {
        let p = agent_problems(
            &[AgentKind::ClaudeCode, AgentKind::Codex],
            AgentsWhere::Local,
            &found(),
            |_| false,
        );
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].agent, AgentKind::Codex);
        assert_eq!(
            p[0].reason,
            "Codex is signed out. Run `codex login` (ChatGPT account or API key), or set OPENAI_API_KEY."
        );
    }

    #[test]
    fn a_missing_cli_is_refused_locally() {
        let p = agent_problems(&[AgentKind::Grok], AgentsWhere::Local, &found(), |_| true);
        assert!(p[0].reason.contains("Grok is not installed"));
        let none = agent_problems(&[AgentKind::Grok], AgentsWhere::Local, &[], |_| true);
        assert!(none[0].reason.contains("not installed"));
    }

    #[test]
    fn cloud_needs_the_vendor_key_and_ignores_local_logins() {
        // Claude Code is signed in on this Mac, which does not help a cloud runner.
        let p = agent_problems(
            &[AgentKind::ClaudeCode, AgentKind::Grok],
            AgentsWhere::Cloud,
            &found(),
            |a| a == AgentKind::ClaudeCode,
        );
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].agent, AgentKind::Grok);
        assert_eq!(
            p[0].reason,
            "Grok cannot run in the cloud without an xAI API key (Settings > Agents)."
        );
    }

    #[test]
    fn scripted_agents_and_repeats_are_not_checked_twice() {
        assert!(agent_problems(&[AgentKind::Mock], AgentsWhere::Cloud, &[], |_| false).is_empty());
        let p = agent_problems(
            &[AgentKind::Codex, AgentKind::Codex],
            AgentsWhere::Local,
            &found(),
            |_| true,
        );
        assert_eq!(p.len(), 1);
    }
}
