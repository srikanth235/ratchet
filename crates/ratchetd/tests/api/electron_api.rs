//! The contract against the Electron API it replaces: every `RatchetApi` method has a route, and no response can
//! carry a secret (ports of the `ipc.test.ts` / `service.test.ts` / `settings.test.ts` checks that are about the
//! surface rather than the behaviour).

use std::collections::BTreeSet;

use serde_json::Value;

fn doc() -> Value {
    serde_json::to_value(ratchetd::openapi::openapi()).unwrap()
}

/// Every method of the preload API (`RatchetApi` of the retired Electron app) and the operation that
/// replaces it. `None`: dropped on purpose (the native app does it itself).
const RATCHET_API: &[(&str, Option<&str>)] = &[
    ("listOptimizations", Some("listOptimizations")),
    ("getOptimization", Some("getOptimization")),
    ("ratchetHistory", Some("getRatchetHistory")),
    ("winningChange", Some("getWinningChange")),
    ("candidatesWithResults", Some("listCandidates")),
    ("spendBreakdown", Some("getSpend")),
    ("recentActivity", Some("listActivity")),
    ("agentEvents", Some("listAgentEvents")),
    ("validateConfig", Some("validateConfig")),
    ("preflight", Some("runPreflight")),
    ("getSettings", Some("getSettings")),
    ("environments", Some("listEnvironments")),
    ("githubStatus", Some("getGithubStatus")),
    ("githubSignIn", Some("startGithubSignIn")),
    ("githubSignOut", Some("signOutOfGithub")),
    ("listRepos", Some("listRepos")),
    ("inspectRepo", Some("inspectRepo")),
    ("pickLocalFolder", None), // NSOpenPanel in the app; `inspectRepo` takes the chosen path
    ("awsProfiles", Some("listAwsProfiles")),
    ("detectHarnesses", Some("listHarnesses")),
    ("redetectHarnesses", Some("detectHarnesses")),
    ("listTemplates", Some("listTemplates")),
    ("proposeTemplate", Some("proposeTemplate")),
    ("createOptimization", Some("createOptimization")),
    ("startOptimization", Some("startOptimization")),
    ("stopOptimization", Some("stopOptimization")),
    ("pauseAgent", Some("pauseAgent")),
    ("resumeAgent", Some("resumeAgent")),
    ("messageAgent", Some("messageAgent")),
    ("openPullRequest", Some("openPullRequest")),
    ("exportReport", Some("getReport")), // the save dialog is the app's; the daemon serves the markdown
    ("endSandbox", Some("endEnvironment")),
    ("setAwsProfile", Some("setAwsProfile")),
    ("setSecret", Some("setSecret")),
    ("clearSecret", Some("clearSecret")),
    ("saveLiveSettings", Some("saveAdvancedSettings")),
    ("testConnection", Some("testConnection")),
    // the Overview and the connectors (`packages/desktop/src/shared/{overview,connectors}.ts`)
    ("overview", Some("getOverview")),
    ("listConnectors", Some("listConnectors")),
    ("refreshConnectors", Some("refreshConnectors")),
    ("addGithubConnector", Some("addGithubConnector")),
    (
        "startGithubConnectorSignIn",
        Some("startGithubConnectorSignIn"),
    ),
    (
        "cancelGithubConnectorSignIn",
        Some("cancelGithubConnectorSignIn"),
    ),
    ("addAwsConnector", Some("addAwsConnector")),
    ("addAgentConnector", Some("addAgentConnector")),
    ("updateConnector", Some("updateConnector")),
    ("removeConnector", Some("removeConnector")),
    ("setDefaultConnector", Some("setDefaultConnector")),
    ("testConnector", Some("testConnector")),
    ("getRepoConnections", Some("getConnectorSelection")),
    ("saveRepoConnections", Some("saveConnectorSelection")),
    ("onEvent", Some("streamEvents")),
];

#[test]
fn every_method_of_the_electron_api_has_a_route() {
    let d = doc();
    let ops: BTreeSet<String> = d["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|i| i.as_object().unwrap().values())
        .map(|op| op["operationId"].as_str().unwrap().to_owned())
        .collect();
    for (method, op) in RATCHET_API {
        if let Some(op) = op {
            assert!(
                ops.contains(*op),
                "RatchetApi.{method} has no operation {op}"
            );
        }
    }
}

/// What a response can carry: every schema reachable from a response body.
fn response_schemas(d: &Value) -> BTreeSet<String> {
    fn collect(v: &Value, schemas: &Value, seen: &mut BTreeSet<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    if k == "$ref" {
                        let r = x.as_str().unwrap();
                        let name = r.rsplit('/').next().unwrap().to_owned();
                        if r.contains("/schemas/") && seen.insert(name.clone()) {
                            collect(&schemas[&name], schemas, seen);
                        }
                    } else {
                        collect(x, schemas, seen);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| collect(x, schemas, seen)),
            _ => {}
        }
    }
    let mut seen = BTreeSet::new();
    for item in d["paths"].as_object().unwrap().values() {
        for op in item.as_object().unwrap().values() {
            collect(&op["responses"], &d["components"]["schemas"], &mut seen);
        }
    }
    seen
}

/// "No IPC response, event or error carries a stored key or token": a response schema may say that a secret is
/// stored (a boolean) or show a hint (the last characters), but never has a field that could hold one.
#[test]
fn no_response_can_carry_a_secret() {
    let d = doc();
    let reachable = response_schemas(&d);
    assert!(
        reachable.contains("SettingsVm")
            && reachable.contains("ConnectorVm")
            && reachable.contains("ApiEvent")
    );
    let mut found = Vec::new();
    for name in &reachable {
        // the properties of the schema, and of each member of a union (`ConnectorVm`, `ApiEvent`)
        let schema_of = &d["components"]["schemas"][name];
        let members = std::iter::once(schema_of)
            .chain(schema_of["oneOf"].as_array().into_iter().flatten())
            .filter_map(|m| m["properties"].as_object());
        for (p, schema) in members.flatten() {
            let lower = p.to_lowercase();
            // `key` alone is a metric key; a count, a flag or an object of flags (`SecretsPresent`) holds no secret
            let harmless = p == "key"
                || schema.get("$ref").is_some()
                || ["boolean", "integer"].iter().any(|t| schema["type"] == *t);
            if ["key", "token", "secret", "password"]
                .iter()
                .any(|w| lower.contains(w))
                && !harmless
            {
                found.push(format!("{name}.{p}"));
            }
        }
    }
    found.sort();
    // the only such string in a response is the visible tail of a key ("····4f2a")
    found.dedup();
    assert_eq!(found, ["ConnectorVm.keyHint"], "{found:?}");
    // the request side carries them: that is how they get in, and they never come back
    for input in [
        "SetSecretInput",
        "AddGithubConnectorInput",
        "AddAgentConnectorInput",
    ] {
        assert!(!reachable.contains(input), "{input} must not be a response");
    }
    // and nothing applies or destroys infrastructure: there is no route for it
    let paths: Vec<String> = d["paths"].as_object().unwrap().keys().cloned().collect();
    assert!(
        paths
            .iter()
            .all(|p| !p.contains("apply") && !p.contains("destroy") && !p.contains("tofu")),
        "{paths:?}"
    );
}
