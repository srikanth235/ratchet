//! Port of `packages/bench/src/image.test.ts`.

use std::fs;
use std::path::Path;

use ratchet_bench::image::{
    ImageKeyInput, ImageKind, ImageSpec, environment_change_guardrail, environment_changes, environment_files_of,
    image_key, image_manifest_key, image_tag, normalize_repo_relative, parse_dockerfile, parse_image_manifest,
    parse_jsonc, read_environment_value, resolve_image_source,
};
use ratchet_bench::{BenchError, FileHashes};
use serde_json::json;

const DIGEST_HEX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn digest() -> String {
    format!("sha256:{DIGEST_HEX}")
}

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tmp");
    for (f, c) in files {
        let p = d.path().join(f);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(&p, c).expect("write");
    }
    d
}

fn h(pairs: &[(&str, &str)]) -> FileHashes {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

fn err(e: BenchError) -> String {
    assert!(matches!(e, BenchError::Infra(_)), "{e:?}");
    e.to_string()
}

mod read_environment {
    use super::*;

    #[test]
    fn returns_nothing_for_configs_without_an_environment() {
        for cfg in [json!({}), json!(null), json!({"environment": "x"}), json!({"environment": []})] {
            let e = read_environment_value(&cfg).expect("ok");
            assert_eq!((e.image, e.machine, e.region, e.network), (None, None, None, None), "{cfg}");
        }
    }

    #[test]
    fn reads_image_machine_region_and_network_ignoring_junk() {
        let e = read_environment_value(&json!({"environment": {
            "image": "./.devcontainer/Dockerfile", "machine": "c7i.2xlarge", "region": "eu-west-1", "network": ["registry.npmjs.org", 3]
        }}))
        .expect("ok");
        assert_eq!(e.image.as_deref(), Some(".devcontainer/Dockerfile"));
        assert_eq!(e.machine.as_deref(), Some("c7i.2xlarge"));
        assert_eq!(e.region.as_deref(), Some("eu-west-1"));
        assert_eq!(e.network, Some(vec!["registry.npmjs.org".to_owned()]));
    }

    #[test]
    fn refuses_an_image_path_that_leaves_the_repository() {
        for p in ["/etc/passwd", "../outside/Dockerfile", "a/../../b", "", "  "] {
            let r = read_environment_value(&json!({"environment": {"image": p}}));
            assert!(matches!(r, Err(BenchError::Infra(_))), "{p:?}");
        }
    }

    #[test]
    fn refuses_a_non_string_image() {
        let e = read_environment_value(&json!({"environment": {"image": 3}})).expect_err("refused");
        assert!(err(e).contains("environment.image must be a path"));
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_repo_relative("./a//b/./c/", "x").expect("ok"), "a/b/c");
        assert_eq!(normalize_repo_relative(".", "x").expect("ok"), ".");
    }
}

mod parse_jsonc_tests {
    use super::*;

    #[test]
    fn accepts_comments_and_trailing_commas_but_leaves_strings_alone() {
        let v = parse_jsonc(
            r#"{
      // the toolchain
      "build": { "dockerfile": "Dockerfile", /* inline */ "args": { "URL": "http://x//y,}" }, },
      "list": [1, 2,],
    }"#,
        )
        .expect("parses");
        assert_eq!(
            v,
            json!({"build": {"dockerfile": "Dockerfile", "args": {"URL": "http://x//y,}"}}, "list": [1, 2]})
        );
    }

    #[test]
    fn throws_on_garbage() {
        assert!(parse_jsonc("{ nope").is_err());
    }
}

mod parse_dockerfile_tests {
    use super::*;

