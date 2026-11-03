mod common;

use std::collections::HashSet;

use common::{every_attempt_has_realistic_usage, git_apply, patch_order, scratch_demo_tree};
use ratchet_agents::demo_scripts::demo_forecast_script_dir;
use ratchet_agents::{DemoVariant, ScriptStep, load_demo_forecast_script};

fn patch_files() -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(demo_forecast_script_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".patch"))
        .collect();
    files.sort();
    files
}

#[test]
fn demo_forecast_ships_the_five_real_improvements() {
    assert_eq!(
        patch_files(),
        [
            "01-seasonal-naive.patch",
            "02-smoothed-level-seasonal-index.patch",
            "03-promo-holiday-effects.patch",
            "04-pooled-seasonal-profile.patch",
            "05-stockout-bias-correction.patch",
        ]
    );
}

/// 03 and 05 are independent of the model; 02 builds on 01; 04 builds on 02. (The TypeScript test also ran the demo's own test suite
/// after each order; that needs node, see PARITY.md.)
#[test]
fn demo_forecast_patches_apply_cumulatively_in_the_cc_and_cx_order() {
    let orders: [(&str, [&str; 5]); 2] = [
        ("cc", ["01", "02", "03", "04", "05"]),
        ("cx", ["03", "01", "05", "02", "04"]),
    ];
    let files = patch_files();
    for (name, order) in orders {
        let dir = scratch_demo_tree("demo-forecast");
        for n in order {
            let f = files
                .iter()
                .find(|p| p.starts_with(&format!("{n}-")))
                .unwrap();
            let path = demo_forecast_script_dir().join(f);
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
fn demo_forecast_04_needs_02() {
    let dir = scratch_demo_tree("demo-forecast");
    assert!(!git_apply(
        dir.path(),
        &demo_forecast_script_dir().join("04-pooled-seasonal-profile.patch"),
        true
    ));
}

#[test]
fn demo_forecast_scripts_play_the_same_fixes_in_different_orders_and_include_every_kind_of_bad_attempt()
 {
    let cc = load_demo_forecast_script(DemoVariant::Cc, None).unwrap();
    let cx = load_demo_forecast_script(DemoVariant::Cx, None).unwrap();
    for s in [&cc, &cx] {
        let names: HashSet<String> = patch_order(s).into_iter().collect();
        assert!(names.len() >= 5);
        let steps: Vec<&ScriptStep> = s.attempts.iter().flat_map(|a| a.steps.iter()).collect();
        let edits: Vec<(&str, &str)> = steps
            .iter()
            .filter_map(|st| {
                if let ScriptStep::Edit { path, find, .. } = st {
                    Some((path.as_str(), find.as_str()))
                } else {
                    None
                }
            })
            .collect();
        assert!(
            edits
                .iter()
                .any(|(p, _)| p.starts_with("eval/") || p.starts_with("data/"))
        ); // protected path
        assert!(
            edits
                .iter()
                .any(|(_, f)| f.contains("table.units") || f.contains("export function predict"))
        ); // leakage / overfit
        assert!(steps.iter().any(
            |st| matches!(st, ScriptStep::Write { path, .. } if path == "src/calibration.ts")
        )); // overfit
        assert!(edits.iter().any(|(_, f)| *f == "export const WINDOW = 4;")); // noise-level no-op
        assert_eq!(s.attempts.last().unwrap().name, "wrap-up");
        assert!(every_attempt_has_realistic_usage(s));
    }
    assert_ne!(patch_order(&cc), patch_order(&cx));
}
