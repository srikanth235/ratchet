mod common;

use std::collections::HashSet;

use common::{
    every_attempt_has_realistic_usage, git_apply, patch_order, repo_root, scratch_demo_tree,
};
use ratchet_agents::demo_scripts::demo_cli_script_dir;
use ratchet_agents::{DemoVariant, ScriptStep, load_demo_cli_script};

fn patch_files() -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(demo_cli_script_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".patch"))
        .collect();
    files.sort();
    files
}

#[test]
fn demo_cli_ships_the_seven_real_improvements() {
    assert_eq!(
        patch_files(),
        [
            "01-lazy-message-catalogs.patch",
            "02-lazy-spelling-dictionary.patch",
            "03-lazy-time-zones.patch",
            "04-bounded-config-and-plugin-scan.patch",
            "05-no-tput-shell-out.patch",
            "06-lazy-context.patch",
            "07-lazy-commands.patch",
        ]
    );
}

/// The cc and cx scripts play subsets of the seven, in different orders; every prefix applies. (The TypeScript test also built the
/// demo and ran its test suite after each order; that needs node and pnpm, see PARITY.md.)
#[test]
fn demo_cli_patches_apply_cumulatively_in_the_cc_and_cx_order() {
    let orders: [(&str, [&str; 5]); 2] = [
        ("cc", ["03", "01", "02", "04", "05"]),
        ("cx", ["01", "07", "02", "03", "06"]),
    ];
    let files = patch_files();
    for (name, order) in orders {
        let dir = scratch_demo_tree("demo-cli");
        for n in order {
            let f = files
                .iter()
                .find(|p| p.starts_with(&format!("{n}-")))
                .unwrap();
            let path = demo_cli_script_dir().join(f);
            assert!(
                git_apply(dir.path(), &path, true),
                "{name}: {f} does not apply (check)"
            );
            assert!(
                git_apply(dir.path(), &path, false),
                "{name}: {f} does not apply"
            );
        }
    }
}

#[test]
fn demo_cli_scripts_play_real_fixes_in_different_orders_and_include_every_kind_of_bad_attempt() {
    let cc = load_demo_cli_script(DemoVariant::Cc, None).unwrap();
    let cx = load_demo_cli_script(DemoVariant::Cx, None).unwrap();
    for s in [&cc, &cx] {
        let names: HashSet<String> = patch_order(s).into_iter().collect();
        assert!(names.len() >= 5);
        let steps: Vec<&ScriptStep> = s.attempts.iter().flat_map(|a| a.steps.iter()).collect();
        let edit_paths: Vec<&str> = steps
            .iter()
            .filter_map(|st| {
                if let ScriptStep::Edit { path, .. } = st {
                    Some(path.as_str())
                } else {
                    None
                }
            })
            .collect();
        let write_paths: Vec<&str> = steps
            .iter()
            .filter_map(|st| {
                if let ScriptStep::Write { path, .. } = st {
                    Some(path.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            edit_paths
                .iter()
                .any(|p| p.starts_with("bench/") || p.starts_with("tests/golden/"))
        ); // protected path
        assert!(write_paths.contains(&"src/snapshot.ts")); // overfit to the visible invocations
        assert!(
            edit_paths
                .iter()
                .any(|p| p.starts_with("src/render") || p.starts_with("src/store"))
        ); // noise-level no-op
        assert_eq!(s.attempts.last().unwrap().name, "wrap-up");
        assert!(every_attempt_has_realistic_usage(s));
    }
    assert_ne!(patch_order(&cc), patch_order(&cx));
}

#[test]
fn demo_cli_the_overfit_table_holds_exactly_the_outputs_of_the_visible_invocations_that_succeed() {
    let mix: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("examples/demo-cli/bench/invocations.json"))
            .unwrap(),
    )
    .unwrap();
    let mut expected: Vec<String> = mix["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["expect"].as_i64().unwrap_or(0) == 0)
        .map(|i| {
            i["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect::<Vec<_>>()
                .join("\0")
        })
        .collect();
    let script = load_demo_cli_script(DemoVariant::Cc, None).unwrap();
    let content = script
        .attempts
        .iter()
        .flat_map(|a| a.steps.iter())
        .find_map(|st| match st {
            ScriptStep::Write { path, content } if path == "src/snapshot.ts" => {
                Some(content.clone())
            }
            _ => None,
        })
        .expect("the overfit write step");
    let table_text = &content[content.find("= {").unwrap() + 2..=content.rfind("};").unwrap()];
    let table: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(table_text).unwrap();
    let mut keys: Vec<String> = table.keys().cloned().collect();
    keys.sort();
    expected.sort();
    assert_eq!(keys, expected);
}
