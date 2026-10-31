//! Golden fixtures from the TypeScript implementation (`tests/golden/generate.mjs` runs `packages/bench/src` with tsx).
//!
//! The generated shell scripts are the security boundary of the remote bench (user switching, holdout handling, image
//! restore), so they are compared byte for byte with what the TypeScript produced: a handful of scripts in full, the rest
//! of the matrix by sha256. Keys, manifests, Dockerfile / JSONC parsing, `resolve_image_source` and the PCRE a glob list
//! compiles to are compared the same way.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ratchet_bench::deps::{
    self, Compression, DepsKeyInput, DepsStrategyId, FileHashes, deps_strategy, parse_deps_manifest,
};
use ratchet_bench::deps_scripts::{DepsBuildInput, deps_build_script};
use ratchet_bench::image::{self, ImageKeyInput, ImageSpec, image_key, parse_image_manifest, resolve_image_source};
use ratchet_bench::image_scripts::{
    self, ContainerRunSpec, ContainerRuntime, ImageBuildInput, ImageRestore, container_run, image_build_script,
    image_restore_fragment,
};
use ratchet_bench::remote_scripts::{
    self, CheckoutScriptInput, DepsRestore, Isolation, Layout, StagedArchive, StepKind, StepScriptInput,
    checkout_script, protected_pcre, step_script, wrap_step,
};
use ratchet_bench::shell;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn golden(name: &str) -> Value {
    let p: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
    serde_json::from_str(&fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn hex_sha(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for b in Sha256::digest(s.as_bytes()) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

fn text(v: &Value, k: &str) -> String {
    get(v, k).as_str().unwrap_or_default().to_owned()
}

fn from<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> T {
    serde_json::from_value(get(v, k).clone()).unwrap_or_else(|e| panic!("{k}: {e} in {v}"))
}

fn opt<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> Option<T> {
    match get(v, k) {
        Value::Null => None,
        other => Some(serde_json::from_value(other.clone()).unwrap_or_else(|e| panic!("{k}: {e}"))),
    }
}

fn hashes(v: &Value) -> FileHashes {
    match v {
        Value::Object(m) => m
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
            .collect(),
        _ => FileHashes::new(),
    }
}

fn strategy(id: &str) -> deps::DepsStrategy {
    deps_strategy(match id {
        "pnpm" => DepsStrategyId::Pnpm,
        "npm" => DepsStrategyId::Npm,
        _ => DepsStrategyId::None,
    })
}

/// Compare a generated script (or an error) with a golden case.
fn check_text(name: &str, i: usize, case: &Value, got: Result<String, ratchet_bench::BenchError>) {
    match got {
        Err(e) => assert_eq!(
            case.get("error").and_then(Value::as_str),
            Some(e.to_string().as_str()),
            "{name} #{i}: input {}",
            get(case, "input")
        ),
        Ok(out) => {
            assert!(
                case.get("error").is_none(),
                "{name} #{i}: the TypeScript refused ({}), Rust produced a script; input {}",
                get(case, "error"),
                get(case, "input")
            );
            if let Some(full) = case.get("full").and_then(Value::as_str) {
                assert_eq!(out, full, "{name} #{i}: input {}", get(case, "input"));
            } else {
                assert_eq!(
                    hex_sha(&out),
                    text(case, "sha256"),
                    "{name} #{i}: input {}",
                    get(case, "input")
                );
            }
        }
    }
}

fn deps_restore(v: &Value) -> DepsRestore {
    DepsRestore {
        strategy: strategy(&text(v, "strategy")),
        uri: text(v, "uri"),
        sha256: text(v, "sha256"),
        compression: from(v, "compression"),
        node: text(v, "node"),
        pm: text(v, "pm"),
    }
}

#[test]
fn shell_helpers() {
    let g = golden("shell.json");
    for c in g["sq"].as_array().expect("sq") {
        assert_eq!(shell::sq(&text(c, "input")), text(c, "output"));
    }
    for c in g["wrapStep"].as_array().expect("wrap") {
        match (wrap_step(&text(c, "input")), c.get("error")) {
            (Ok(out), None) => assert_eq!(out, text(c, "output")),
            (Err(e), Some(want)) => assert_eq!(e.to_string(), want.as_str().unwrap_or_default()),
            (got, want) => panic!("wrapStep {}: Rust {got:?}, TypeScript {want:?}", c["input"]),
        }
    }
    for c in g["assertSafeRef"].as_array().expect("refs") {
        let input = text(c, "input");
        let r = shell::assert_safe_ref(&input, "ref");
        match c.get("error") {
            Some(e) => assert_eq!(r.expect_err("refused").to_string(), e.as_str().unwrap_or_default()),
            None => assert!(r.is_ok(), "{input}"),
        }
    }
    for c in g["localTracePath"].as_array().expect("trace") {
        let layout = Layout::new("/var/tmp/w/job");
        assert_eq!(
            remote_scripts::local_trace_path(&layout, &text(c, "input")),
            text(c, "output")
        );
    }
    for c in g["nodeShim"].as_array().expect("shim") {
        let a = c["input"].as_array().expect("args");
        assert_eq!(
            ratchet_bench::node_path::node_shim_script(a[0].as_str().unwrap_or_default(), a[1].as_bool().unwrap_or(false)),
            text(c, "output")
        );
    }
    for c in g["sentinels"].as_array().expect("sentinels") {
        let input = text(c, "input");
        assert_eq!(remote_scripts::strip_sentinels(&input), text(c, "stripped"));
        let got: Vec<Value> = remote_scripts::parse_sentinels(&input)
            .into_iter()
            .map(|s| serde_json::json!({"kind": s.kind, "data": s.data}))
            .collect();
        // an array payload carries no fields in either implementation
        let want: Vec<Value> = c["parsed"]
            .as_array()
            .expect("parsed")
            .iter()
            .map(|p| {
                if p["data"].is_array() {
                    serde_json::json!({"kind": p["kind"], "data": {}})
                } else {
                    p.clone()
                }
            })
            .collect();
        assert_eq!(Value::Array(got), Value::Array(want));
    }
}

#[test]
fn protected_pcre_matches_the_typescript_translation() {
    let g = golden("pcre.json");
    let mut compared = 0;
    for (i, c) in g.as_array().expect("cases").iter().enumerate() {
        if c.get("error").is_some() {
            continue; // the JavaScript RegExp constructor refused the glob; the Rust glob matcher simply never matches
        }
        let globs: Vec<String> = serde_json::from_value(c["input"].clone()).expect("globs");
        assert_eq!(protected_pcre(&globs), text(c, "output"), "#{i}: {globs:?}");
        compared += 1;
    }
    assert!(compared > 1400, "{compared}");
}

#[test]
fn container_run_command_lines() {
    for (i, c) in golden("container_run.json").as_array().expect("cases").iter().enumerate() {
        let spec: ContainerRunSpec = serde_json::from_value(c["input"].clone()).expect("spec");
        assert_eq!(container_run(&spec), text(c, "output"), "#{i}");
    }
}

fn checkout_input(i: &Value) -> CheckoutScriptInput {
    CheckoutScriptInput {
        layout: from(i, "layout"),
        code: from(i, "code"),
        bench: from(i, "bench"),
        globs: from(i, "globs"),
        pinned: opt(i, "pinned").unwrap_or_default(),
        prune: get(i, "prune").as_bool().unwrap_or(false),
        isolation: from(i, "isolation"),
        deps: i.get("deps").map(deps_restore),
        image: opt::<ImageRestore>(i, "image"),
        candidate_launcher: opt(i, "candidateLauncher"),
    }
}

#[test]
fn checkout_scripts_are_byte_identical() {
    let g = golden("checkout.json");
    let cases = g.as_array().expect("cases");
    assert!(cases.len() > 100);
    for (i, c) in cases.iter().enumerate() {
        check_text("checkout", i, c, checkout_script(&checkout_input(&c["input"])));
    }
}

#[test]
fn step_scripts_are_byte_identical() {
    let g = golden("step.json");
    let cases = g.as_array().expect("cases");
    assert!(cases.len() > 100);
    for (i, c) in cases.iter().enumerate() {
        let v = &c["input"];
        let mut input = StepScriptInput::new(
            from(v, "layout"),
            from::<StepKind>(v, "kind"),
            text(v, "name"),
            text(v, "command"),
            get(v, "timeoutMs").as_u64().unwrap_or(0),
        );
        input.env = get(v, "env")
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned())).collect())
            .unwrap_or_default();
        input.isolation = opt::<Isolation>(v, "isolation").unwrap_or_default();
        input.holdout = v.get("holdout").map(|h| text(h, "uri"));
        input.container = opt::<ContainerRuntime>(v, "container");
        input.deps_mounted = get(v, "depsMounted").as_bool().unwrap_or(false);
        input.candidate_launcher = opt(v, "candidateLauncher");
        check_text("step", i, c, step_script(&input));
    }
}

