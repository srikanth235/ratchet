//! Automatic protected paths (docs/SIMPLIFIED_MODEL.md: "No knobs the system can derive").
//!
//! A candidate that changes a protected path is blocked, and the bench takes protected files from the
//! evaluator ref instead of the candidate. Which paths those are used to be typed under `protected:`; now most
//! are derived:
//!
//!  - `ratchet.yaml` itself (the config path in use);
//!  - the evaluator's files: the template's (`evaluator.template`), plus every `bench/` or `eval/` directory that
//!    `evaluator.run` references;
//!  - the environment: `environment.image` (and `.devcontainer/` when the image lives there) and the `infra` module;
//!  - lockfiles and install config ([`INSTALL_FILES`]): the bench has no network, so dependencies are those of
//!    the base ref;
//!  - the user's own `protected:` list, which only ADDS to the above.
//!
//! Port of `packages/core/src/protected-set.ts`.

use serde::{Deserialize, Serialize};

use crate::config::RatchetConfig;
use crate::protected::normalize_repo_path;
use crate::templates::find_evaluator_template;

/// Lockfiles and install configuration, at any depth. Dependency changes are not allowed in v0.
pub const INSTALL_FILES: [&str; 11] = [
    "pnpm-lock.yaml",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    "pnpm-workspace.yaml",
    ".pnpmfile.cjs",
];

/// Directory names whose contents are treated as the evaluator when `evaluator.run` points into them.
pub const EVALUATOR_DIRS: [&str; 2] = ["bench", "eval"];

/// Why a glob is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ProtectedSource {
    Ratchet,
    Evaluator,
    Environment,
    Install,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ProtectedEntry {
    pub glob: String,
    pub source: ProtectedSource,
    /// Plain words, for the UI and for the reason shown when a candidate is blocked.
    pub reason: String,
}

/// The slice of a parsed `ratchet.yaml` that decides what is protected. A [`RatchetConfig`] converts into it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtectedConfigInput {
    pub protected: Vec<String>,
    pub evaluator_run: Option<String>,
    pub evaluator_template: Option<String>,
    pub environment_image: Option<String>,
    pub infra: Option<String>,
}