    #[test]
    fn finds_base_images_and_the_context_sources_that_are_copied() {
        let info = parse_dockerfile(&format!(
            "# syntax=docker/dockerfile:1
FROM --platform=linux/amd64 node:22@{} AS build
COPY package.json pnpm-lock.yaml ./
COPY --chown=1000:1000 tools/ /opt/tools/
RUN pnpm install \\
  # a comment inside a continued line
  --frozen-lockfile
FROM build AS test
FROM scratch
COPY --from=build /app /app
ADD https://example.com/x.tgz /x.tgz
COPY [\"a b.txt\", \"/dst/\"]
COPY <<EOF /etc/motd
hello
EOF
",
            digest()
        ));
        assert_eq!(info.base_images, [format!("node:22@{}", digest())]);
        assert_eq!(info.copy_sources, ["package.json", "pnpm-lock.yaml", "tools/", "a b.txt"]);
    }
}

mod resolve_image_source_tests {
    use super::*;

    fn dockerfile() -> String {
        format!("FROM node:22@{}\nCOPY tools.txt /opt/tools.txt\nCOPY scripts/ /opt/scripts/\n", digest())
    }

    fn files() -> Vec<(&'static str, String)> {
        vec![
            (".devcontainer/Dockerfile", dockerfile()),
            (".devcontainer/tools.txt", "k6\n".into()),
            (".devcontainer/scripts/a.sh", "echo a\n".into()),
            (".devcontainer/scripts/sub/b.sh", "echo b\n".into()),
            (".devcontainer/unrelated.md", "not copied\n".into()),
            ("src/index.ts", "export {}\n".into()),
            ("ratchet.yaml", "version: 0\n".into()),
        ]
    }

    fn files_tree(over: &[(&str, &str)]) -> tempfile::TempDir {
        let mut all: Vec<(String, String)> = files().into_iter().map(|(a, b)| (a.to_owned(), b)).collect();
        for (k, v) in over {
            all.retain(|(f, _)| f != k);
            all.push(((*k).to_owned(), (*v).to_owned()));
        }
        let borrowed: Vec<(&str, &str)> = all.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        tree(&borrowed)
    }

    fn sorted_keys(f: &FileHashes) -> Vec<&str> {
        let mut k: Vec<&str> = f.keys().map(String::as_str).collect();
        k.sort_unstable();
        k
    }

    #[test]
    fn hashes_the_dockerfile_and_exactly_the_context_files_it_copies() {
        let t = files_tree(&[]);
        let r = resolve_image_source(t.path(), ".devcontainer/Dockerfile").expect("resolves");
        assert_eq!(r.spec.kind, ImageKind::Dockerfile);
        assert_eq!(r.spec.dockerfile.as_deref(), Some(".devcontainer/Dockerfile"));
        assert_eq!(r.spec.context.as_deref(), Some(".devcontainer"));
        assert_eq!(r.spec.base_digests, [digest()]);
        assert!(r.spec.notes.is_empty());
        assert_eq!(
            sorted_keys(&r.files),
            [
                ".devcontainer/Dockerfile",
                ".devcontainer/scripts/a.sh",
                ".devcontainer/scripts/sub/b.sh",
                ".devcontainer/tools.txt"
            ]
        );
    }

    #[test]
    fn notes_an_unpinned_base_image() {
        let t = files_tree(&[(".devcontainer/Dockerfile", "FROM node:22\n")]);
        let r = resolve_image_source(t.path(), ".devcontainer/Dockerfile").expect("resolves");
        assert!(r.spec.base_digests.is_empty());
        assert!(r.spec.notes.join(" ").contains("not pinned by digest"));
    }