#[test]
fn dependency_snapshot_job_scripts_are_byte_identical() {
    let g = golden("deps_job.json");
    for (i, c) in g.as_array().expect("cases").iter().enumerate() {
        let v = &c["input"];
        let input = DepsBuildInput {
            layout: from(v, "layout"),
            strategy: strategy(&text(v, "strategy")),
            base: from::<StagedArchive>(v, "base"),
            key: text(v, "key"),
            lockfiles: hashes(get(v, "lockfiles")),
            image: text(v, "image"),
            arch: text(v, "arch"),
            bucket: text(v, "bucket"),
            timeout_ms: get(v, "timeoutMs").as_u64().unwrap_or(0),
            isolation: from(v, "isolation"),
            container: opt::<ImageRestore>(v, "container"),
        };
        check_text("deps job", i, c, deps_build_script(&input));
    }
}

#[test]
fn image_job_and_restore_scripts_are_byte_identical() {
    let g = golden("image_job.json");
    for (i, c) in g.as_array().expect("cases").iter().enumerate() {
        let v = &c["input"];
        let input = ImageBuildInput {
            work: text(v, "work"),
            base: from(v, "base"),
            key: text(v, "key"),
            tag: text(v, "tag"),
            spec: from::<ImageSpec>(v, "spec"),
            files: hashes(get(v, "files")),
            arch: text(v, "arch"),
            bucket: text(v, "bucket"),
            timeout_ms: get(v, "timeoutMs").as_u64().unwrap_or(0),
        };
        check_text("image job", i, c, image_build_script(&input));
    }

    let g = golden("image_restore.json");
    for (i, c) in g["restore"].as_array().expect("restore").iter().enumerate() {
        let v = &c["input"];
        let restore: ImageRestore = from(v, "restore");
        check_text(
            "image restore",
            i,
            c,
            image_restore_fragment(&restore, &text(v, "work"), get(v, "isolated").as_bool().unwrap_or(false)),
        );
    }
    for (i, c) in g["ensure"].as_array().expect("ensure").iter().enumerate() {
        let restore: ImageRestore = serde_json::from_value(c["input"].clone()).expect("restore");
        check_text("ensure image loaded", i, c, image_scripts::ensure_image_loaded(&restore));
    }
    let f = &g["fragments"];
    for c in f["ctr"].as_array().expect("ctr") {
        let rt: ContainerRuntime = serde_json::from_value(c["input"].clone()).expect("rt");
        assert_eq!(image_scripts::ctr_function(&rt), text(c, "output"));
    }
    for c in f["ids"].as_array().expect("ids") {
        assert_eq!(
            image_scripts::ids_fragment(c["input"].as_bool().unwrap_or(false)),
            text(c, "output")
        );
    }
    assert_eq!(image_scripts::launcher_conf_fragment(), f["launcherConf"].as_str().unwrap_or_default());
    for c in f["helperMounts"].as_array().expect("mounts") {
        let rt: ContainerRuntime = serde_json::from_value(c["input"].clone()).expect("rt");
        assert_eq!(serde_json::json!(image_scripts::helper_mounts(&rt)), c["output"]);
    }
}

