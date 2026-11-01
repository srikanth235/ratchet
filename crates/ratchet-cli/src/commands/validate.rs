//! `ratchet validate [path]` (offline). Port of `packages/cli/src/commands/validate.ts`.

use std::path::Path;

use ratchet_core::config::{format_config_error, guardrail_name, load_config};
use ratchet_core::numfmt::{js_number_to_string, to_fixed};
use ratchet_core::protected_set::ProtectedSource;
use ratchet_core::{ProtectedConfigInput, effective_protected};
use serde_json::json;

use super::emit_json;
use crate::io::Io;

pub const DEFAULT_CONFIG_PATH: &str = "ratchet.yaml";

/// `3.00` -> `3`, `2.50` -> `2.5` (`.toFixed(2).replace(/\.?0+$/, '')`).
fn trim_fraction(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// Returns the process exit code: 0 valid, 1 invalid.
pub fn run_validate(path: &Path, json: bool, io: &dyn Io) -> i32 {
    let file = path.display().to_string();
    let c = match load_config(path) {
        Ok(c) => c,
        Err(errors) => {
            if json {
                emit_json(io, &json!({"file": file, "ok": false, "errors": errors}));
            } else {
                io.err(&format!(
                    "{file}: invalid ({} error{})\n",
                    errors.len(),
                    if errors.len() == 1 { "" } else { "s" }
                ));
                for e in &errors {
                    io.err(&format!("  - {}\n", format_config_error(e)));
                }
            }
            return 1;
        }
    };
    if json {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let r = ratchetd::api::validate_config_text(&text);
        emit_json(io, &json!({"file": file, "ok": true, "summary": r.summary}));
        return 0;
    }
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_owned());
    let auto = effective_protected(&ProtectedConfigInput::from(&c), Some(&base));
    let mut lines = vec![
        format!("{file}: OK"),
        format!(
            "  objective   {} {} ({})",
            c.objective.direction,
            c.objective.metric,
            match c.objective.min_gain {
                None => "min gain calibrated from the baseline".to_owned(),
                Some(g) => format!("min gain {}%", trim_fraction(to_fixed(g * 100.0, 2))),
            }
        ),
    ];
    if let Some(p) = &c.pack {
        lines.push(format!("  pack        {p}"));
    }
    if let Some(v) = &c.objective.value {
        lines.push(format!(
            "  value       ${} per {} per {} ({})",
            js_number_to_string(v.usd_per_unit),
            v.unit.as_deref().unwrap_or("unit"),
            v.period,
            v.basis
        ));
    }
    let e = &c.evaluator;
    lines.push(format!(
        "  evaluator   {}: {}{}",
        e.kind,
        e.run,
        e.template
            .as_ref()
            .map_or(String::new(), |t| format!("  (template {t})"))
    ));
    lines.push(format!(
        "              ref {}, {} on {}{}",
        e.git_ref,
        match e.repeats {
            None => "repeats calibrated from the baseline".to_owned(),
            Some(r) => format!("{r} repeats"),
        },
        e.hardware,
        match &e.holdout {
            Some(h) => format!(", holdout {h}"),
            None => ", no holdout".to_owned(),
        }
    ));
    lines.push(format!(
        "  guardrails  {}",
        if c.guardrails.is_empty() {
            "none".to_owned()
        } else {
            c.guardrails
                .iter()
                .map(guardrail_name)
                .collect::<Vec<_>>()
                .join("; ")
        }
    ));
    lines.push(format!(
        "  protected   {}",
        if c.protected.is_empty() {
            "none listed".to_owned()
        } else {
            c.protected.join(", ")
        }
    ));
    let mut seen = Vec::new();
    for entry in &auto.entries {
        if !matches!(
            entry.source,
            ProtectedSource::User | ProtectedSource::Install
        ) && !seen.contains(&entry.glob)
        {
            seen.push(entry.glob.clone());
        }
    }
    lines.push(format!(
        "              automatic: {}, lockfiles and install config",
        seen.join(", ")
    ));
    if let Some(env) = &c.environment {
        let mut parts = Vec::new();
        if let Some(v) = &env.image {
            parts.push(format!("image {v}"));
        }
        if let Some(v) = &env.machine {
            parts.push(format!("machine {v}"));
        }
        if let Some(v) = &env.region {
            parts.push(format!("region {v}"));
        }
        if let Some(v) = &env.network {
            parts.push(format!("network {}", v.join(",")));
        }
        if !parts.is_empty() {
            lines.push(format!("  environment {}", parts.join("; ")));
        }
    }
    if let Some(infra) = &c.infra {
        lines.push(format!(
            "  infra       {infra} (read from the protected ref only)"
        ));
    }
    lines.push(format!(
        "  agent       {}; instance types {}",
        c.agent.instructions,
        c.agent.allowed_instance_types.join(", ")
    ));
    io.out(&(lines.join("\n") + "\n"));
    0
}