    #[test]
    fn reads_a_devcontainer_json_with_dockerfile_context_args_and_target() {
        let t = files_tree(&[
            (
                ".devcontainer/devcontainer.json",
                r#"{
          // toolchain
          "name": "x", "build": { "dockerfile": "Dockerfile", "context": "..", "args": { "NODE": "22" }, "target": "dev", },
          "features": { "ghcr.io/devcontainers/features/node:1": {} },
          "postCreateCommand": "pnpm i",
        }"#,
            ),
            (".devcontainer/Dockerfile", "FROM node:22\nCOPY .devcontainer/tools.txt /t\n"),
        ]);
        let r = resolve_image_source(t.path(), ".devcontainer/devcontainer.json").expect("resolves");
        assert_eq!(r.spec.kind, ImageKind::Dockerfile);
        assert_eq!(r.spec.dockerfile.as_deref(), Some(".devcontainer/Dockerfile"));
        assert_eq!(r.spec.context.as_deref(), Some("."));
        assert_eq!(r.spec.target.as_deref(), Some("dev"));
        assert_eq!(r.spec.args.get("NODE").map(String::as_str), Some("22"));
        assert_eq!(
            sorted_keys(&r.files),
            [".devcontainer/Dockerfile", ".devcontainer/devcontainer.json", ".devcontainer/tools.txt"]
        );
        assert!(r.spec.notes.join(" ").contains("features, postCreateCommand"));
    }

    #[test]
    fn supports_a_devcontainer_that_names_a_registry_image_instead_of_a_build() {
        let t = tree(&[(
            ".devcontainer/devcontainer.json",
            r#"{ "image": "mcr.microsoft.com/devcontainers/base:ubuntu" }"#,
        )]);
        let r = resolve_image_source(t.path(), ".devcontainer/devcontainer.json").expect("resolves");
        assert_eq!(r.spec.kind, ImageKind::Image);
        assert_eq!(r.spec.image.as_deref(), Some("mcr.microsoft.com/devcontainers/base:ubuntu"));
        assert!(r.spec.notes.join(" ").contains("not pinned by digest"));
    }