#[test]
fn keys_and_manifests() {
    let g = golden("keys.json");
    for c in g["depsKeys"].as_array().expect("deps keys") {
        let v = &c["input"];
        let mut s = strategy(&text(v, "strategy"));
        s.rev = get(v, "rev").as_u64().unwrap_or(1) as u32;
        let input = DepsKeyInput {
            strategy: s,
            lockfiles: hashes(get(v, "lockfiles")),
            image: text(v, "image"),
            arch: text(v, "arch"),
        };
        assert_eq!(deps::deps_key(&input), text(c, "output"), "{v}");
    }
    for c in g["imageKeys"].as_array().expect("image keys") {
        let v = &c["input"];
        let input = ImageKeyInput {
            spec: from(v, "spec"),
            files: hashes(get(v, "files")),
            arch: text(v, "arch"),
        };
        assert_eq!(image_key(&input), text(c, "output"), "{v}");
    }
    for c in g["arch"].as_array().expect("arch") {
        assert_eq!(deps::arch_from_instance_type(&text(c, "input")), text(c, "output"));
    }
    for c in g["pinned"].as_array().expect("pinned") {
        let want: Vec<String> = serde_json::from_value(c["output"].clone()).expect("list");
        assert_eq!(deps::pinned_deps_files(&strategy(&text(c, "input"))), want);
    }
    for c in g["strategyEnv"].as_array().expect("env") {
        let s = strategy(&text(c, "input"));
        let obj = |pairs: Vec<(String, String)>| {
            Value::Object(pairs.into_iter().map(|(k, v)| (k, Value::String(v))).collect())
        };
        assert_eq!(obj(s.fetch_env("/w/store")), c["fetchEnv"]);
        assert_eq!(obj(s.setup_env("/w/deps/store", "/w/home/.ratchet-deps")), c["setupEnv"]);
        assert_eq!(s.fetch, text(c, "fetch"));
        assert_eq!(s.version, text(c, "version"));
        assert_eq!(s.prune, text(c, "prune"));
        assert_eq!(s.restore, text(c, "restore"));
    }
    let gr = &g["guardrails"];
    let detail = |r: ratchet_core::GuardrailResult| serde_json::to_value(r).expect("json");
    assert_eq!(detail(deps::dependency_change_guardrail(&["a".into(), "b".into()], "ratchet/bench")), gr["deps"]);
    assert_eq!(deps::offline_setup_hint(&deps::PNPM, "ratchet/bench"), text(gr, "hint"));
    assert_eq!(deps::offline_setup_hint(&deps::NPM, "main"), text(gr, "hintNpm"));
    assert_eq!(detail(image::environment_change_guardrail(&["a".into()], "ratchet/bench")), gr["env"]);
    let many: Vec<String> = (0..12).map(|i| format!("f{i}")).collect();
    assert_eq!(detail(image::environment_change_guardrail(&many, "r")), gr["envMany"]);
    assert_eq!(image::container_setup_hint("ratchet/bench"), text(gr, "container"));
    for c in g["logKeys"].as_array().expect("log keys") {
        let job: ratchet_core::ports::BenchJob = serde_json::from_value(c["input"].clone()).expect("job");
        assert_eq!(ratchet_bench::pipeline::log_key(&job), text(c, "output"));
    }
}

