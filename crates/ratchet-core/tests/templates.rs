//! Port of `packages/core/src/templates/templates.test.ts`.
//!
//! The per-template cases of `describe.each(TEMPLATE_IDS)` are loops inside one test each, so a failure names
//! the template. The evaluator scripts are the unchanged `.mjs` files of `packages/core/templates`; their
//! tests run them with `node` and are skipped (with a note) when `node` is not installed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use ratchet_core::bench::{extract_last_json_object, lookup_path};
use ratchet_core::config::parse_config;
use ratchet_core::enums::{Direction, EvaluatorKind, TemplateCategory, TemplateKind};
use ratchet_core::protected::find_protected_violations;
use ratchet_core::protected_set::{ProtectedConfigInput, effective_protected};
use ratchet_core::templates::{
    RenderTemplateOptions, RenderedTemplate, TemplateFilter, TemplateInfo, build_template_proposal,
    evaluator_template_ids, get_template, list_templates, render_template, resolve_template_id,
    template_file_paths, template_files, template_ids_with_files,
};
use serde_json::{Map, Value, json};

const TEMPLATE_IDS: [&str; 7] = [
    "http-latency",
    "cli-cold-start",
    "test-suite-time",
    "build-size",
    "memory",
    "forecast-backtest",
    "llm-eval",
];