    #[test]
    fn refuses_what_the_bench_cannot_honor_with_a_reason() {
        let case = |files: &[(&str, &str)], path: &str| -> String {
            err(resolve_image_source(tree(files).path(), path).expect_err("refused"))
        };
        assert!(case(&[("d.json", r#"{ "dockerComposeFile": "c.yml" }"#)], "d.json").contains("dockerComposeFile is not supported"));
        assert!(case(&[("d.json", r#"{ "name": "x" }"#)], "d.json").contains("needs \"build.dockerfile\""));
        assert!(case(&[("d.json", "{ nope")], "d.json").contains("not valid JSON"));
        assert!(case(&[], "Dockerfile").contains("does not exist"));
        assert!(case(&[("Dockerfile", "# nothing\n")], "Dockerfile").contains("no FROM"));
        assert!(
            case(
                &[("d.json", r#"{ "build": { "dockerfile": "D", "args": { "A": "x y;rm" } } }"#), ("D", "FROM a\n")],
                "d.json"
            )
            .contains("not a plain build argument")
        );
    }

    #[test]
    fn refuses_a_dockerfile_that_copies_the_repository_into_the_image() {
        let t = tree(&[("Dockerfile", "FROM node:22\nCOPY . /app\n"), ("ratchet.yaml", "version: 0\n"), ("src/a.ts", "1")]);
        let e = err(resolve_image_source(t.path(), "Dockerfile").expect_err("refused"));
        assert!(e.contains("copies ratchet.yaml (or the whole repository)"), "{e}");
    }

    #[test]
    fn a_root_level_dockerfile_may_copy_a_manifest_without_copying_the_repository() {
        let t = tree(&[
            ("Dockerfile", "FROM node:22\nCOPY package.json pnpm-lock.yaml ./\n"),
            ("package.json", "{}"),
            ("pnpm-lock.yaml", "x"),
            ("src/a.ts", "1"),
            ("ratchet.yaml", "v"),
        ]);
        let r = resolve_image_source(t.path(), "Dockerfile").expect("resolves");
        assert_eq!(sorted_keys(&r.files), ["Dockerfile", "package.json", "pnpm-lock.yaml"]);
    }

    #[test]
    fn expands_wildcard_sources_against_the_context() {
        let t = tree(&[
            ("ctx/Dockerfile", "FROM a\nCOPY *.cfg /etc/\n"),
            ("ctx/a.cfg", "1"),
            ("ctx/b.cfg", "2"),
            ("ctx/c.txt", "3"),
            ("ctx/sub/d.cfg", "4"),
        ]);
        let r = resolve_image_source(t.path(), "ctx/Dockerfile").expect("resolves");
        assert_eq!(sorted_keys(&r.files), ["ctx/Dockerfile", "ctx/a.cfg", "ctx/b.cfg"]);
    }

    #[test]
    fn treats_a_variable_copy_source_conservatively_as_the_whole_small_context() {
        let t = tree(&[("ctx/Dockerfile", "FROM a\nARG F=x\nCOPY $F /x\n"), ("ctx/x", "1"), ("ctx/y", "2")]);
        let r = resolve_image_source(t.path(), "ctx/Dockerfile").expect("resolves");
        assert_eq!(sorted_keys(&r.files), ["ctx/Dockerfile", "ctx/x", "ctx/y"]);
        assert!(r.spec.notes.join(" ").contains("uses a variable"));
    }

    #[test]
    fn a_dockerfile_that_is_a_symlink_is_refused() {
        let t = tree(&[("real", "FROM a\n")]);
        std::os::unix::fs::symlink("real", t.path().join("Dockerfile")).expect("symlink");
        let e = err(resolve_image_source(t.path(), "Dockerfile").expect_err("refused"));
        assert!(e.contains("regular file"), "{e}");
    }

    mod environment_changes_tests {
        use super::*;

        fn resolved(p: &Path) -> FileHashes {
            resolve_image_source(p, ".devcontainer/Dockerfile").expect("resolves").files
        }

        #[test]
        fn detects_a_changed_dockerfile_a_changed_added_removed_file_and_an_unresolvable_environment() {
            let base_tree = files_tree(&[]);
            let base = resolved(base_tree.path());
            let of = |over: &[(&str, &str)]| -> FileHashes {
                environment_files_of(files_tree(over).path(), ".devcontainer/Dockerfile")
            };
            let same = of(&[
                ("src/index.ts", "export const x = 1\n"),
                (".devcontainer/unrelated.md", "changed but not copied\n"),
            ]);
            assert!(environment_changes(&base, &same).is_empty());

            let df = of(&[(".devcontainer/Dockerfile", &format!("{}RUN curl evil\n", dockerfile()))]);
            assert_eq!(environment_changes(&base, &df), [".devcontainer/Dockerfile"]);

            let tools = of(&[(".devcontainer/tools.txt", "k6\nevil\n")]);
            assert_eq!(environment_changes(&base, &tools), [".devcontainer/tools.txt"]);

            let added = of(&[(".devcontainer/scripts/c.sh", "echo c\n")]);
            assert_eq!(environment_changes(&base, &added), [".devcontainer/scripts/c.sh"]);

            let without_dockerfile: Vec<(String, String)> = files()
                .into_iter()
                .filter(|(f, _)| *f != ".devcontainer/Dockerfile")
                .map(|(a, b)| (a.to_owned(), b))
                .collect();
            let borrowed: Vec<(&str, &str)> =
                without_dockerfile.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            let deleted = environment_files_of(tree(&borrowed).path(), ".devcontainer/Dockerfile");
            assert!(
                environment_changes(&base, &deleted)
                    .contains(&"the environment (Dockerfile / devcontainer) no longer resolves".to_owned())
            );
        }

        #[test]
        fn the_verdict_text_says_environment_changes_are_not_allowed_in_v0() {
            let g = environment_change_guardrail(&[".devcontainer/Dockerfile".into()], "ratchet/bench");
            assert_eq!((g.name.as_str(), g.passed), ("environment unchanged", false));
            let d = g.detail.expect("detail");
            assert!(d.contains("environment changes are not allowed in v0") && d.contains(".devcontainer/Dockerfile"));
        }
    }
}

mod image_key_and_the_manifest {
    use super::*;

    fn spec(args: &[(&str, &str)], digests: Vec<String>) -> ImageSpec {
        ImageSpec {
            kind: ImageKind::Dockerfile,
            path: "Dockerfile".into(),
            dockerfile: Some("Dockerfile".into()),
            context: Some(".".into()),
            target: None,
            args: args.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
            image: None,
            base_images: vec!["node:22".into()],
            base_digests: digests,
            notes: vec![],
        }
    }

    fn base() -> ImageKeyInput {
        ImageKeyInput {
            spec: spec(&[("A", "1"), ("B", "2")], vec![]),
            files: h(&[("Dockerfile", "h1"), ("b.txt", "h2")]),
            arch: "x86_64".into(),
        }
    }

    #[test]
    fn is_content_addressed() {
        let k = image_key(&base());
        assert!(k.starts_with("img-") && k.len() == 44 && k[4..].bytes().all(|b| b.is_ascii_hexdigit()), "{k}");
        let reordered = ImageKeyInput {
            spec: spec(&[("B", "2"), ("A", "1")], vec![]),
            files: h(&[("b.txt", "h2"), ("Dockerfile", "h1")]),
            arch: "x86_64".into(),
        };
        assert_eq!(image_key(&reordered), k);
        let mut changed = base();
        changed.files.insert("Dockerfile".into(), "other".into());
        assert_ne!(image_key(&changed), k);
        let changed = ImageKeyInput { spec: spec(&[("A", "9")], vec![]), ..base() };
        assert_ne!(image_key(&changed), k);
        let changed = ImageKeyInput { spec: spec(&[("A", "1"), ("B", "2")], vec![digest()]), ..base() };
        assert_ne!(image_key(&changed), k);
        let changed = ImageKeyInput { arch: "arm64".into(), ..base() };
        assert_ne!(image_key(&changed), k);
        assert_eq!(image_tag(&k), format!("ratchet-env:{k}"));
        assert_eq!(image_manifest_key(&k), format!("bench-images/{k}.json"));
    }

    #[test]
    fn accepts_a_manifest_that_matches_what_the_controller_computed_and_rejects_anything_else() {
        let input = base();
        let key = image_key(&input);
        let sha = "b".repeat(64);
        let good = json!({
            "v": 1, "key": key, "rev": 1, "arch": "x86_64", "files": input.files, "tag": image_tag(&key),
            "imageId": format!("sha256:{}", "c".repeat(64)), "compression": "zstd",
            "archive": format!("bench-images/{key}/{sha}.tar.zst"), "sha256": sha, "bytes": 10, "createdAt": 1,
        });
        let check = |patch: serde_json::Value| {
            let mut m = good.clone();
            for (k, v) in patch.as_object().expect("patch") {
                m[k] = v.clone();
            }
            parse_image_manifest(&m.to_string(), &input, &key)
        };
        assert!(check(json!({})).is_ok());
        assert!(parse_image_manifest("nope", &input, &key).is_err());
        let rejected = |patch: serde_json::Value, want: Option<&str>| {
            let e = check(patch.clone()).expect_err(&patch.to_string());
            if let Some(w) = want {
                assert!(e.contains(w), "{patch}: {e}");
            }
        };
        rejected(json!({"key": format!("img-{}", "0".repeat(40))}), Some("another key"));
        rejected(json!({"arch": "arm64"}), Some("architecture"));
        rejected(json!({"files": {"Dockerfile": "evil"}}), Some("file hashes"));
        rejected(json!({"tag": "ratchet-env:other"}), None);
        rejected(json!({"imageId": "latest"}), Some("image id"));
        rejected(json!({"archive": format!("bench-images/other/{sha}.tar.zst")}), Some("outside its own prefix"));
        rejected(json!({"archive": format!("bench-inputs/{key}/{sha}.tar.zst")}), None);
        rejected(json!({"compression": "gzip"}), None); // the archive name says zstd
        rejected(json!({"rev": 2}), None);
    }
}