impl From<&RatchetConfig> for ProtectedConfigInput {
    fn from(c: &RatchetConfig) -> Self {
        ProtectedConfigInput {
            protected: c.protected.clone(),
            evaluator_run: Some(c.evaluator.run.clone()),
            evaluator_template: c.evaluator.template.clone(),
            environment_image: c.environment.as_ref().and_then(|e| e.image.clone()),
            infra: c.infra.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EffectiveProtected {
    /// Everything, de-duplicated, in the order: config, evaluator, environment, install, user.
    pub entries: Vec<ProtectedEntry>,
    /// Every glob: what the engine blocks candidates on.
    pub globs: Vec<String>,
    /// Everything except lockfiles and install config: what the bench takes from the evaluator ref and requires
    /// to be non-empty. (Lockfiles are pinned separately through the dependency strategy.)
    pub evaluator_globs: Vec<String>,
    /// Only the lockfiles and install config.
    pub install_globs: Vec<String>,
    /// The evaluator's own files and the user's list: the globs of which at least one must match a file in the
    /// evaluator ref, or the bench refuses to run (an evaluator ref that holds none of them is not trusted).
    /// `ratchet.yaml`, the environment and the lockfiles are protected too but do not count here.
    pub required: Vec<String>,
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_segment_char(c: char) -> bool {
    is_word(c) || matches!(c, '.' | '@' | '-')
}

/// `bench/` and `eval/` directories referenced by an evaluation command. For `node dist/bench/x.js` that is
/// `dist/bench/**` and, as the likely source of the built file, `bench/**`. Paths that escape the repo are
/// ignored.
///
/// Equivalent to scanning with `/(?<![\w.@/])((?:\.\/)?(?:[\w.@-]+\/)*?)(bench|eval)(?=\/)/g`.
pub fn evaluator_dirs_from_run(run: &str) -> Vec<String> {
    let chars: Vec<char> = run.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |s: String| {
        if seen.insert(s.clone()) {
            out.push(s);
        }
    };
    let starts_with_at = |pos: usize, word: &str| -> bool {
        let w: Vec<char> = word.chars().collect();
        chars
            .get(pos..pos + w.len())
            .is_some_and(|s| s == w.as_slice())
    };

    let mut start = 0;
    while start < chars.len() {
        // lookbehind: the previous character must not be a word char, '.', '@' or '/'
        if start > 0 {
            let prev = chars[start - 1];
            if is_word(prev) || matches!(prev, '.' | '@' | '/') {
                start += 1;
                continue;
            }
        }
        match match_dir_at(&chars, start, &starts_with_at) {
            Some((prefix, dir, end)) => {
                let prefix = normalize_repo_path(&prefix);
                if prefix.split('/').any(|s| s == "..") || prefix.starts_with("..") {
                    // paths that leave the repo are ignored
                } else {
                    let dir_path = if prefix.is_empty() {
                        dir.to_owned()
                    } else {
                        format!("{}/{dir}", prefix.trim_end_matches('/'))
                    };
                    push(format!("{dir_path}/**"));
                    if !prefix.is_empty() {
                        push(format!("{dir}/**"));
                    }
                }
                start = end.max(start + 1);
            }
            None => start += 1,
        }
    }
    out
}

/// Try to match `((?:\./)?(?:[\w.@-]+/)*?)(bench|eval)(?=/)` at `start`. Returns the prefix text, the directory
/// name and the end of the match (just before the `/`).
fn match_dir_at(
    chars: &[char],
    start: usize,
    starts_with_at: &dyn Fn(usize, &str) -> bool,
) -> Option<(String, &'static str, usize)> {
    // Lazy repetition of "[\w.@-]+/" segments, with an optional leading "./". Each segment is the maximal run of
    // segment characters, which must be followed by '/'.
    let mut variants: Vec<usize> = vec![start];
    if starts_with_at(start, "./") {
        variants.insert(0, start + 2); // the optional group is greedy: try with it first
    }
    for from in variants {
        let mut pos = from;
        loop {
            for dir in EVALUATOR_DIRS {
                let end = pos + dir.len();
                if starts_with_at(pos, dir) && chars.get(end) == Some(&'/') {
                    let prefix: String = chars[start..pos].iter().collect();
                    return Some((prefix, dir, end));
                }
            }
            // consume one more segment
            let run = chars[pos..]
                .iter()
                .take_while(|c| is_segment_char(**c))
                .count();
            if run == 0 || chars.get(pos + run) != Some(&'/') {
                break;
            }
            pos += run + 1;
        }
    }
    None
}

/// The effective protected set for a config. Pure.
///
/// `config_path` is the path of the config inside the repo (default `ratchet.yaml`).
pub fn effective_protected(
    config: &ProtectedConfigInput,
    config_path: Option<&str>,
) -> EffectiveProtected {
    let mut entries: Vec<ProtectedEntry> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |glob: &str, source: ProtectedSource, reason: &str| {
        let g = glob.replace('\\', "/");
        let mut g = g.as_str();
        while let Some(rest) = g.strip_prefix("./") {
            g = rest;
        }
        let g = g.trim_start_matches('/');
        if g.is_empty() || !seen.insert(g.to_owned()) {
            return;
        }
        entries.push(ProtectedEntry {
            glob: g.to_owned(),
            source,
            reason: reason.to_owned(),
        });
    };

    let cfg = normalize_repo_path(config_path.unwrap_or("ratchet.yaml"));
    add(
        if cfg.is_empty() { "ratchet.yaml" } else { &cfg },
        ProtectedSource::Ratchet,
        "the optimization config",
    );

    let tpl = config
        .evaluator_template
        .as_deref()
        .and_then(find_evaluator_template);
    if let Some(t) = &tpl {
        for g in &t.protect {
            add(
                g,
                ProtectedSource::Evaluator,
                &format!("the {} evaluator", t.id),
            );
        }
    }
    if let Some(run) = config.evaluator_run.as_deref().filter(|r| !r.is_empty()) {
        for g in evaluator_dirs_from_run(run) {
            add(
                &g,
                ProtectedSource::Evaluator,
                "the evaluator referenced by evaluator.run",
            );
        }
    }

    let image = config
        .environment_image
        .as_deref()
        .map(normalize_repo_path)
        .unwrap_or_default();
    if !image.is_empty() {
        add(
            &image,
            ProtectedSource::Environment,
            "the environment image",
        );
        if image.starts_with(".devcontainer/") {
            add(
                ".devcontainer/**",
                ProtectedSource::Environment,
                "the environment image",
            );
        }
    }
    if let Some(infra) = config.infra.as_deref().filter(|i| !i.is_empty()) {
        add(
            &normalize_repo_path(infra),
            ProtectedSource::Environment,
            "the custom infrastructure module",
        );
    }

    for f in INSTALL_FILES {
        add(
            &format!("**/{f}"),
            ProtectedSource::Install,
            "dependencies are fixed: the bench has no network",
        );
    }

    for g in &config.protected {
        add(
            g,
            ProtectedSource::User,
            "listed under protected: in ratchet.yaml",
        );
    }

    let pick = |pred: &dyn Fn(ProtectedSource) -> bool| -> Vec<String> {
        entries
            .iter()
            .filter(|e| pred(e.source))
            .map(|e| e.glob.clone())
            .collect()
    };
    EffectiveProtected {
        globs: entries.iter().map(|e| e.glob.clone()).collect(),
        evaluator_globs: pick(&|s| s != ProtectedSource::Install),
        install_globs: pick(&|s| s == ProtectedSource::Install),
        required: pick(&|s| matches!(s, ProtectedSource::Evaluator | ProtectedSource::User)),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protected::find_protected_violations;

    fn base() -> ProtectedConfigInput {
        ProtectedConfigInput {
            evaluator_run: Some("node other/script.js".into()),
            ..Default::default()
        }
    }

    fn run(cmd: &str) -> ProtectedConfigInput {
        ProtectedConfigInput {
            evaluator_run: Some(cmd.into()),
            ..Default::default()
        }
    }

    fn template(id: &str) -> ProtectedConfigInput {
        ProtectedConfigInput {
            evaluator_template: Some(id.into()),
            ..Default::default()
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    // protected-set.test.ts evaluatorDirsFromRun: "finds bench/ and eval/ directories referenced by the command"
    #[test]
    fn finds_bench_and_eval_directories_referenced_by_the_command() {
        assert_eq!(
            evaluator_dirs_from_run("k6 run bench/k6/search.js"),
            strs(&["bench/**"])
        );
        assert_eq!(
            evaluator_dirs_from_run("node eval/backtest.mjs"),
            strs(&["eval/**"])
        );
        assert_eq!(
            evaluator_dirs_from_run("node ./bench/run.mjs && node eval/check.mjs"),
            strs(&["bench/**", "eval/**"])
        );
    }

    // "treats a built copy as the evaluator and also protects the sources it is built from"
    #[test]
    fn treats_a_built_copy_as_the_evaluator() {
        assert_eq!(
            evaluator_dirs_from_run("node dist/bench/hyperfine.js --runs 10"),
            strs(&["dist/bench/**", "bench/**"])
        );
    }

    // "sees paths inside shell expansions and quotes"
    #[test]
    fn sees_paths_inside_shell_expansions_and_quotes() {
        let run = "node dist/bench/hyperfine.js --mix \"${RATCHET_TRACE:-bench/invocations.json}\"";
        assert_eq!(
            evaluator_dirs_from_run(run),
            strs(&["dist/bench/**", "bench/**"])
        );
    }

    // "ignores words that merely look similar, absolute paths and paths that leave the repo"
    #[test]
    fn ignores_lookalikes_absolute_paths_and_paths_that_leave_the_repo() {
        assert_eq!(
            evaluator_dirs_from_run("node benchmark/run.js"),
            Vec::<String>::new()
        );
        assert_eq!(
            evaluator_dirs_from_run("node my-evaluation/run.js"),
            Vec::<String>::new()
        );
        assert_eq!(
            evaluator_dirs_from_run("node /opt/bench/run.js"),
            Vec::<String>::new()
        );
        assert_eq!(
            evaluator_dirs_from_run("node ../bench/run.js"),
            Vec::<String>::new()
        );
        assert_eq!(evaluator_dirs_from_run("make eval"), Vec::<String>::new());
    }

    // "always protects the config and the lockfiles / install config, at any depth"
    #[test]
    fn always_protects_the_config_and_the_install_files_at_any_depth() {
        let p = effective_protected(&base(), None);
        assert!(p.globs.contains(&"ratchet.yaml".to_owned()));
        for f in INSTALL_FILES {
            assert!(p.globs.contains(&format!("**/{f}")), "{f}");
        }
        let hits = find_protected_violations(
            &[
                "pnpm-lock.yaml",
                "packages/api/package-lock.json",
                ".npmrc",
                "pnpm-workspace.yaml",
                ".pnpmfile.cjs",
                "npm-shrinkwrap.json",
                "src/index.ts",
            ],
            &p.globs,
        );
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "pnpm-lock.yaml",
                "packages/api/package-lock.json",
                ".npmrc",
                "pnpm-workspace.yaml",
                ".pnpmfile.cjs",
                "npm-shrinkwrap.json"
            ]
        );
    }

    // "uses the config path in use"
    #[test]
    fn uses_the_config_path_in_use() {
        assert_eq!(
            effective_protected(&base(), Some("ops/ratchet.yaml")).globs[0],
            "ops/ratchet.yaml"
        );
        assert_eq!(
            effective_protected(&base(), Some("./ratchet.yaml")).globs[0],
            "ratchet.yaml"
        );
    }

    // "protects what evaluator.run references under bench/ or eval/"
    #[test]
    fn protects_what_evaluator_run_references() {
        let p = effective_protected(&run("k6 run bench/k6/search.js"), None);
        let v = find_protected_violations(
            &["bench/k6/search.js", "bench/query-mix.json", "src/app.ts"],
            &p.globs,
        );
        assert_eq!(
            v.iter().map(|v| v.path.as_str()).collect::<Vec<_>>(),
            ["bench/k6/search.js", "bench/query-mix.json"]
        );
    }

    // "protects a template's evaluator files, even when run is not given"
    #[test]
    fn protects_a_templates_evaluator_files() {
        let p = effective_protected(&template("forecast-backtest"), None);
        assert!(p.globs.contains(&"eval/**".to_owned()) && p.globs.contains(&"data/**".to_owned()));
        let e = p.entries.iter().find(|e| e.glob == "eval/**").unwrap();
        assert_eq!(e.source, ProtectedSource::Evaluator);
        assert!(
            effective_protected(&template("http-latency"), None)
                .globs
                .contains(&"bench/**".to_owned())
        );
    }

    // "an unknown template adds nothing"
    #[test]
    fn an_unknown_template_adds_nothing() {
        assert!(
            !effective_protected(&template("nope"), None)
                .globs
                .contains(&"bench/**".to_owned())
        );
    }

    // "protects the environment image and a custom infra module"
    #[test]
    fn protects_the_environment_image_and_a_custom_infra_module() {
        let c = ProtectedConfigInput {
            environment_image: Some(".devcontainer/Dockerfile".into()),
            infra: Some("ops/tofu".into()),
            ..base()
        };
        let p = effective_protected(&c, None);
        for g in [".devcontainer/Dockerfile", ".devcontainer/**", "ops/tofu"] {
            assert!(p.globs.contains(&g.to_owned()), "{g}");
        }
        assert_eq!(
            find_protected_violations(
                &["ops/tofu/main.tf", ".devcontainer/devcontainer.json"],
                &p.globs
            )
            .len(),
            2
        );
        let c = ProtectedConfigInput {
            environment_image: Some("Dockerfile.bench".into()),
            ..base()
        };
        assert!(
            effective_protected(&c, None)
                .globs
                .contains(&"Dockerfile.bench".to_owned())
        );
    }

    // "the user's protected: list only adds to the automatic set"
    #[test]
    fn the_users_protected_list_only_adds() {
        let c = ProtectedConfigInput {
            protected: strs(&["tests/contract/**", "ratchet.yaml", "bench/**"]),
            ..base()
        };
        let p = effective_protected(&c, None);
        let user: Vec<&str> = p
            .entries
            .iter()
            .filter(|e| e.source == ProtectedSource::User)
            .map(|e| e.glob.as_str())
            .collect();
        assert_eq!(user, ["tests/contract/**", "bench/**"]);
        assert_eq!(p.globs.iter().filter(|g| *g == "ratchet.yaml").count(), 1); // not duplicated
        assert!(p.globs.contains(&"**/pnpm-lock.yaml".to_owned()));
    }

    // "is deterministic and de-duplicated"
    #[test]
    fn is_deterministic_and_deduplicated() {
        let c = ProtectedConfigInput {
            evaluator_template: Some("http-latency".into()),
            evaluator_run: Some("node bench/http-latency.mjs".into()),
            protected: strs(&["bench/**"]),
            ..Default::default()
        };
        let a = effective_protected(&c, None);
        assert_eq!(effective_protected(&c, None), a);
        let set: std::collections::HashSet<_> = a.globs.iter().collect();
        assert_eq!(set.len(), a.globs.len());
    }

    // "splits install files from the evaluator set"
    #[test]
    fn splits_install_files_from_the_evaluator_set() {
        let c = ProtectedConfigInput {
            protected: strs(&["extra/**"]),
            ..base()
        };
        let p = effective_protected(&c, None);
        assert_eq!(p.evaluator_globs, strs(&["ratchet.yaml", "extra/**"]));
        assert_eq!(p.install_globs.len(), INSTALL_FILES.len());
        let mut both: Vec<String> = p
            .evaluator_globs
            .iter()
            .chain(&p.install_globs)
            .cloned()
            .collect();
        let mut all = p.globs.clone();
        both.sort();
        all.sort();
        assert_eq!(both, all);
    }

    // "gives every entry a reason in plain words"
    #[test]
    fn gives_every_entry_a_reason() {
        let c = ProtectedConfigInput {
            evaluator_template: Some("memory".into()),
            protected: strs(&["x"]),
            ..Default::default()
        };
        for e in effective_protected(&c, None).entries {
            assert!(e.reason.len() > 5, "{e:?}");
        }
    }

    // "required globs: are the evaluator files and the user list, not ratchet.yaml, the environment or lockfiles"
    #[test]
    fn required_globs_are_the_evaluator_files_and_the_user_list() {
        let c = ProtectedConfigInput {
            evaluator_run: Some("node bench/run.mjs".into()),
            environment_image: Some("Dockerfile".into()),
            protected: strs(&["tests/**"]),
            ..Default::default()
        };
        let p = effective_protected(&c, None);
        assert_eq!(p.required, strs(&["bench/**", "tests/**"]));
        assert_eq!(
            p.evaluator_globs,
            strs(&["ratchet.yaml", "bench/**", "Dockerfile", "tests/**"])
        );
    }

    // "are empty when nothing marks the evaluator (the bench then refuses to run)"
    #[test]
    fn required_is_empty_when_nothing_marks_the_evaluator() {
        assert_eq!(
            effective_protected(&run("sh ./score.sh"), None).required,
            Vec::<String>::new()
        );
    }
}
