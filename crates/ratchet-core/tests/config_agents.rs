//! `agents:` in ratchet.yaml: the optional override of an optimization's harness order and fallback rules.

use ratchet_core::AgentsPolicy;
use ratchet_core::config::parse_config;
use ratchet_core::enums::AgentKind;

const BASE: &str = "version: 0\nobjective: { metric: http_req_duration.p95, direction: minimize }\nevaluator:\n  template: http-latency\nagent:\n  instructions: RATCHET.md\n  allowed_instance_types: [c7i.large]\n";

fn with(extra: &str) -> String {
    format!("{BASE}{extra}")
}

#[test]
fn absent_means_the_ledger_settings_apply() {
    assert_eq!(parse_config(BASE).unwrap().agents, None);
}

#[test]
fn a_plain_list_is_the_order_with_default_rules() {
    let c = parse_config(&with("agents: [codex, claude-code, codex]\n")).unwrap();
    assert_eq!(
        c.agents,
        Some(AgentsPolicy::new([AgentKind::Codex, AgentKind::ClaudeCode]))
    );
}

#[test]
fn a_mapping_sets_the_rules_in_minutes() {
    let c = parse_config(&with(
        "agents:\n  order: [claude-code, grok]\n  wait_limit_minutes: 10\n  crash_strikes: 2\n  recheck_minutes: 1\n",
    ))
    .unwrap();
    let a = c.agents.unwrap();
    assert_eq!(a.harnesses, [AgentKind::ClaudeCode, AgentKind::Grok]);
    assert_eq!((a.wait_limit_s, a.crash_strikes, a.recheck_s), (600, 2, 60));
}

#[test]
fn bad_values_are_named() {
    for (extra, needle) in [
        ("agents: [gemini]\n", "agents must list harnesses"),
        ("agents: [mock]\n", "agents must list harnesses"),
        ("agents: []\n", "at least one harness"),
        ("agents: nope\n", "agents must list harnesses"),
        ("agents:\n  crash_strikes: 2\n", "agents.order is required"),
        (
            "agents:\n  order: [codex]\n  crash_strikes: 0\n",
            "at least 1",
        ),
        (
            "agents:\n  order: [codex]\n  recheck_minutes: 1.5\n",
            "whole number",
        ),
    ] {
        let errs = parse_config(&with(extra)).expect_err(extra);
        assert!(
            errs.iter().any(|e| e.message.contains(needle)),
            "{extra}: {errs:?}"
        );
    }
}