fn same_ok_manifest(got: &Value, want: &Value) {
    // `bytes` and `createdAt` are informational; the TypeScript kept any finite number, Rust stores integers
    let strip = |v: &Value| {
        let mut v = v.clone();
        if let Some(m) = v.as_object_mut() {
            m.remove("bytes");
            m.remove("createdAt");
        }
        v
    };
    assert_eq!(strip(got), strip(want));
}

#[test]
fn manifests_are_accepted_and_rejected_like_the_typescript() {
    let g = golden("keys.json");
    for (i, c) in g["depsManifests"].as_array().expect("deps manifests").iter().enumerate() {
        let v = &c["input"];
        let input = DepsKeyInput {
            strategy: strategy(&text(v, "strategy")),
            lockfiles: hashes(get(v, "lockfiles")),
            image: text(v, "image"),
            arch: text(v, "arch"),
        };
        let got = parse_deps_manifest(&text(v, "text"), &input, &text(v, "key"));
        let want = &c["parsed"];
        match got {
            Ok(m) => {
                assert_eq!(want["ok"], true, "#{i} {}", text(v, "text"));
                same_ok_manifest(&serde_json::to_value(&m).expect("json"), &want["manifest"]);
            }
            Err(e) => {
                assert_eq!(want["ok"], false, "#{i}: {e} for {}", text(v, "text"));
                assert_eq!(e, text(want, "error"), "#{i}");
            }
        }
    }
    for (i, c) in g["imageManifests"].as_array().expect("image manifests").iter().enumerate() {
        let v = &c["input"];
        let input = ImageKeyInput {
            spec: from(v, "spec"),
            files: hashes(get(v, "files")),
            arch: text(v, "arch"),
        };
        let got = parse_image_manifest(&text(v, "text"), &input, &text(v, "key"));
        let want = &c["parsed"];
        match got {
            Ok(m) => {
                assert_eq!(want["ok"], true, "#{i} {}", text(v, "text"));
                same_ok_manifest(&serde_json::to_value(&m).expect("json"), &want["manifest"]);
            }
            Err(e) => {
                assert_eq!(want["ok"], false, "#{i}: {e} for {}", text(v, "text"));
                assert_eq!(e, text(want, "error"), "#{i}");
            }
        }
    }
}

