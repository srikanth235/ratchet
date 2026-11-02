//! The chat harness of one CLI on this machine: the same adapters the optimizations use, handed out as [`Harness`] so a caller can
//! hold a conversation open (`Harness::open`). Built like `local_agents::local_adapter_for`: the binary detection found, the user's
//! own login, no API key unless detection reported `api-key` auth.

use std::collections::HashMap;
use std::sync::Arc;

use ratchet_core::enums::AgentKind;
use ratchet_core::ports::Harness;

use crate::claude_code::{ClaudeCodeAdapter, ClaudeCodeOptions};
use crate::cli_agent::CliAgentOptions;
use crate::codex::{CodexAdapter, CodexOptions};
use crate::detect::{HarnessKind, local_agent_env};
use crate::grok::{GrokAdapter, GrokOptions};
use crate::local_agents::{LocalAdapterOptions, local_agent_problem};

fn merge_cli(common: CliAgentOptions, over: &CliAgentOptions) -> CliAgentOptions {
    CliAgentOptions {
        binary: over.binary.clone().or(common.binary),
        env: over.env.clone().or(common.env),
        spawner: over.spawner.clone().or(common.spawner),
        clock: over.clock.clone().or(common.clock),
        extra_args: if over.extra_args.is_empty() {
            common.extra_args
        } else {
            over.extra_args.clone()
        },
        kill_grace: over.kill_grace.or(common.kill_grace),
        disable_live_session: over.disable_live_session || common.disable_live_session,
    }
}

/// The harness of `kind` on this machine. Fails with the detection hint when the CLI is missing or signed out.
pub fn local_harness_for(
    opts: &LocalAdapterOptions,
    kind: AgentKind,
) -> Result<Arc<dyn Harness>, String> {
    let base_env: HashMap<String, String> = opts
        .base_env
        .clone()
        .unwrap_or_else(|| std::env::vars().collect());
    let kind = match kind {
        AgentKind::ClaudeCode => HarnessKind::ClaudeCode,
        AgentKind::Codex => HarnessKind::Codex,
        AgentKind::Grok => HarnessKind::Grok,
        AgentKind::Mock => return Err("mock agents cannot chat".to_string()),
    };
    if let Some(problem) = local_agent_problem(&opts.harnesses, kind) {
        return Err(problem);
    }
    let info = opts
        .harnesses
        .iter()
        .find(|h| h.kind == kind)
        .expect("checked by local_agent_problem");
    let env = local_agent_env(kind, info.auth, &base_env, &opts.login_path);
    let common = CliAgentOptions {
        binary: info.path.clone(),
        env: Some(env),
        spawner: opts.spawner.clone(),
        clock: opts.clock.clone(),
        ..CliAgentOptions::default()
    };
    Ok(match kind {
        HarnessKind::ClaudeCode => {
            let over = opts.claude.clone().unwrap_or_default();
            Arc::new(ClaudeCodeAdapter::new(ClaudeCodeOptions {
                cli: merge_cli(common, &over.cli),
                allow_subscription_auth: true,
                ..over
            })) as Arc<dyn Harness>
        }
        HarnessKind::Codex => {
            let over = opts.codex.clone().unwrap_or_default();
            Arc::new(CodexAdapter::new(CodexOptions {
                cli: merge_cli(common, &over.cli),
                allow_subscription_auth: true,
                ..over
            })) as Arc<dyn Harness>
        }
        HarnessKind::Grok => {
            let over = opts.grok.clone().unwrap_or_default();
            Arc::new(GrokAdapter::new(GrokOptions {
                cli: merge_cli(common, &over.cli),
                allow_subscription_auth: true,
                ..over
            })) as Arc<dyn Harness>
        }
    })
}
