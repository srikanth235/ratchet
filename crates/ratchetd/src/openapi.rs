//! The OpenAPI document, generated from the route annotations and the `ToSchema` types.
//!
//! `ratchetd openapi` writes it to [`COMMITTED_PATH`]; a test fails when the committed file is stale. The Swift
//! client is generated from that file with `swift-openapi-generator`.

use std::path::PathBuf;

use utoipa::OpenApi as _;
use utoipa::openapi::path::{Operation, PathItem};
use utoipa::openapi::response::{Response, ResponseBuilder};
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
use utoipa::openapi::{ContentBuilder, InfoBuilder, OpenApi, RefOr, ServerBuilder};

use crate::api::API_VERSION;

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(crate::api::SecretName)), tags(
    (name = "status", description = "The daemon itself."),
    (name = "events", description = "The live event stream (server-sent events)."),
    (name = "optimizations", description = "Optimizations, their candidates, ratchet history and spend."),
    (name = "agents", description = "Agents: log, pause, resume, messages."),
    (name = "reports", description = "Reports and pull requests."),
    (name = "config", description = "ratchet.yaml validation and preflight."),
    (name = "settings", description = "Settings, secrets, connection tests."),
    (name = "github", description = "GitHub sign-in, repos, inspecting a repo."),
    (name = "aws", description = "AWS profiles."),
    (name = "harnesses", description = "Detected agent CLIs."),
    (name = "templates", description = "Evaluator templates (Benchmarks)."),
    (name = "environments", description = "Sandboxes: what is running, end now, receipts."),
    (name = "overview (provisional)", description = "PROVISIONAL. Overview: spent, value locked in, return, needs you."),
    (name = "connectors (provisional)", description = "PROVISIONAL. Saved GitHub / AWS / agent connectors and per-repo selection."),
    (name = "environment drafts", description = "Dynamic environments: a setup agent drafts the environment a goal needs, the user asks for changes and approves, the files are committed and locked."),
))]
struct ApiDoc;

/// The document with every route, the bearer scheme and the common error responses.
pub fn openapi() -> OpenApi {
    let base = ApiDoc::openapi();
    let (_, mut api) = crate::routes::api_router().split_for_parts();
    // The tag list and the schemas that only parameters use (the router starts from an empty document).
    api.merge(base);
    finish(&mut api);
    api
}

fn finish(api: &mut OpenApi) {
    api.info = InfoBuilder::new()
        .title("ratchetd")
        .version(API_VERSION)
        .description(Some(
            "The Ratchet daemon API: REST resources over HTTP on a local unix socket, plus one server-sent-event \
             stream. Every route needs `Authorization: Bearer <token>` (the per-boot token the daemon writes next \
             to its socket). Timestamps are epoch milliseconds, money is USD. Routes tagged `(provisional)` mirror \
             features that are still being built and may change.",
        ))
        .build();
    api.servers = Some(vec![
        ServerBuilder::new()
            .url("http://localhost")
            .description(Some("The daemon, over its unix socket (`ratchetd.sock`)."))
            .build(),
    ]);
    let components = api.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        "bearer",
        SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
    );
    api.security = Some(vec![utoipa::openapi::security::SecurityRequirement::new(
        "bearer",
        Vec::<String>::new(),
    )]);

    let error = |description: &str| -> Response {
        ResponseBuilder::new()
            .description(description)
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Some(RefOr::Ref(utoipa::openapi::Ref::from_schema_name(
                        "ApiError",
                    ))))
                    .build(),
            )
            .build()
    };
    let shared = [
        ("Unauthorized", "Missing or wrong bearer token."),
        (
            "BadRequest",
            "The request is malformed or a field is invalid (see `fields`).",
        ),
        ("NotFound", "No such resource."),
        ("Failure", "Any other failure; `code` says which."),
    ];
    let components = api.components.get_or_insert_with(Default::default);
    for (name, description) in shared {
        components
            .responses
            .insert(name.to_owned(), RefOr::T(error(description)));
    }
    let shared_ref = |name: &str| RefOr::Ref(utoipa::openapi::Ref::from_response_name(name));
    for item in api.paths.paths.values_mut() {
        for_each_operation(item, |op, has_path_param| {
            let has_body = op.request_body.is_some();
            let has_query = op.parameters.as_ref().is_some_and(|p| {
                p.iter()
                    .any(|p| matches!(p.parameter_in, utoipa::openapi::path::ParameterIn::Query))
            });
            let r = &mut op.responses.responses;
            for v in r.values_mut() {
                if let RefOr::T(resp) = v
                    && resp.description.is_empty()
                {
                    resp.description = "OK".to_owned();
                }
            }
            r.entry("401".into())
                .or_insert_with(|| shared_ref("Unauthorized"));
            if has_body || has_query {
                r.entry("400".into())
                    .or_insert_with(|| shared_ref("BadRequest"));
            }
            if has_path_param {
                r.entry("404".into())
                    .or_insert_with(|| shared_ref("NotFound"));
            }
            r.entry("default".into())
                .or_insert_with(|| shared_ref("Failure"));
        });
    }
}

fn for_each_operation(item: &mut PathItem, mut f: impl FnMut(&mut Operation, bool)) {
    let mut ops: Vec<&mut Operation> = Vec::new();
    for op in [
        item.get.as_mut(),
        item.put.as_mut(),
        item.post.as_mut(),
        item.delete.as_mut(),
        item.patch.as_mut(),
    ]
    .into_iter()
    .flatten()
    {
        ops.push(op);
    }
    for op in ops {
        let has_path_param = op.parameters.as_ref().is_some_and(|p| {
            p.iter()
                .any(|p| matches!(p.parameter_in, utoipa::openapi::path::ParameterIn::Path))
        });
        f(op, has_path_param);
    }
}

/// The document as YAML (what is committed).
pub fn openapi_yaml() -> String {
    let mut y = openapi().to_yaml().unwrap_or_default();
    if !y.ends_with('\n') {
        y.push('\n');
    }
    y
}

/// The document as pretty JSON (what `/v1/openapi.json` serves).
pub fn openapi_json() -> String {
    openapi().to_pretty_json().unwrap_or_default()
}

/// Where the generated document is committed: `api/openapi.yaml` at the repository root.
pub const COMMITTED_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../api/openapi.yaml");

pub fn committed_path() -> PathBuf {
    PathBuf::from(COMMITTED_PATH)
}

/// `METHOD /path operationId`, sorted: a short, reviewable index of the contract.
pub fn route_table() -> Vec<String> {
    let api = openapi();
    let mut rows = Vec::new();
    for (path, item) in &api.paths.paths {
        for (m, op) in [
            ("GET", &item.get),
            ("PUT", &item.put),
            ("POST", &item.post),
            ("DELETE", &item.delete),
            ("PATCH", &item.patch),
        ] {
            if let Some(op) = op {
                rows.push(format!(
                    "{m} {path} {}",
                    op.operation_id.as_deref().unwrap_or("?")
                ));
            }
        }
    }
    rows
}
