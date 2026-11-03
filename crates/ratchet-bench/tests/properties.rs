//! Laws that must keep holding for the pure parts of the bench.

use std::process::Command;

use proptest::prelude::*;
use ratchet_bench::deps::{FileHashes, lockfile_changes};
use ratchet_bench::image::{normalize_repo_relative, parse_dockerfile, parse_jsonc};
#[cfg(target_os = "linux")]
use ratchet_bench::launcher::{build_plan, parse_conf};
use ratchet_bench::pipeline::parse_bench_output;
use ratchet_bench::shell::{fill, js_cmp, sq};
use ratchet_bench::{flatten_numeric, protected_pcre};
#[cfg(target_os = "linux")]
use std::ffi::OsString;

fn hashes(pairs: &[(String, String)]) -> FileHashes {
    pairs.iter().cloned().collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// Whatever the text, a shell reads `sq(text)` back as exactly that text.
    #[test]
    fn sq_round_trips_through_bash(s in "[^\\x00]{0,40}") {
        let out = Command::new("bash").args(["-c", &format!("printf %s {}", sq(&s))]).output().expect("bash");
        prop_assert_eq!(String::from_utf8_lossy(&out.stdout).into_owned(), s);
    }
}

proptest! {
    #[test]
    fn fill_substitutes_each_placeholder_once_and_never_rescans(a in "[a-z{}]{0,12}", b in "[a-z{}]{0,12}") {
        let out = fill("<{{x}}|{{y}}>", &[("x", &a), ("y", &b)]);
        prop_assert_eq!(out, format!("<{a}|{b}>"));
    }

    #[test]
    fn utf16_order_is_a_total_order(a in ".{0,6}", b in ".{0,6}", c in ".{0,6}") {
        use std::cmp::Ordering::{Equal, Greater, Less};
        prop_assert_eq!(js_cmp(&a, &b), js_cmp(&b, &a).reverse());
        if js_cmp(&a, &b) != Greater && js_cmp(&b, &c) != Greater {
            prop_assert!(js_cmp(&a, &c) != Greater);
        }
        prop_assert_eq!(js_cmp(&a, &a), Equal);
        let _ = Less;
    }

    /// The lockfile comparison is symmetric, sorted, and empty exactly for equal maps.
    #[test]
    fn lockfile_changes_is_symmetric_and_sorted(
        base in proptest::collection::vec(("[a-c]{1,2}", "[0-2]"), 0..5),
        cand in proptest::collection::vec(("[a-c]{1,2}", "[0-2]"), 0..5),
    ) {
        let (b, c) = (hashes(&base), hashes(&cand));
        let ab = lockfile_changes(&b, &c);
        prop_assert_eq!(&ab, &lockfile_changes(&c, &b));
        let mut sorted = ab.clone();
        sorted.sort();
        prop_assert_eq!(&ab, &sorted);
        prop_assert_eq!(ab.is_empty(), b == c);
    }

    /// An environment path either stays inside the repository or is refused; it never starts with `./`, `/` or `..`.
    #[test]
    fn repo_relative_paths_never_escape(p in "[a-z./\\\\]{0,16}") {
        if let Ok(n) = normalize_repo_relative(&p, "x") {
            prop_assert!(!n.starts_with('/') && !n.starts_with("./") && n != ".." && !n.starts_with("../"), "{n}");
        }
    }

    /// Parsers of repository text never panic.
    #[test]
    fn parsers_do_not_panic(s in ".{0,200}") {
        let _ = parse_dockerfile(&s);
        let _ = parse_jsonc(&s);
        #[cfg(target_os = "linux")]
        let _ = parse_conf(&s);
        let _ = parse_bench_output(&s, "a.b");
    }

    #[test]
    fn protected_pcre_does_not_panic_and_is_never_empty(globs in proptest::collection::vec(".{0,10}", 0..4)) {
        prop_assert!(!protected_pcre(&globs).is_empty());
    }

    /// The launcher's environment never carries the holdout or credentials, whatever the arguments are.
    #[cfg(target_os = "linux")]
    #[test]
    fn launcher_environment_never_has_the_holdout_or_aws(args in proptest::collection::vec("[A-Z_]{0,6}(=[a-z]{0,3})?", 0..6)) {
        let vars = [("RATCHET_CANDIDATE_DIR", "/w"), ("RATCHET_CANDIDATE_HOME", "/w"), ("RATCHET_CANDIDATE_TMP", "/w"), ("RATCHET_TRACE", "secret"), ("AWS_SECRET_ACCESS_KEY", "x")];
        let argv: Vec<OsString> = args.iter().map(OsString::from).collect();
        let getenv = |name: &str| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v));
        if let Ok(plan) = build_plan(&argv, getenv) {
            for e in &plan.env {
                let e = e.to_string_lossy();
                prop_assert!(!e.starts_with("RATCHET_TRACE=") && !e.starts_with("AWS_"), "{e}");
            }
        }
    }
}

#[test]
fn flatten_numeric_keeps_only_finite_numbers() {
    let v = serde_json::json!({"a": 1, "b": {"c": 2.5, "d": [1]}, "e": "x"});
    let m = flatten_numeric(&v);
    assert!(m.values().all(|x| x.is_finite()));
    assert_eq!(m.len(), 2);
}