#[test]
fn dockerfile_jsonc_and_path_parsing() {
    let g = golden("parse.json");
    for c in g["dockerfiles"].as_array().expect("dockerfiles") {
        let info = image::parse_dockerfile(&text(c, "input"));
        assert_eq!(
            serde_json::json!({"baseImages": info.base_images, "copySources": info.copy_sources}),
            c["output"],
            "{}",
            c["input"]
        );
    }
    for c in g["jsonc"].as_array().expect("jsonc") {
        let got = image::parse_jsonc(&text(c, "input"));
        match c.get("error") {
            Some(_) => assert!(got.is_err(), "{}", c["input"]),
            None => assert_eq!(got.expect("parses"), c["output"], "{}", c["input"]),
        }
    }
    for c in g["normalize"].as_array().expect("normalize") {
        let got = image::normalize_repo_relative(&text(c, "input"), "x");
        match c.get("error") {
            Some(e) => assert_eq!(got.expect_err("refused").to_string(), e.as_str().unwrap_or_default(), "{}", c["input"]),
            None => assert_eq!(got.expect("ok"), text(c, "output"), "{}", c["input"]),
        }
    }
}

#[test]
fn resolve_image_source_matches_the_typescript() {
    let g = golden("resolve_image.json");
    let root = tempfile::tempdir().expect("tmp");
    for (n, c) in g.as_array().expect("cases").iter().enumerate() {
        let dir = root.path().join(format!("t{n}"));
        fs::create_dir_all(&dir).expect("mkdir");
        let input = &c["input"];
        if let Some(files) = input["files"].as_object() {
            for (f, body) in files {
                let p = dir.join(f);
                fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
                fs::write(&p, body.as_str().unwrap_or_default()).expect("write");
            }
        }
        if let Some(links) = input["symlink"].as_object() {
            for (f, target) in links {
                std::os::unix::fs::symlink(target.as_str().unwrap_or_default(), dir.join(f)).expect("symlink");
            }
        }
        let path = text(input, "path");
        let got = resolve_image_source(&dir, &path);
        match (&got, c.get("error")) {
            (Err(e), Some(want)) => {
                // the parser's own message (V8's vs serde_json's) is not part of the contract
                let cut = |s: &str| {
                    s.split_once("(comments and trailing commas are allowed): ")
                        .map_or(s.to_owned(), |(a, _)| a.to_owned())
                };
                assert_eq!(cut(&e.to_string()), cut(want.as_str().unwrap_or_default()), "#{n} {input}");
            }
            (Ok(r), None) => {
                let want = &c["output"];
                assert_eq!(serde_json::to_value(&r.spec).expect("spec"), want["spec"], "#{n} {input}");
                assert_eq!(
                    serde_json::to_string(&r.files).expect("files"),
                    serde_json::to_string(&want["files"]).expect("files"),
                    "#{n} {input}"
                );
            }
            (a, b) => panic!("#{n} {input}: Rust {a:?}, TypeScript {b:?}"),
        }
        let env = image::environment_files_of(&dir, &path);
        let want_env: BTreeMap<String, String> = serde_json::from_value(c["env"].clone()).expect("env");
        let got_env: BTreeMap<String, String> = env.into_iter().collect();
        if text(c, "error").contains("is not valid JSON") {
            // the marker hashes the parser's message (V8's vs serde_json's): only the shape is comparable
            assert_eq!(got_env.keys().collect::<Vec<_>>(), want_env.keys().collect::<Vec<_>>(), "#{n} {input}");
        } else {
            assert_eq!(got_env, want_env, "#{n} {input}");
        }
    }
}

#[test]
fn compression_names() {
    assert_eq!(Compression::Zstd.as_str(), "zstd");
    assert_eq!(Compression::Gzip.as_str(), "gzip");
}
