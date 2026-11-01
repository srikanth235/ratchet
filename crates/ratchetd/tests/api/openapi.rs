//! The OpenAPI document: the committed file is current, the contract is snapshotted, and the document is
//! well-formed (unique operation ids, no dangling references, camelCase wire names).

use std::collections::BTreeSet;

use ratchetd::openapi;
use serde_json::Value;

fn doc() -> Value {
    serde_json::to_value(openapi::openapi()).unwrap()
}

#[test]
fn the_committed_openapi_file_is_current() {
    let path = openapi::committed_path();
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        on_disk == openapi::openapi_yaml(),
        "{} is stale. Regenerate it with `cargo run -p ratchetd -- openapi` and commit the result \
         (the Swift client is generated from this file).",
        path.display()
    );
}

#[test]
fn the_yaml_is_the_same_document_as_the_json() {
    let yaml: Value = serde_norway::from_str(&openapi::openapi_yaml()).unwrap();
    let json: Value = serde_json::from_str(&openapi::openapi_json()).unwrap();
    assert_eq!(yaml, json);
}

#[test]
fn the_route_table_is_snapshotted() {
    insta::assert_snapshot!("routes", openapi::route_table().join("\n"));
}

#[test]
fn the_schema_names_are_snapshotted() {
    let d = doc();
    let names: Vec<&String> = d["components"]["schemas"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    insta::assert_yaml_snapshot!("schemas", names);
}

#[test]
fn the_events_union_is_snapshotted() {
    let d = doc();
    insta::assert_json_snapshot!("api_event", d["components"]["schemas"]["ApiEvent"]);
}

#[test]
fn every_operation_is_documented() {
    let d = doc();
    let declared: BTreeSet<&str> = d["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    let mut ids = BTreeSet::new();
    let mut n = 0;
    for (path, item) in d["paths"].as_object().unwrap() {
        assert!(path.starts_with("/v1/"), "{path}");
        for (method, op) in item.as_object().unwrap() {
            n += 1;
            let at = format!("{} {path}", method.to_uppercase());
            let id = op["operationId"]
                .as_str()
                .unwrap_or_else(|| panic!("{at}: no operationId"));
            assert!(
                ids.insert(id.to_owned()),
                "{at}: duplicate operationId {id}"
            );
            assert!(
                id.chars().next().unwrap().is_lowercase() && !id.contains('_'),
                "{at}: {id} is not camelCase"
            );
            assert!(
                op["summary"].as_str().is_some_and(|s| !s.is_empty()),
                "{at}: no summary"
            );
            let tags = op["tags"]
                .as_array()
                .unwrap_or_else(|| panic!("{at}: no tag"));
            assert_eq!(tags.len(), 1, "{at}");
            assert!(
                declared.contains(tags[0].as_str().unwrap()),
                "{at}: undeclared tag {}",
                tags[0]
            );
            let r = op["responses"].as_object().unwrap();
            assert!(
                r.contains_key("401") && r.contains_key("default"),
                "{at}: error responses"
            );
            assert!(
                r.keys().any(|k| k.starts_with('2')),
                "{at}: no success response"
            );
            for (code, resp) in r {
                assert!(
                    resp.get("$ref").is_some()
                        || resp["description"].as_str().is_some_and(|d| !d.is_empty()),
                    "{at}: response {code} has no description"
                );
            }
            if path.contains('{') {
                assert!(r.contains_key("404"), "{at}: path parameters need a 404");
            }
            if tags[0].as_str().unwrap().contains("provisional") {
                assert!(
                    op["summary"].is_string(),
                    "{at}: provisional routes stay documented"
                );
            }
        }
    }
    assert!(n > 50);
    // every declared tag is used
    let used: BTreeSet<&str> = d["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|i| i.as_object().unwrap().values())
        .map(|op| op["tags"][0].as_str().unwrap())
        .collect();
    assert_eq!(declared, used, "declared tags and used tags differ");
    // the bearer scheme guards everything
    assert_eq!(d["security"][0]["bearer"], serde_json::json!([]));
    assert_eq!(d["components"]["securitySchemes"]["bearer"]["type"], "http");
}

fn refs(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                if k == "$ref" {
                    out.insert(x.as_str().unwrap().to_owned());
                } else {
                    refs(x, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
        _ => {}
    }
}

#[test]
fn there_are_no_dangling_references() {
    let d = doc();
    let mut all = BTreeSet::new();
    refs(&d, &mut all);
    assert!(all.len() > 100);
    for r in all {
        let (kind, name) = r
            .strip_prefix("#/components/")
            .and_then(|r| r.split_once('/'))
            .unwrap_or_else(|| panic!("odd reference {r}"));
        assert!(
            d["components"][kind].get(name).is_some(),
            "{r} points at nothing"
        );
    }
}

/// The wire is camelCase. A snake_case property in a schema means serde and utoipa disagree about a name (for
/// example `rename_all_fields`, which utoipa ignores): the Swift client would be generated wrong.
#[test]
fn schema_property_names_are_camel_case() {
    fn walk(v: &Value, at: &str, bad: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(Value::Object(props)) = o.get("properties") {
                    for k in props.keys() {
                        if k.contains('_')
                            || k.contains('-')
                            || k.chars().next().is_some_and(char::is_uppercase)
                        {
                            bad.push(format!("{at}.{k}"));
                        }
                    }
                }
                for (k, x) in o {
                    walk(x, &format!("{at}/{k}"), bad);
                }
            }
            Value::Array(a) => a
                .iter()
                .enumerate()
                .for_each(|(i, x)| walk(x, &format!("{at}[{i}]"), bad)),
            _ => {}
        }
    }
    let d = doc();
    let mut bad = Vec::new();
    walk(&d["components"]["schemas"], "", &mut bad);
    assert!(bad.is_empty(), "non-camelCase properties: {bad:#?}");
}

#[test]
fn enums_have_stable_wire_values() {
    let d = doc();
    let e = |n: &str| -> Vec<String> {
        d["components"]["schemas"][n]["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{n} is not a string enum"))
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(e("KpiRange"), ["7d", "30d", "90d"]);
    assert_eq!(e("UiStatus").last().unwrap(), "pr-open");
    assert_eq!(
        e("CandidateVerdict"),
        [
            "pending",
            "verified",
            "rejected",
            "failed_confirm",
            "blocked"
        ]
    );
    assert_eq!(e("AgentKind"), ["claude-code", "codex", "grok", "mock"]);
    assert_eq!(e("HarnessAuth"), ["signed-in", "api-key", "signed-out"]);
    assert_eq!(e("EnvironmentScope"), ["none", "single", "full"]);
    assert_eq!(e("ErrorCode").len(), 8);
}