fn ids(v: Vec<TemplateInfo>) -> Vec<String> {
    v.into_iter().map(|t| t.id).collect()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn render(id: &str) -> RenderedTemplate {
    render_template(id, &RenderTemplateOptions::default()).unwrap()
}

fn kind_filter(kind: TemplateKind) -> TemplateFilter {
    TemplateFilter {
        kind: Some(kind),
        category: None,
    }
}

// ---------------------------------------------------------------- registry

// "has the seven templates of the spec table, then the three demos"
#[test]
fn has_the_seven_templates_of_the_spec_table_then_the_three_demos() {
    assert_eq!(
        ids(list_templates(kind_filter(TemplateKind::Template))),
        TEMPLATE_IDS
    );
    assert_eq!(
        ids(list_templates(kind_filter(TemplateKind::Demo))),
        ["demo-search", "demo-cli", "demo-forecast"]
    );
    let mut all: Vec<String> = TEMPLATE_IDS.iter().map(|s| (*s).to_owned()).collect();
    all.extend(["demo-search", "demo-cli", "demo-forecast"].map(String::from));
    assert_eq!(ids(list_templates(TemplateFilter::default())), all);
    assert_eq!(evaluator_template_ids(), TEMPLATE_IDS);
}

// "carries the metadata the gallery shows"
#[test]
fn carries_the_metadata_the_gallery_shows() {
    let spec: [(&str, &str, Option<&str>); 7] = [
        ("http-latency", "c7i.2xlarge", Some("hidden request mix")),
        ("cli-cold-start", "c7i.large", Some("hidden invocations")),
        ("test-suite-time", "c7i.2xlarge", None),
        ("build-size", "c7i.xlarge", None),
        ("memory", "c7i.xlarge", Some("hidden workload")),
        (
            "forecast-backtest",
            "c7i.xlarge",
            Some("most recent periods"),
        ),
        ("llm-eval", "c7i.large", Some("hidden eval set")),
    ];
    for t in list_templates(kind_filter(TemplateKind::Template)) {
        let (_, machine, holdout) = spec.iter().find(|(id, ..)| *id == t.id).unwrap();
        assert_eq!(
            (t.machine.as_str(), t.holdout.as_deref()),
            (*machine, *holdout),
            "{}",
            t.id
        );
        assert!(!t.title.is_empty() && !t.description.is_empty() && !t.measures.is_empty());
        assert_eq!(t.files[0], "ratchet.yaml");
        assert!(t.files.contains(&"RATCHET.md".to_owned()), "{}", t.id);
    }
}

// "demos are metadata only and point at the examples"
#[test]
fn demos_are_metadata_only_and_point_at_the_examples() {
    for t in list_templates(kind_filter(TemplateKind::Demo)) {
        assert!(t.files.is_empty());
        assert!(
            repo_root()
                .join(t.example.as_deref().unwrap())
                .join("ratchet.yaml")
                .exists(),
            "{}",
            t.id
        );
        let e = render_template(&t.id, &RenderTemplateOptions::default()).unwrap_err();
        assert!(e.to_string().contains("demo"), "{e}");
    }
}

// "filters by category and resolves the legacy pack names"
#[test]
fn filters_by_category_and_resolves_the_legacy_pack_names() {
    let by = |c| {
        ids(list_templates(TemplateFilter {
            kind: None,
            category: Some(c),
        }))
    };
    assert_eq!(by(TemplateCategory::Llm), ["llm-eval"]);
    assert_eq!(
        by(TemplateCategory::Forecasting),
        ["forecast-backtest", "demo-forecast"]
    );
    assert_eq!(resolve_template_id("perf"), "cli-cold-start");
    assert_eq!(resolve_template_id("forecasting"), "forecast-backtest");
    assert_eq!(resolve_template_id("http-latency"), "http-latency");
    assert_eq!(get_template("perf").unwrap().id, "cli-cold-start");
    assert_eq!(get_template("nope"), None);
}

// "the list is a copy: callers cannot corrupt the registry"
#[test]
fn the_list_is_a_copy() {
    let mut a = list_templates(TemplateFilter::default());
    a[0].title = "x".into();
    a[0].objective.metric = "y".into();
    let fresh = list_templates(TemplateFilter::default());
    assert_ne!(fresh[0].title, "x");
    assert_ne!(fresh[0].objective.metric, "y");
}

// "the embedded files are in sync with packages/core/templates (run scripts/gen-templates.mjs)"
// The files are embedded with include_dir!; this checks the embedded copy against the files on disk.
#[test]
fn the_embedded_files_are_in_sync_with_the_template_directory() {
    let dir = repo_root().join("packages/core/templates");
    let mut on_disk = BTreeSet::new();
    fn walk(dir: &Path, base: &Path, out: &mut BTreeSet<(String, String)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert((rel, std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    walk(&dir, &dir, &mut on_disk);
    let mut embedded = BTreeSet::new();
    for id in template_ids_with_files() {
        for (path, content) in template_files(&id) {
            embedded.insert((format!("{id}/{path}"), content.to_owned()));
        }
    }
    assert_eq!(embedded, on_disk);
    let mut got = template_ids_with_files();
    got.sort();
    let mut want: Vec<String> = TEMPLATE_IDS.iter().map(|s| (*s).to_owned()).collect();
    want.sort();
    assert_eq!(got, want);
}

// ---------------------------------------------------------------- describe.each(TEMPLATE_IDS)

// "generates a ratchet.yaml that passes parseConfig"
#[test]
fn each_template_generates_a_ratchet_yaml_that_passes_parse_config() {
    for id in TEMPLATE_IDS {
        let rendered = render(id);
        let yaml = &rendered
            .files
            .iter()
            .find(|f| f.path == "ratchet.yaml")
            .unwrap()
            .content;
        let c = parse_config(yaml).unwrap_or_else(|e| {
            panic!(
                "{id}: {}",
                e.iter()
                    .map(|e| format!("{}: {}", e.path, e.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        });
        assert_eq!(c.evaluator.template.as_deref(), Some(id));
        assert_eq!(c.evaluator.hardware, rendered.info.machine);
        assert_eq!(c.evaluator.kind, rendered.info.evaluator_kind);
        assert_eq!(c.objective.metric, rendered.info.objective.metric);
        assert_eq!(c.objective.direction, rendered.info.objective.direction);
        assert_eq!(c.objective.min_gain, None);
        assert_eq!(c.evaluator.repeats, None);
        assert_eq!(
            c.objective.value.as_ref().map(|v| v.basis),
            Some(ratchet_core::enums::ValueBasis::Estimated)
        );
        assert!(!c.guardrails.is_empty());
        assert_eq!(c.agent.instructions, "RATCHET.md");
    }
}

// "writes the evaluator files, the brief, and nothing outside the repo"
#[test]
fn each_template_writes_the_evaluator_files_the_brief_and_nothing_outside_the_repo() {
    for id in TEMPLATE_IDS {
        let rendered = render(id);
        let paths: Vec<String> = rendered.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, template_file_paths(id), "{id}");
        assert_eq!(paths, rendered.info.files, "{id}");
        for p in &paths {
            assert!(!p.starts_with('/') && !p.contains(".."), "{id}: {p}");
        }
        let brief = &rendered
            .files
            .iter()
            .find(|f| f.path == "RATCHET.md")
            .unwrap()
            .content;
        assert!(!brief.contains("{{"), "{id}");
        assert!(brief.contains(&rendered.info.objective.metric), "{id}");
        assert!(brief.contains("Ratchet-Candidate: true"), "{id}");
    }
}

// "every evaluator file is protected automatically"
#[test]
fn each_templates_evaluator_files_are_protected_automatically() {
    for id in TEMPLATE_IDS {
        let rendered = render(id);
        let cfg = parse_config(&rendered.files[0].content).unwrap();
        let eff = effective_protected(&ProtectedConfigInput::from(&cfg), None);
        let evaluator_files: Vec<&str> = rendered
            .files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| p.starts_with("bench/") || p.starts_with("eval/") || p.starts_with("data/"))
            .collect();
        assert!(!evaluator_files.is_empty(), "{id}");
        let hit: Vec<String> = find_protected_violations(&evaluator_files, &eff.globs)
            .into_iter()
            .map(|v| v.path)
            .collect();
        assert_eq!(hit, evaluator_files, "{id}");
        assert_eq!(
            find_protected_violations(&["ratchet.yaml", "pnpm-lock.yaml"], &eff.globs).len(),
            2
        );
        // the code the agents are meant to change stays editable
        assert_eq!(
            find_protected_violations(
                &[
                    "src/app.ts",
                    "src/forecast.mjs",
                    "src/task.mjs",
                    "RATCHET.md"
                ],
                &eff.globs
            ),
            vec![],
            "{id}"
        );
    }
}

// "works without the template: the same run command as the config resolves to"
#[test]
fn each_templates_run_command_points_at_one_of_its_files() {
    for id in TEMPLATE_IDS {
        let rendered = render(id);
        let cfg = parse_config(&rendered.files[0].content).unwrap();
        let script = cfg.evaluator.run.split(' ').nth(1).unwrap();
        assert!(
            rendered.files.iter().any(|f| f.path == script),
            "{id}: {script}"
        );
    }
}

// "honours --metric, --direction and --value-usd and quotes what needs it"
#[test]
fn each_template_honours_metric_direction_and_value_and_quotes_what_needs_it() {
    for id in TEMPLATE_IDS {
        let r = render_template(
            id,
            &RenderTemplateOptions {
                metric: Some("a: b".into()),
                direction: Some(Direction::Maximize),
                value_usd: Some(7.0),
            },
        )
        .unwrap();
        let parsed = parse_config(&r.files[0].content).unwrap_or_else(|e| panic!("{id}: {e:?}"));
        assert_eq!(parsed.objective.metric, "a: b");
        assert_eq!(parsed.objective.direction, Direction::Maximize);
        assert_eq!(
            parsed.objective.value.as_ref().map(|v| v.usd_per_unit),
            Some(7.0)
        );
        assert!(
            r.files
                .iter()
                .find(|f| f.path == "RATCHET.md")
                .unwrap()
                .content
                .contains("raise `a: b`")
        );
    }
}

// "the PR proposal says what was added and how to run a baseline"
#[test]
fn each_templates_pr_proposal_says_what_was_added_and_how_to_run_a_baseline() {
    for id in TEMPLATE_IDS {
        let rendered = render(id);
        let p = build_template_proposal(id, &RenderTemplateOptions::default()).unwrap();
        assert_eq!(p.branch, format!("ratchet/template-{id}"));
        assert_eq!(p.files, rendered.files);
        assert!(p.title.contains(&rendered.info.title));
        for f in &rendered.files {
            assert!(
                p.body.contains(&format!("`{}`", f.path)),
                "{id}: {}",
                f.path
            );
        }
        assert!(p.body.contains("ratchet baseline"));
        assert!(p.body.contains("Run a baseline"));
        assert!(p.body.contains(&rendered.info.objective.metric));
        assert!(p.body.contains("protected automatically"));
    }
}

// ---------------------------------------------------------------- legacy aliases

// "perf and forecasting render the templates that replaced them"
#[test]
fn perf_and_forecasting_render_the_templates_that_replaced_them() {
    assert_eq!(render("perf").info.id, "cli-cold-start");
    assert_eq!(render("forecasting").info.id, "forecast-backtest");
    assert!(render("perf").files[0].content.contains("pack: perf"));
}

// ---------------------------------------------------------------- beyond the TypeScript cases

#[test]
fn an_unknown_template_is_an_error_with_its_name() {
    let e = render_template("nope", &RenderTemplateOptions::default()).unwrap_err();
    assert_eq!(e.to_string(), "unknown template \"nope\"");
    assert_eq!(template_file_paths("nope"), Vec::<String>::new());
    assert_eq!(template_file_paths("demo-search"), Vec::<String>::new());
}

#[test]
fn the_template_ratchet_yaml_text_is_stable() {
    // The exact text for one template; the generator in TypeScript produces the same bytes.
    let yaml = &render("http-latency").files[0].content;
    insta::assert_snapshot!("http_latency_ratchet_yaml", yaml);
    let p = build_template_proposal("memory", &RenderTemplateOptions::default()).unwrap();
    insta::assert_snapshot!("memory_proposal_body", p.body);
}

// ---------------------------------------------------------------- the evaluator scripts run (needs node)

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write(rendered: &RenderedTemplate) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for f in &rendered.files {
        let p = dir.path().join(&f.path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, &f.content).unwrap();
    }
    dir
}

fn run_evaluator(dir: &Path, command: &str, env: &[(&str, String)]) -> Map<String, Value> {
    let mut parts = command.split(' ');
    let bin = parts.next().unwrap();
    let out = Command::new(bin)
        .args(parts)
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    extract_last_json_object(&stdout).unwrap_or_else(|| {
        panic!(
            "evaluator printed no JSON object:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn metric_of(json: &Map<String, Value>, id: &str) -> f64 {
    let path = get_template(id).unwrap().objective.metric;
    lookup_path(&Value::Object(json.clone()), &path)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .unwrap_or_else(|| panic!("no metric {path}"))
}

macro_rules! needs_node {
    () => {
        if !node_available() {
            eprintln!("skipped: node is not installed");
            return;
        }
    };
}

// "http-latency: drives a service and reports percentiles and the error rate"
#[test]
fn http_latency_drives_a_service_and_reports_percentiles_and_the_error_rate() {
    needs_node!();
    let dir = write(&render("http-latency"));
    let d = dir.path();
    std::fs::write(
        d.join("server.mjs"),
        "import http from 'node:http';\nhttp.createServer((req, res) => { if (req.url === '/boom') { res.statusCode = 500; res.end('x'); return; } res.end('ok'); }).listen(Number(process.env.PORT));\n",
    )
    .unwrap();
    let port = 41000
        + (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
            % 2000);
    std::fs::write(
        d.join("bench/http-latency.config.json"),
        json!({"start": "node server.mjs", "port": port, "ready_path": "/", "rate": 40, "duration_s": 1, "warmup_s": 0, "requests": [{"path": "/"}, {"path": "/x"}]}).to_string(),
    )
    .unwrap();
    let out = run_evaluator(d, "node bench/http-latency.mjs", &[]);
    let p95 = metric_of(&out, "http-latency");
    assert!(p95 > 0.0);
    assert_eq!(out["error_rate"], json!(0));
    assert!(out["requests"].as_f64().unwrap() >= 30.0);
    assert!(
        lookup_path(&Value::Object(out.clone()), "http_req_duration.p99")
            .and_then(Value::as_f64)
            .unwrap()
            >= p95
    );

    // the sealed holdout replaces the visible mix; a failing path shows up in error_rate
    std::fs::write(
        d.join("holdout.json"),
        json!({"requests": [{"path": "/boom"}]}).to_string(),
    )
    .unwrap();
    let held = run_evaluator(
        d,
        "node bench/http-latency.mjs",
        &[(
            "RATCHET_TRACE",
            d.join("holdout.json").to_string_lossy().into_owned(),
        )],
    );
    assert_eq!(held["error_rate"], json!(1));
}

// "cli-cold-start: times invocations, counts unexpected exits, reads the holdout invocations"
#[test]
fn cli_cold_start_times_invocations_counts_unexpected_exits_and_reads_the_holdout() {
    needs_node!();
    let dir = write(&render("cli-cold-start"));
    let d = dir.path();
    let env = |extra: Option<String>| {
        let mut e = vec![
            ("BENCH_RUNS", "3".to_owned()),
            ("BENCH_WARMUP", "0".to_owned()),
            ("BENCH_NO_HYPERFINE", "1".to_owned()),
        ];
        if let Some(t) = extra {
            e.push(("RATCHET_TRACE", t));
        }
        e
    };
    let out = run_evaluator(d, "node bench/cold-start.mjs", &env(None));
    assert!(metric_of(&out, "cli-cold-start") > 0.0);
    assert_eq!(
        (&out["invocations"], &out["runs"], &out["error_rate"]),
        (&json!(1), &json!(3), &json!(0))
    );
    std::fs::write(
        d.join("holdout.json"),
        json!({"commands": ["node --version", "node -e \"process.exit(3)\""]}).to_string(),
    )
    .unwrap();
    let held = run_evaluator(
        d,
        "node bench/cold-start.mjs",
        &env(Some(d.join("holdout.json").to_string_lossy().into_owned())),
    );
    assert_eq!(
        (&held["invocations"], &held["runs"], &held["error_rate"]),
        (&json!(2), &json!(6), &json!(0.5))
    );
}

// "test-suite-time: times the test command and reports failures as error_rate"
#[test]
fn test_suite_time_times_the_test_command_and_reports_failures_as_error_rate() {
    needs_node!();
    let dir = write(&render("test-suite-time"));
    let d = dir.path();
    let good = run_evaluator(
        d,
        "node bench/test-time.mjs",
        &[(
            "BENCH_TEST_COMMAND",
            "node -e \"setTimeout(()=>{},120)\"".into(),
        )],
    );
    assert!(metric_of(&good, "test-suite-time") >= 100.0);
    assert_eq!(
        (&good["error_rate"], &good["exit_code"], &good["runs"]),
        (&json!(0), &json!(0), &json!(1))
    );
    let bad = run_evaluator(
        d,
        "node bench/test-time.mjs",
        &[("BENCH_TEST_COMMAND", "node -e \"process.exit(1)\"".into())],
    );
    assert_eq!(
        (&bad["error_rate"], &bad["exit_code"]),
        (&json!(1), &json!(1))
    );
}

// "build-size: runs the build, sizes the output, reports a failed build"
#[test]
fn build_size_runs_the_build_sizes_the_output_and_reports_a_failed_build() {
    needs_node!();
    let dir = write(&render("build-size"));
    let d = dir.path();
    std::fs::write(
        d.join("bench/build-size.config.json"),
        json!({
            "clean": "rm -rf out",
            "build": "node -e \"require('fs').mkdirSync('out/sub',{recursive:true});require('fs').writeFileSync('out/a.js','x'.repeat(2048));require('fs').writeFileSync('out/sub/b.css','y'.repeat(1024));require('fs').writeFileSync('out/ignore.png','z'.repeat(9999))\"",
            "output_dir": "out",
            "include": ["js", "css"]
        })
        .to_string(),
    )
    .unwrap();
    let out = run_evaluator(d, "node bench/build-size.mjs", &[]);
    assert_eq!(metric_of(&out, "build-size"), 3.0);
    assert_eq!((&out["files"], &out["error_rate"]), (&json!(2), &json!(0)));
    assert!(out["bundle_gzip_kb"].as_f64().unwrap() < 3.0);
    assert!(out["build_seconds"].as_f64().unwrap() > 0.0);
    let failed = run_evaluator(
        d,
        "node bench/build-size.mjs",
        &[("BENCH_BUILD_COMMAND", "node -e \"process.exit(2)\"".into())],
    );
    assert_eq!(
        (&failed["error_rate"], &failed["bundle_kb"]),
        (&json!(1), &json!(0))
    );
}

// "memory: reports the peak resident memory of the workload tree"
#[test]
fn memory_reports_the_peak_resident_memory_of_the_workload_tree() {
    needs_node!();
    let dir = write(&render("memory"));
    let d = dir.path();
    let env = |cmd: &str| {
        vec![
            ("BENCH_RUNS", "2".to_owned()),
            ("BENCH_COMMAND", cmd.to_owned()),
        ]
    };
    let small = run_evaluator(d, "node bench/memory.mjs", &env("node -e \"0\""));
    let big = run_evaluator(
        d,
        "node bench/memory.mjs",
        &env(
            "node -e \"const b=Buffer.alloc(200*1024*1024,1);let s=0;for(let i=0;i<b.length;i+=4096)s+=b[i];setTimeout(()=>{},150)\"",
        ),
    );
    assert!(metric_of(&small, "memory") > 5.0);
    assert!(metric_of(&big, "memory") > metric_of(&small, "memory") + 100.0);
    assert_eq!((&big["runs"], &big["error_rate"]), (&json!(2), &json!(0)));
    assert_eq!(big["peak_rss_mb_runs"].as_array().unwrap().len(), 2);
    let failing = run_evaluator(
        d,
        "node bench/memory.mjs",
        &env("node -e \"process.exit(1)\""),
    );
    assert_eq!(failing["error_rate"], json!(1));
}

// "memory: hands the workload path (the holdout when RATCHET_TRACE is set) to the command as $WORKLOAD"
#[test]
fn memory_hands_the_workload_path_to_the_command() {
    needs_node!();
    let dir = write(&render("memory"));
    let d = dir.path();
    std::fs::write(d.join("holdout.json"), "{}").unwrap();
    let cmd = "node -e \"process.exit(process.env.WORKLOAD.endsWith('holdout.json') ? 0 : 1)\""
        .to_owned();
    let trace = d.join("holdout.json").to_string_lossy().into_owned();
    let with = run_evaluator(
        d,
        "node bench/memory.mjs",
        &[
            ("BENCH_RUNS", "1".into()),
            ("BENCH_COMMAND", cmd.clone()),
            ("RATCHET_TRACE", trace),
        ],
    );
    assert_eq!(with["error_rate"], json!(0));
    let without = run_evaluator(
        d,
        "node bench/memory.mjs",
        &[("BENCH_RUNS", "1".into()), ("BENCH_COMMAND", cmd)],
    );
    assert_eq!(without["error_rate"], json!(1));
}

// "forecast-backtest: deterministic, plausible WAPE, scores the sealed holdout, no look-ahead"
#[test]
fn forecast_backtest_is_deterministic_scores_the_holdout_and_has_no_look_ahead() {
    needs_node!();
    let dir = write(&render("forecast-backtest"));
    let d = dir.path();
    let a = run_evaluator(d, "node eval/backtest.mjs", &[]);
    let b = run_evaluator(d, "node eval/backtest.mjs", &[]);
    assert_eq!(a, b);
    let wape = metric_of(&a, "forecast-backtest");
    assert!(wape > 1.0 && wape < 100.0);
    assert_eq!(
        (&a["leakage_violations"], &a["series"], &a["origins"]),
        (&json!(0), &json!(20), &json!(100))
    );
    assert!(a["mape"].as_f64().unwrap() > 0.0);

    // the model only ever sees the history before the origin
    std::fs::write(
        d.join("src/forecast.mjs"),
        "export function forecast(history, horizon) { if (!Object.isFrozen(history)) throw new Error('mutable'); return Array.from({ length: horizon }, () => history.length); }\n",
    )
    .unwrap();
    assert!(run_evaluator(d, "node eval/backtest.mjs", &[]).contains_key("wape"));

    let series: Vec<i64> = (0..40).map(|i| 10 + (i % 5)).collect();
    std::fs::write(d.join("holdout.json"), json!([series]).to_string()).unwrap();
    let held = run_evaluator(
        d,
        "node eval/backtest.mjs",
        &[(
            "RATCHET_TRACE",
            d.join("holdout.json").to_string_lossy().into_owned(),
        )],
    );
    assert_eq!((&held["series"], &held["origins"]), (&json!(1), &json!(3)));
}

// "llm-eval: scores the eval set, reports cost, reads the hidden set, counts failures"
#[test]
fn llm_eval_scores_the_eval_set_reports_cost_reads_the_hidden_set_and_counts_failures() {
    needs_node!();
    let dir = write(&render("llm-eval"));
    let d = dir.path();
    let out = run_evaluator(d, "node eval/llm-eval.mjs", &[]);
    let score = metric_of(&out, "llm-eval");
    assert!(score > 0.3 && score < 1.0);
    assert_eq!(
        (&out["n"], &out["error_rate"], &out["cost_per_call_usd"]),
        (&json!(20), &json!(0), &json!(0.0001))
    );
    assert!(out["tokens_per_call"].as_f64().unwrap() > 0.0);

    std::fs::write(
        d.join("src/task.mjs"),
        "export async function run(input) { return { output: input }; }\n",
    )
    .unwrap();
    let cases = [
        json!({"input": "positive", "expected": "positive"}),
        json!({"input": "The answer is 41.9", "expected": 42, "match": "number"}),
        json!({"input": "abc", "expected": "^zzz", "match": "regex"}),
        json!({"input": "x", "expected": "x", "match": "bogus"}),
    ];
    let lines: Vec<String> = cases.iter().map(Value::to_string).collect();
    std::fs::write(d.join("holdout.jsonl"), lines.join("\n")).unwrap();
    let held = run_evaluator(
        d,
        "node eval/llm-eval.mjs",
        &[(
            "RATCHET_TRACE",
            d.join("holdout.jsonl").to_string_lossy().into_owned(),
        )],
    );
    assert_eq!(
        (&held["n"], &held["error_rate"], &held["score"]),
        (&json!(4), &json!(0.25), &json!(0.5))
    );

    std::fs::write(
        d.join("src/task.mjs"),
        "export async function run() { throw new Error('model down'); }\n",
    )
    .unwrap();
    assert_eq!(
        run_evaluator(d, "node eval/llm-eval.mjs", &[])["error_rate"],
        json!(1)
    );
}

#[test]
fn evaluator_kind_of_each_template_matches_the_registry() {
    // a guard on the table above: backtest for the forecast template, eval for llm, benchmark otherwise
    for id in TEMPLATE_IDS {
        let want = match id {
            "forecast-backtest" => EvaluatorKind::Backtest,
            "llm-eval" => EvaluatorKind::Eval,
            _ => EvaluatorKind::Benchmark,
        };
        assert_eq!(get_template(id).unwrap().evaluator_kind, want, "{id}");
    }
}
