//! `ratchet.yaml` text -> the summary the screens show (port of `validateConfigText` in `mappers.ts`). Pure, so the
//! `ratchet` CLI uses it too.

use ratchet_core::bench::calibrate::calibration::FALLBACK_GAIN;
use ratchet_core::config::Guardrail;
use ratchet_core::{ProtectedConfigInput, effective_protected, get_template, parse_config};

use super::*;

pub fn validate_config_text(text: &str) -> ValidateConfigResult {
    let c = match parse_config(text) {
        Ok(c) => c,
        Err(errors) => {
            return ValidateConfigResult {
                ok: false,
                summary: None,
                errors,
            };
        }
    };
    let holdout = c.evaluator.holdout.clone().or_else(|| {
        c.evaluator
            .template
            .as_deref()
            .and_then(get_template)
            .and_then(|t| t.holdout)
    });
    let summary = ConfigSummary {
        name: c.objective.name.clone(),
        pack: c.pack.clone(),
        metric: c.objective.metric.clone(),
        direction: c.objective.direction,
        // Absent in ratchet.yaml means "calibrated from the baseline"; the fallback stands in here.
        min_gain: c.objective.min_gain.unwrap_or(FALLBACK_GAIN),
        evaluator: EvaluatorSummary {
            kind: c.evaluator.kind,
            git_ref: c.evaluator.git_ref.clone(),
            run: c.evaluator.run.clone(),
            repeats: c.evaluator.repeats,
            hardware: c.evaluator.hardware.clone(),
            holdout,
            template: c.evaluator.template.clone(),
        },
        environment: EnvironmentSummary {
            image: c.environment.as_ref().and_then(|e| e.image.clone()),
            machine: c.evaluator.hardware.clone(),
            region: c.environment.as_ref().and_then(|e| e.region.clone()),
            network: c
                .environment
                .as_ref()
                .and_then(|e| e.network.clone())
                .unwrap_or_default(),
        },
        value: c.objective.value.as_ref().map(|v| ValueSummary {
            usd_per_unit: v.usd_per_unit,
            period: v.period,
            basis: v.basis,
            unit: v.unit.clone(),
        }),
        guardrails: c
            .guardrails
            .iter()
            .map(|g| match g {
                Guardrail::Command(c) => format!("run: {}", c.run),
                Guardrail::Threshold(t) => format!("{} {}", t.metric, t.raw),
            })
            .collect(),
        protected_globs: c.protected.clone(),
        protected_entries: effective_protected(&ProtectedConfigInput::from(&c), None)
            .entries
            .into_iter()
            .map(|e| ProtectedEntryVm {
                glob: e.glob,
                reason: e.reason,
            })
            .collect(),
        instance_types: c.agent.allowed_instance_types.clone(),
    };
    ValidateConfigResult {
        ok: true,
        summary: Some(summary),
        errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_config_has_a_summary() {
        let r = validate_config_text(
            "version: 0\nobjective:\n  metric: http_req_duration.p95\n  direction: minimize\n  min_gain: 0.03\n  value: { usd_per_unit: 1200, period: year, basis: estimated, unit: ms }\nevaluator:\n  run: k6 run bench/k6/search.js\nguardrails:\n  - run: pnpm test:unit\n",
        );
        assert!(r.ok, "{:?}", r.errors);
        let s = r.summary.unwrap();
        assert_eq!(s.metric, "http_req_duration.p95");
        assert_eq!(s.min_gain, 0.03);
        assert!(s.protected_entries.iter().any(|e| e.glob == "ratchet.yaml"));
        assert_eq!(s.value.unwrap().usd_per_unit, 1200.0);
        assert_eq!(s.guardrails[0], "run: pnpm test:unit");
    }

    #[test]
    fn an_invalid_config_lists_its_errors() {
        let r = validate_config_text("version: 0\nobjective: { metric: m, direction: sideways }\n");
        assert!(!r.ok);
        assert!(r.summary.is_none());
        assert!(r.errors.iter().any(|e| e.path == "objective.direction"));
    }
}
