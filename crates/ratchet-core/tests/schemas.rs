//! The data types are `schemars::JsonSchema` always, and `utoipa::ToSchema` behind the `utoipa` feature: the
//! OpenAPI contract crate generates the API description from them.

use ratchet_core::bench::Calibration;
use ratchet_core::config::RatchetConfig;
use ratchet_core::ports::{EngineEvent, RunnerMessage, SandboxOutputs};
use ratchet_core::ratchet::{RatchetInput, RatchetOutcome};
use ratchet_core::rows::{ActivityItem, CandidateWithResults, Optimization, SpendBreakdown};
use ratchet_core::templates::TemplateInfo;
use schemars::schema_for;
use serde_json::Value;

fn props(schema: &Value) -> Vec<String> {
    schema["properties"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn json_schemas_use_the_wire_names() {
    let outcome = serde_json::to_value(schema_for!(RatchetOutcome)).unwrap();
    assert!(
        props(&outcome).contains(&"candidateValue".to_owned()),
        "{outcome}"
    );
    assert!(
        outcome["properties"]["verdict"]
            .to_string()
            .contains("Verdict"),
        "{outcome}"
    );
    assert!(
        outcome["$defs"]["Verdict"]
            .to_string()
            .contains("pending_confirm"),
        "{outcome}"
    );

    let input = serde_json::to_value(schema_for!(RatchetInput)).unwrap();
    assert!(props(&input).contains(&"protectedViolations".to_owned()));

    let config = serde_json::to_value(schema_for!(RatchetConfig)).unwrap();
    let p = props(&config);
    for k in [
        "version",
        "objective",
        "evaluator",
        "guardrails",
        "protected",
        "agent",
    ] {
        assert!(p.contains(&k.to_owned()), "{k} in {p:?}");
    }

    let opt = serde_json::to_value(schema_for!(Optimization)).unwrap();
    let p = props(&opt);
    for k in [
        "baseRef",
        "minGain",
        "budgetUsd",
        "timeLimitS",
        "valueUsdPerUnit",
        "sandboxId",
    ] {
        assert!(p.contains(&k.to_owned()), "{k} in {p:?}");
    }

    let cal = serde_json::to_value(schema_for!(Calibration)).unwrap();
    assert!(props(&cal).contains(&"repeatsCapped".to_owned()));
    let tpl = serde_json::to_value(schema_for!(TemplateInfo)).unwrap();
    assert!(props(&tpl).contains(&"evaluatorKind".to_owned()));
    let out = serde_json::to_value(schema_for!(SandboxOutputs)).unwrap();
    assert!(props(&out).contains(&"agentRolePath".to_owned()));
}

#[test]
fn tagged_enums_describe_every_variant() {
    for (name, schema) in [
        (
            "EngineEvent",
            serde_json::to_value(schema_for!(EngineEvent)).unwrap(),
        ),
        (
            "RunnerMessage",
            serde_json::to_value(schema_for!(RunnerMessage)).unwrap(),
        ),
        (
            "ActivityItem",
            serde_json::to_value(schema_for!(ActivityItem)).unwrap(),
        ),
    ] {
        let text = schema.to_string();
        assert!(
            text.contains("\"type\"") || text.contains("\"kind\""),
            "{name}: {text}"
        );
    }
    let engine = schema_for!(EngineEvent);
    let text = serde_json::to_string(&engine).unwrap();
    for tag in [
        "optimization",
        "agent-event",
        "candidate",
        "ratchet",
        "spend",
        "notice",
        "run-active",
        "sandbox",
    ] {
        assert!(text.contains(&format!("\"{tag}\"")), "{tag}");
    }
}

#[test]
fn flattened_records_keep_their_fields() {
    let c = serde_json::to_value(schema_for!(CandidateWithResults)).unwrap();
    let text = c.to_string();
    for k in [
        "agentKind",
        "benchValue",
        "movedRatchet",
        "verdictAt",
        "submittedAt",
    ] {
        assert!(text.contains(k), "{k} in {text}");
    }
    let s = serde_json::to_value(schema_for!(SpendBreakdown)).unwrap();
    assert!(s.to_string().contains("byCategory"));
}

#[cfg(feature = "utoipa")]
mod openapi {
    use super::*;
    use ratchet_core::ids::OptimizationId;
    use ratchet_core::ports::SandboxPlan;
    use ratchet_core::rows::{Agent, BenchStats, Candidate, RatchetMove, SpendEvent};
    use utoipa::OpenApi;

    #[derive(OpenApi)]
    #[openapi(components(schemas(
        RatchetConfig,
        RatchetInput,
        RatchetOutcome,
        Calibration,
        Optimization,
        Agent,
        Candidate,
        RatchetMove,
        SpendEvent,
        SpendBreakdown,
        BenchStats,
        ActivityItem,
        CandidateWithResults,
        EngineEvent,
        RunnerMessage,
        SandboxOutputs,
        SandboxPlan,
        TemplateInfo,
        OptimizationId,
    )))]
    struct Contract;

    #[test]
    fn the_domain_types_build_an_openapi_document() {
        let doc = serde_json::to_value(Contract::openapi()).unwrap();
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        for name in [
            "RatchetConfig",
            "RatchetOutcome",
            "Optimization",
            "Candidate",
            "EngineEvent",
            "RunnerMessage",
            "TemplateInfo",
            "OptimizationId",
        ] {
            assert!(
                schemas.contains_key(name),
                "{name} missing from {:?}",
                schemas.keys().collect::<Vec<_>>()
            );
        }
        let opt = schemas["Optimization"]["properties"].as_object().unwrap();
        assert!(opt.contains_key("minGain") && opt.contains_key("sandboxId"));
        assert_eq!(schemas["OptimizationId"]["type"], "string");
        // referenced types are collected into the document too
        assert!(
            schemas["Verdict"].to_string().contains("pending_confirm"),
            "{}",
            schemas["Verdict"]
        );
    }
}
