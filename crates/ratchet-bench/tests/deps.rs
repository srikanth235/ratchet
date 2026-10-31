//! Port of `packages/bench/src/deps.test.ts`.

use std::fs;
use std::os::unix::fs::symlink;

use ratchet_bench::BenchError;
use ratchet_bench::deps::{
    Compression, DepsKeyInput, DepsManifest, DepsStrategyId, FileHashes, NO_DEPS, NPM, PNPM,
    arch_from_instance_type, dependency_change_guardrail, deps_key, hash_named_files,
    lockfile_changes, manifest_key, parse_deps_manifest, resolve_deps_strategy, root_entries,
};
use ratchet_bench::deps_scripts::{DepsBuildInput, deps_build_script};
use ratchet_bench::remote_scripts::{
    CheckoutScriptInput, DepsRestore, Isolation, Layout, StagedArchive, checkout_script,
};
use ratchet_core::DepsMode as DepsMode_;
use serde_json::json;

fn archive(name: &str) -> StagedArchive {
    StagedArchive::new(
        format!("s3://ratchet-test-artifacts/bench-inputs/opt1/job1/{name}"),
        "a".repeat(64),
        "c".repeat(40),
    )
}

fn h(pairs: &[(&str, &str)]) -> FileHashes {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn infra(e: BenchError) -> String {
    e.to_string()
}

mod resolve_deps_strategy_tests {
    use super::*;

    #[test]
    fn auto_pnpm_wins_then_npm_otherwise_none() {
        let auto = |files: &[&str]| {
            resolve_deps_strategy(DepsMode_::Auto, files.iter().copied())
                .expect("auto")
                .strategy
        };
        assert_eq!(auto(&["package.json", "pnpm-lock.yaml"]), PNPM);
        assert_eq!(auto(&["pnpm-lock.yaml", "package-lock.json"]), PNPM);
        assert_eq!(auto(&["package.json", "package-lock.json"]), NPM);
        assert_eq!(auto(&["npm-shrinkwrap.json"]), NPM);
        assert_eq!(auto(&["package.json", "src"]), NO_DEPS);
        assert_eq!(auto(&[]), NO_DEPS);
    }

    #[test]
    fn auto_yarn_lock_is_said_not_silently_ignored() {
        let r = resolve_deps_strategy(DepsMode_::Auto, ["yarn.lock"]).expect("auto");
        assert_eq!(r.strategy, NO_DEPS);
        let note = r.note.expect("a note");
        assert!(
            note.contains("yarn.lock") && note.contains("pnpm and npm only"),
            "{note}"
        );
    }

    #[test]
    fn none_always_means_none_even_with_a_lockfile() {
        assert_eq!(
            resolve_deps_strategy(DepsMode_::None, ["pnpm-lock.yaml"])
                .expect("none")
                .strategy,
            NO_DEPS
        );
    }

    #[test]
    fn an_explicit_package_manager_demands_its_lockfile() {
        assert_eq!(
            resolve_deps_strategy(DepsMode_::Npm, ["package-lock.json"])
                .expect("npm")
                .strategy,
            NPM
        );
        let e = resolve_deps_strategy(DepsMode_::Pnpm, ["package-lock.json"]).expect_err("refused");
        assert!(matches!(e, BenchError::Infra(_)));
        assert!(infra(e).contains("bench.ref has no pnpm-lock.yaml"));
        let e = resolve_deps_strategy(DepsMode_::Npm, Vec::<&str>::new()).expect_err("refused");
        assert!(infra(e).contains("package-lock.json or npm-shrinkwrap.json"));
    }

    #[test]
    fn the_no_op_strategy_pins_nothing_fetches_nothing_and_sets_no_environment() {
        assert!(NO_DEPS.lockfiles.is_empty());
        assert!(NO_DEPS.setup_env("/s", "/x").is_empty());
        assert!(NO_DEPS.fetch_env("/s").is_empty());
    }
}

mod strategies {
    use super::*;

    fn get<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn pnpm_fetches_into_the_store_only_and_refuses_to_switch_pnpm_versions() {
        assert_eq!(PNPM.fetch, "pnpm fetch");
        let env = PNPM.fetch_env("/w/store");
        assert_eq!(get(&env, "npm_config_store_dir"), Some("/w/store"));
        assert_eq!(get(&env, "PNPM_STORE_DIR"), Some("/w/store"));
        assert_eq!(get(&env, "npm_config_ignore_scripts"), Some("true"));
        assert_eq!(
            get(&env, "npm_config_manage_package_manager_versions"),
            Some("false")
        );
    }

    #[test]
    fn pnpm_setup_gets_the_restored_store_offline_and_never_hardlinks() {
        let env = PNPM.setup_env("/w/deps/store", "/w/home/.ratchet-deps");
        assert_eq!(get(&env, "npm_config_store_dir"), Some("/w/deps/store"));
        assert_eq!(get(&env, "PNPM_STORE_DIR"), Some("/w/deps/store"));
        assert_eq!(get(&env, "npm_config_offline"), Some("true"));
        assert_eq!(
            get(&env, "npm_config_package_import_method"),
            Some("clone-or-copy")
        );
        // the candidate's own install scripts do run in setup
        assert_eq!(get(&env, "npm_config_ignore_scripts"), None);
    }

    #[test]
    fn pnpm_per_project_registrations_stay_out_of_the_snapshot() {
        assert!(PNPM.prune.contains("projects"));
        assert!(PNPM.restore.contains("ln -s \"$SCRATCH/pnpm-projects\""));
    }

    #[test]
    fn npm_installs_with_ignore_scripts_and_keeps_only_the_cache() {
        assert_eq!(NPM.fetch, "npm ci --ignore-scripts --no-audit --no-fund");
        assert_eq!(
            get(&NPM.fetch_env("/w/store"), "npm_config_cache"),
            Some("/w/store")
        );
        let env = NPM.setup_env("/w/deps/store", "/s");
        assert_eq!(get(&env, "npm_config_cache"), Some("/w/deps/store"));
        assert_eq!(get(&env, "npm_config_offline"), Some("true"));
        assert_eq!(get(&env, "npm_config_logs_dir"), Some("/s/npm-logs"));
    }
}

mod hashing_and_comparing_lockfiles {
    use super::*;

    #[test]
    fn hashes_the_files_that_exist_by_content() {
        let a = tempfile::tempdir().expect("tmp");
        fs::write(a.path().join("pnpm-lock.yaml"), "x").expect("write");
        let hs =
            hash_named_files(a.path(), &["pnpm-lock.yaml", "package-lock.json"]).expect("hash");
        assert_eq!(hs.keys().collect::<Vec<_>>(), ["pnpm-lock.yaml"]);
        let v = &hs["pnpm-lock.yaml"];
        assert!(
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{v}"
        );
    }

    #[test]
    fn a_symlinked_lockfile_never_equals_a_real_file_with_the_same_text() {
        let a = tempfile::tempdir().expect("tmp");
        let b = tempfile::tempdir().expect("tmp");
        fs::write(a.path().join("real"), "pnpm-lock.yaml").expect("write");
        fs::write(a.path().join("pnpm-lock.yaml"), "content").expect("write");
        symlink("/etc/passwd", b.path().join("pnpm-lock.yaml")).expect("symlink");
        let ha = hash_named_files(a.path(), &["pnpm-lock.yaml"]).expect("hash");
        let hb = hash_named_files(b.path(), &["pnpm-lock.yaml"]).expect("hash");
        assert_eq!(lockfile_changes(&ha, &hb), ["pnpm-lock.yaml"]);
        assert!(hb["pnpm-lock.yaml"].starts_with("link:"));
    }

    #[test]
    fn root_entries_lists_the_root_of_a_tree() {
        let a = tempfile::tempdir().expect("tmp");
        fs::create_dir(a.path().join("src")).expect("mkdir");
        fs::write(a.path().join("package-lock.json"), "{}").expect("write");
        let mut got = root_entries(a.path()).expect("entries");
        got.sort();
        assert_eq!(got, ["package-lock.json", "src"]);
    }

    #[test]
    fn lockfile_changes_counts_changed_added_and_removed_and_sorts() {
        assert!(lockfile_changes(&h(&[("a", "1")]), &h(&[("a", "1")])).is_empty());
        assert_eq!(
            lockfile_changes(&h(&[("a", "1")]), &h(&[("a", "2")])),
            ["a"]
        );
        assert_eq!(lockfile_changes(&h(&[("a", "1")]), &h(&[])), ["a"]);
        assert_eq!(lockfile_changes(&h(&[]), &h(&[("b", "1")])), ["b"]);
        assert_eq!(
            lockfile_changes(&h(&[("z", "1"), ("a", "1")]), &h(&[("z", "2"), ("a", "2")])),
            ["a", "z"]
        );
    }

    #[test]
    fn the_blocking_reason_says_why_and_how_to_fix_it() {
        let g = dependency_change_guardrail(&["pnpm-lock.yaml".into()], "ratchet/bench");
        assert_eq!(
            (g.name.as_str(), g.passed),
            ("dependencies unchanged", false)
        );
        let d = g.detail.expect("detail");
        assert!(d.contains("dependency changes are not allowed in v0: the bench has no network"));
        assert!(d.contains("pnpm-lock.yaml differs from ratchet/bench"));
    }
}

mod snapshot_key {
    use super::*;

    fn base() -> DepsKeyInput {
        DepsKeyInput {
            strategy: PNPM,
            lockfiles: h(&[("pnpm-lock.yaml", &"a".repeat(64))]),
            image: "ami-1".into(),
            arch: "x86_64".into(),
        }
    }

    #[test]
    fn is_strategy_dash_40_hex_and_stable() {
        let k = deps_key(&base());
        assert!(
            k.starts_with("pnpm-")
                && k.len() == 45
                && k[5..].bytes().all(|b| b.is_ascii_hexdigit()),
            "{k}"
        );
        assert_eq!(k, deps_key(&base()));
        assert_eq!(manifest_key(&k), format!("bench-deps/{k}.json"));
    }

    #[test]
    fn depends_on_lockfile_image_arch_strategy_and_its_revision() {
        let k = deps_key(&base());
        let mut other = base();
        other.lockfiles = h(&[("pnpm-lock.yaml", &"b".repeat(64))]);
        assert_ne!(deps_key(&other), k);
        let mut other = base();
        other.image = "ami-2".into();
        assert_ne!(deps_key(&other), k);
        let mut other = base();
        other.arch = "arm64".into();
        assert_ne!(deps_key(&other), k);
        let mut other = base();
        other.strategy.rev += 1;
        assert_ne!(deps_key(&other), k);
        let mut other = base();
        other.strategy = NPM;
        other.lockfiles = h(&[("package-lock.json", &"a".repeat(64))]);
        assert!(deps_key(&other).starts_with("npm-"));
    }

    #[test]
    fn does_not_depend_on_the_order_lockfiles_were_listed_in() {
        let l1 = h(&[
            ("package-lock.json", &"a".repeat(64)),
            ("npm-shrinkwrap.json", &"b".repeat(64)),
        ]);
        let l2 = h(&[
            ("npm-shrinkwrap.json", &"b".repeat(64)),
            ("package-lock.json", &"a".repeat(64)),
        ]);
        let mk = |lockfiles| DepsKeyInput {
            strategy: NPM,
            lockfiles,
            image: "ami-1".into(),
            arch: "x86_64".into(),
        };
        assert_eq!(deps_key(&mk(l1)), deps_key(&mk(l2)));
    }

    macro_rules! arch_is {
        ($($name:ident: $t:literal => $arch:literal;)*) => {
            $( #[test] fn $name() { assert_eq!(arch_from_instance_type($t), $arch); } )*
        };
    }

    arch_is! {
        arch_of_c7i_2xlarge: "c7i.2xlarge" => "x86_64";
        arch_of_m7i_large: "m7i.large" => "x86_64";
        arch_of_t3_medium: "t3.medium" => "x86_64";
        arch_of_g5_xlarge: "g5.xlarge" => "x86_64";
        arch_of_c7g_large: "c7g.large" => "arm64";
        arch_of_m6gd_xlarge: "m6gd.xlarge" => "arm64";
        arch_of_t4g_nano: "t4g.nano" => "arm64";
        arch_of_im4gn_large: "im4gn.large" => "arm64";
        arch_of_is4gen_medium: "is4gen.medium" => "arm64";
        arch_of_hpc7g_4xlarge: "hpc7g.4xlarge" => "arm64";
        arch_of_a1_large: "a1.large" => "arm64";
    }
}

mod manifest_validation {
    use super::*;

    struct Fixture {
        input: DepsKeyInput,
        key: String,
        good: serde_json::Value,
        digest: String,
    }

    fn fixture() -> Fixture {
        let lockfiles = h(&[("pnpm-lock.yaml", &"a".repeat(64))]);
        let input = DepsKeyInput {
            strategy: PNPM,
            lockfiles: lockfiles.clone(),
            image: "ami-1".into(),
            arch: "x86_64".into(),
        };
        let key = deps_key(&input);
        let digest = "d".repeat(64);
        let good = json!({
            "v": 1, "key": key, "strategy": "pnpm", "rev": PNPM.rev, "lockfiles": lockfiles, "image": "ami-1", "arch": "x86_64",
            "node": "v22.22.0", "pm": "10.28.0", "compression": "zstd", "archive": format!("bench-deps/{key}/{digest}.tar.zst"),
            "sha256": digest, "bytes": 1234, "createdAt": 1,
        });
        Fixture {
            input,
            key,
            good,
            digest,
        }
    }

    fn check(f: &Fixture, patch: serde_json::Value) -> Result<DepsManifest, String> {
        let mut m = f.good.clone();
        for (k, v) in patch.as_object().expect("patch") {
            if v.is_null() {
                m.as_object_mut().expect("obj").remove(k);
            } else {
                m[k] = v.clone();
            }
        }
        parse_deps_manifest(&m.to_string(), &f.input, &f.key)
    }

    #[test]
    fn accepts_what_the_snapshot_job_writes() {
        let f = fixture();
        let m = check(&f, json!({})).expect("valid");
        assert_eq!(
            m,
            DepsManifest {
                v: 1,
                key: f.key.clone(),
                strategy: DepsStrategyId::Pnpm,
                rev: PNPM.rev,
                lockfiles: f.input.lockfiles.clone(),
                image: "ami-1".into(),
                arch: "x86_64".into(),
                node: "v22.22.0".into(),
                pm: "10.28.0".into(),
                compression: Compression::Zstd,
                archive: format!("bench-deps/{}/{}.tar.zst", f.key, f.digest),
                sha256: f.digest.clone(),
                bytes: 1234,
                created_at: 1,
            }
        );
        let gz = check(
            &f,
            json!({"compression": "gzip", "archive": format!("bench-deps/{}/{}.tgz", f.key, f.digest)}),
        );
        assert!(gz.is_ok());
    }

    macro_rules! rejects {
        ($($name:ident: |$f:ident| $patch:expr => $want:literal;)*) => {
            $(
                #[test]
                fn $name() {
                    let $f = fixture();
                    let e = check(&$f, $patch).expect_err(stringify!($name));
                    assert!(e.contains($want), "{e}");
                }
            )*
        };
    }

    rejects! {
        rejects_another_key: |f| json!({"key": format!("pnpm-{}", "0".repeat(40))}) => "another key";
        rejects_other_lockfile_hashes: |f| json!({"lockfiles": {"pnpm-lock.yaml": "b".repeat(64)}}) => "different lockfile hashes";
        rejects_an_extra_lockfile: |f| json!({"lockfiles": {"pnpm-lock.yaml": "a".repeat(64), "x.yaml": "c".repeat(64)}}) => "different lockfile hashes";
        rejects_another_image: |f| json!({"image": "ami-2"}) => "another image";
        rejects_another_arch: |f| json!({"arch": "arm64"}) => "another image or architecture";
        rejects_another_strategy_revision: |f| json!({"rev": PNPM.rev + 1}) => "strategy revision";
        rejects_an_archive_outside_its_prefix: |f| json!({"archive": format!("bench-deps/other/{}.tar.zst", f.digest)}) => "outside its own prefix";
        rejects_a_traversing_archive: |f| json!({"archive": format!("bench-deps/{}/../../x/{}.tar.zst", f.key, f.digest)}) => "outside its own prefix";
        rejects_an_archive_of_another_digest: |f| json!({"archive": format!("bench-deps/{}/{}.tar.zst", f.key, "e".repeat(64))}) => "outside its own prefix";
        rejects_an_extension_that_does_not_match_the_compression: |f| json!({"archive": format!("bench-deps/{}/{}.tgz", f.key, f.digest)}) => "outside its own prefix";
        rejects_a_bad_digest: |f| json!({"sha256": "xyz"}) => "sha256";
        rejects_a_shell_looking_node_version: |f| json!({"node": "v22; rm -rf /"}) => "tool versions";
        rejects_an_unknown_compression: |f| json!({"compression": "xz"}) => "compression";
        rejects_a_negative_size: |f| json!({"bytes": -1}) => "size";
    }

    #[test]
    fn rejects_text_that_is_not_json() {
        let f = fixture();
        let e = parse_deps_manifest("nope", &f.input, &f.key).expect_err("not json");
        assert!(e.contains("not JSON"), "{e}");
    }
}

mod the_dependency_snapshot_job_script {
    use super::*;

    const KEY: &str = "pnpm-ffffffffffffffffffffffffffffffffffffffff";

    fn input() -> DepsBuildInput {
        DepsBuildInput {
            layout: Layout::new("/var/tmp/ratchet-bench/deps-opt1-1234"),
            strategy: PNPM,
            base: archive("deps-base.tgz"),
            key: KEY.into(),
            lockfiles: h(&[("pnpm-lock.yaml", &"a".repeat(64))]),
            image: "ami-1".into(),
            arch: "x86_64".into(),
            bucket: "ratchet-test-artifacts".into(),
            timeout_ms: 600_000,
            isolation: Isolation::User,
            container: None,
        }
    }

    fn script() -> String {
        deps_build_script(&input()).expect("script")
    }

    fn idx(s: &str, needle: &str) -> usize {
        s.find(needle).unwrap_or_else(|| panic!("missing {needle}"))
    }

    #[test]
    fn only_ever_downloads_the_staged_base_tree() {
        let s = script();
        let downloads = s
            .lines()
            .filter(|l| {
                l.starts_with("aws s3 cp --only-show-errors 's3://")
                    && l.contains("\"$W/in/base.tgz\"")
            })
            .count();
        assert_eq!(downloads, 1);
        assert!(s.contains("s3://ratchet-test-artifacts/bench-inputs/opt1/job1/deps-base.tgz"));
        for needle in [
            "code.tgz",
            "bench.tgz",
            "git ",
            "bench-results",
            "sealed",
            "holdout",
            "RATCHET_TRACE",
        ] {
            assert!(!s.contains(needle), "{needle}");
        }
    }

    #[test]
    fn refuses_to_be_built_for_the_candidate_archive_or_another_basename() {
        let mut i = input();
        i.base = archive("code.tgz");
        assert!(
            infra(deps_build_script(&i).expect_err("refused")).contains("ending in deps-base.tgz")
        );
        let mut i = input();
        i.base.uri = "s3://b/../deps-base.tgz".into();
        assert!(matches!(deps_build_script(&i), Err(BenchError::Infra(_))));
    }

    #[test]
    fn verifies_the_digest_before_unpacking_anything() {
        let s = script();
        assert!(idx(&s, "sha256sum") < idx(&s, "tar -xzf"));
    }

    #[test]
    fn runs_the_fetch_as_the_unprivileged_user_with_no_new_privileges_and_a_clean_environment() {
        let s = script();
        assert!(s.contains("DU='ratchet-deps'"));
        assert!(s.contains("setpriv --reuid=\"$DU\" --regid=\"$DU\" --clear-groups --no-new-privs -- env -i PATH=\"$PATH\""));
        assert!(
            !s.contains("AWS_"),
            "credentials never reach the unprivileged user"
        );
        assert!(s.contains("for v in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy NO_PROXY no_proxy NODE_USE_ENV_PROXY"));
    }

    #[test]
    fn checks_the_user_and_the_imds_block_before_it_runs_anything_as_that_user() {
        let s = script();
        let run = idx(
            &s,
            "setpriv --reuid=\"$DU\" --regid=\"$DU\" --clear-groups --no-new-privs -- env -i",
        );
        assert!(idx(&s, "id -u \"$DU\"") < run);
        assert!(
            idx(
                &s,
                "iptables -C OUTPUT -m owner --uid-owner \"$DU\" -d 169.254.169.254 -j REJECT"
            ) < run
        );
        assert!(idx(&s, "/dev/tcp/169.254.169.254/80") < run);
        assert!(s.contains("exit 13") && s.contains("exit 14"));
    }

    #[test]
    fn executes_only_the_strategy_fetch() {
        let s = script();
        assert!(s.contains("bash -c 'pnpm fetch'"));
        let re = regex::Regex::new(r"pnpm (install|i|run|build|exec|dlx)").expect("re");
        assert!(!re.is_match(&s));
        assert!(s.contains("npm_config_ignore_scripts='true'"));
        assert!(s.contains("npm_config_store_dir='/var/tmp/ratchet-bench/deps-opt1-1234/store'"));
        let mut i = input();
        i.strategy = NPM;
        i.key = format!("npm-{}", "e".repeat(40));
        i.lockfiles = h(&[("package-lock.json", &"a".repeat(64))]);
        assert!(
            deps_build_script(&i)
                .expect("npm")
                .contains("bash -c 'npm ci --ignore-scripts --no-audit --no-fund'")
        );
    }

    #[test]
    fn packs_and_uploads_as_root_only_after_the_fetch_succeeded_archive_first_manifest_last() {
        let s = script();
        let fetch_done = idx(&s, "the package fetch (pnpm) exited with status");
        let pack = idx(&s, "tar -C \"$STORE\"");
        let upload_archive = idx(&s, "\"s3://ratchet-test-artifacts/$ARCHIVE\"");
        let upload_manifest = idx(
            &s,
            &format!("s3://ratchet-test-artifacts/bench-deps/{KEY}.json"),
        );
        assert!(fetch_done < pack && pack < upload_archive && upload_archive < upload_manifest);
        assert!(s.contains(&format!("ARCHIVE='bench-deps/{KEY}'/\"$SHA.$EXT\"")));
    }

    #[test]
    fn prunes_per_project_state_and_refuses_symlinks_before_packing() {
        let s = script();
        assert!(idx(&s, PNPM.prune) < idx(&s, "find \"$STORE\" -type l"));
        assert!(idx(&s, "find \"$STORE\" -type l") < idx(&s, "tar -C \"$STORE\""));
    }

    #[test]
    fn writes_a_manifest_that_carries_the_controller_computed_key_material() {
        let head = json!({"v": 1, "key": KEY, "strategy": "pnpm", "rev": PNPM.rev, "lockfiles": {"pnpm-lock.yaml": "a".repeat(64)}, "image": "ami-1", "arch": "x86_64"}).to_string();
        assert!(script().contains(&head[..head.len() - 1]));
    }

    #[test]
    fn rejects_keys_buckets_and_strategies_that_could_change_what_is_written() {
        let mut i = input();
        i.key = "pnpm-../../x".into();
        assert!(infra(deps_build_script(&i).expect_err("key")).contains("not a plain key"));
        let mut i = input();
        i.bucket = "Bad_Bucket;x".into();
        assert!(infra(deps_build_script(&i).expect_err("bucket")).contains("valid bucket"));
        let mut i = input();
        i.strategy = NO_DEPS;
        assert!(
            infra(deps_build_script(&i).expect_err("strategy")).contains("nothing to snapshot")
        );
    }

    #[test]
    fn isolation_none_skips_the_os_user_checks_and_still_runs_the_fetch() {
        let mut i = input();
        i.isolation = Isolation::None;
        let none = deps_build_script(&i).expect("script");
        assert!(!none.contains("setpriv") && !none.contains("iptables"));
        assert!(none.contains("bash -c 'pnpm fetch'"));
    }
}

mod the_snapshot_restore_inside_the_checkout_script {
    use super::*;

    fn restore() -> DepsRestore {
        DepsRestore {
            strategy: PNPM,
            uri: format!(
                "s3://ratchet-test-artifacts/bench-deps/pnpm-{}/{}.tar.zst",
                "f".repeat(40),
                "d".repeat(64)
            ),
            sha256: "d".repeat(64),
            compression: Compression::Zstd,
            node: "v22.22.0".into(),
            pm: "10.28.0".into(),
        }
    }

    fn checkout(deps: Option<DepsRestore>, pinned: Vec<String>) -> Result<String, BenchError> {
        let mut i = CheckoutScriptInput::new(
            Layout::new("/var/tmp/ratchet-bench/opt1-1234"),
            archive("code.tgz"),
            archive("bench.tgz"),
            vec!["bench/**".into()],
        );
        i.pinned = pinned;
        i.deps = deps;
        i.isolation = Isolation::User;
        checkout_script(&i)
    }

    fn script() -> String {
        checkout(
            Some(restore()),
            PNPM.lockfiles.iter().map(|s| (*s).to_owned()).collect(),
        )
        .expect("script")
    }

    fn at(s: &str, needle: &str) -> usize {
        s.find(needle).unwrap_or_else(|| panic!("missing {needle}"))
    }

    #[test]
    fn happens_in_the_root_checkout_script_after_the_isolation_checks_and_never_through_setpriv() {
        let s = script();
        let start = at(&s, "# ---- dependency snapshot (root)");
        let fragment = &s[start..at(&s, "emit checkout")];
        assert!(!fragment.contains("setpriv") && !fragment.contains("sudo"));
        assert!(start > at(&s, "the holdout directory is readable by"));
        assert!(start > at(&s, "apply init"));
        assert!(start < at(&s, "emit checkout"));
    }

    #[test]
    fn verifies_the_digest_and_tool_versions_before_extracting_and_refuses_symlinks_after() {
        let s = script();
        assert!(at(&s, "sha256sum -- \"$D/in/snapshot\"") < at(&s, "zstd -dc"));
        assert!(at(&s, "node --version") < at(&s, "zstd -dc"));
        assert!(at(&s, "pnpm --version") < at(&s, "zstd -dc"));
        assert!(at(&s, "zstd -dc") < at(&s, "find \"$STORE\" -type l"));
        assert!(s.contains("= 'v22.22.0'") && s.contains("= '10.28.0'"));
    }

    #[test]
    fn hands_the_store_to_root_and_removes_group_other_write() {
        let s = script();
        assert!(s.contains("chown -R \"$BU:\" \"$SCRATCH\""));
        assert!(s.contains("chown -hR root:root \"$D\""));
        assert!(s.contains("chmod -R go-w,a+rX \"$D\""));
        assert!(s.contains("SCRATCH=\"$W/home/.ratchet-deps\""));
        assert!(s.contains("D=\"$W/deps\"; STORE=\"$D/store\""));
        assert!(at(&s, "chown -hR root:root \"$D\"") < at(&s, "chmod -R go-w,a+rX \"$D\""));
        // the root chown covers the store, scratch is not under it
        assert!(!s.contains("chown -R \"$BU:\" \"$D\""));
    }

    #[test]
    fn wires_the_strategy_restore_step_before_locking_the_store_down() {
        let s = script();
        assert!(at(&s, PNPM.restore) < at(&s, "chown -hR root:root \"$D\""));
    }

    #[test]
    fn the_lockfile_is_pinned_like_a_protected_path_but_does_not_count_as_the_user_protecting_something()
     {
        let s = script();
        assert!(
            s.contains("RE_USER=")
                && s.contains("matched=$(")
                && s.contains("\\\"matched\\\":$matched")
        );
        let re = regex::Regex::new(r"(?m)^RE='([^']*)'; RE_USER='([^']*)'").expect("re");
        let c = re.captures(&s).expect("RE line");
        assert!(c[1].contains("pnpm-lock"));
        assert!(!c[2].contains("pnpm-lock"));
    }

    #[test]
    fn a_gzip_snapshot_is_extracted_without_zstd_and_a_zstd_one_demands_it() {
        let mut gz = restore();
        gz.compression = Compression::Gzip;
        gz.uri = gz.uri.replace(".tar.zst", ".tgz");
        let s = checkout(Some(gz), vec![]).expect("gzip");
        assert!(s.contains("tar -xzf \"$D/in/snapshot\"") && !s.contains("zstd"));
        assert!(script().contains("zstd is missing on this image"));
    }

    #[test]
    fn is_absent_without_a_snapshot() {
        let none = checkout(None, vec![]).expect("script");
        assert!(!none.contains("dependency snapshot") && !none.contains("\"$D/in/snapshot\""));
    }

    fn refused(f: impl FnOnce(&mut DepsRestore)) {
        let mut r = restore();
        f(&mut r);
        assert!(matches!(
            checkout(Some(r), vec![]),
            Err(BenchError::Infra(_))
        ));
    }

    #[test]
    fn rejects_a_uri_outside_bench_deps() {
        refused(|r| r.uri = "s3://ratchet-test-artifacts/bench-inputs/x/code.tgz".into());
    }

    #[test]
    fn rejects_a_traversing_uri() {
        refused(|r| r.uri = "s3://b/bench-deps/../bench-inputs/x.tgz".into());
    }

    #[test]
    fn rejects_a_bad_digest() {
        refused(|r| r.sha256 = "nope".into());
    }

    #[test]
    fn rejects_a_shell_looking_version() {
        refused(|r| r.node = "v22'; rm -rf /; '".into());
    }
}
