//! Property tests for the pieces that must keep their behaviour exactly: statistics, auto-calibration, the
//! ratchet rule, protected-path matching, and the number formatting the messages rely on. (The golden tests
//! compare with the TypeScript on fixed inputs; these check the laws on random ones.)

use proptest::prelude::*;
use ratchet_core::bench::calibrate::{CalibrationInput, calibrate, calibration, nice_gain};
use ratchet_core::bench::stats::{
    NoiseFloorOptions, coefficient_of_variation, mean, median, noise_floor, stddev, summarize,
};
use ratchet_core::config::GuardrailResult;
use ratchet_core::enums::{Direction, Verdict};
use ratchet_core::numfmt::{js_number_to_string, to_fixed, to_precision};
use ratchet_core::protected::{
    ProtectedViolation, find_protected_violations, matches_glob, normalize_repo_path,
};
use ratchet_core::ratchet::{RatchetInput, evaluate_ratchet, improvement_of};

fn samples() -> impl Strategy<Value = Vec<f64>> {
    prop::collection::vec(-1.0e6f64..1.0e6, 1..12)
}

fn positive_samples() -> impl Strategy<Value = Vec<f64>> {
    prop::collection::vec(1.0f64..1.0e4, 3..12)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    // ---------------------------------------------------------------- statistics

    #[test]
    fn mean_and_median_lie_between_min_and_max(xs in samples()) {
        let s = summarize(&xs).unwrap();
        let slack = 1e-9 * s.max.abs().max(s.min.abs()).max(1.0);
        prop_assert!(s.min - slack <= s.mean && s.mean <= s.max + slack);
        prop_assert!(s.min <= s.median && s.median <= s.max);
        prop_assert_eq!(s.mean, mean(&xs).unwrap());
        prop_assert_eq!(s.median, median(&xs).unwrap());
    }

    #[test]
    fn stddev_and_cv_are_non_negative_and_zero_for_constants(xs in samples(), c in -1.0e5f64..1.0e5, n in 1usize..8) {
        prop_assert!(stddev(&xs).unwrap() >= 0.0);
        prop_assert!(coefficient_of_variation(&xs).unwrap() >= 0.0);
        // equal samples: zero up to the rounding of their mean (as in the TypeScript, which has the same sums)
        let constant = vec![c; n];
        prop_assert!(stddev(&constant).unwrap() <= 1e-9 * c.abs().max(1.0));
    }

    #[test]
    fn stddev_is_shift_invariant_and_scale_equivariant(xs in positive_samples(), shift in -1.0e3f64..1.0e3, k in 0.5f64..8.0) {
        let sd = stddev(&xs).unwrap();
        let shifted: Vec<f64> = xs.iter().map(|x| x + shift).collect();
        let scaled: Vec<f64> = xs.iter().map(|x| x * k).collect();
        prop_assert!((stddev(&shifted).unwrap() - sd).abs() <= 1e-6 * sd.max(1.0));
        prop_assert!((stddev(&scaled).unwrap() - k * sd).abs() <= 1e-6 * (k * sd).max(1.0));
        // the coefficient of variation does not depend on the unit
        let cv = coefficient_of_variation(&xs).unwrap();
        prop_assert!((coefficient_of_variation(&scaled).unwrap() - cv).abs() <= 1e-9 * cv.max(1.0));
    }

    #[test]
    fn noise_floor_is_monotone_in_k_and_bounded_below(xs in samples(), k1 in 0.0f64..5.0, dk in 0.0f64..5.0, min in 0.0f64..10.0) {
        let a = noise_floor(&xs, NoiseFloorOptions { k: Some(k1), min_absolute: Some(min) }).unwrap();
        let b = noise_floor(&xs, NoiseFloorOptions { k: Some(k1 + dk), min_absolute: Some(min) }).unwrap();
        prop_assert!(a >= min && b >= a);
    }

    // ---------------------------------------------------------------- calibration

    #[test]
    fn auto_calibration_follows_the_documented_formula(xs in positive_samples()) {
        let c = calibrate(&CalibrationInput { samples: &xs, ..Default::default() }).unwrap();
        let cv = c.cv.expect("positive samples with at least 3 runs have a CV");
        // g = max(2 CV, 1%); n = ceil(18 CV^2 / g^2) clamped to 3..15
        let g = (2.0 * cv).max(0.01);
        let wanted = (18.0 * cv * cv / (g * g) - 1e-9).ceil().max(3.0);
        prop_assert_eq!(c.repeats as f64, wanted.min(15.0));
        prop_assert_eq!(c.repeats_capped, wanted > 15.0);
        prop_assert!((calibration::MIN_REPEATS..=calibration::MAX_REPEATS).contains(&c.repeats));
        // never below the floor, never below what the chosen repeats can resolve, never above 100%
        prop_assert!(c.min_gain >= 0.01 - 1e-12);
        // (the threshold is capped at 100%, whatever the noise)
        prop_assert!(c.min_gain >= c.resolvable.unwrap().min(1.0) - 1e-9);
        prop_assert!(c.min_gain >= g.min(1.0) - 1e-9);
        prop_assert!(c.min_gain <= 1.0);
    }

    #[test]
    fn a_noisier_baseline_never_gets_a_lower_threshold(cv1 in 0.0005f64..0.5, factor in 1.0f64..4.0) {
        // two baselines with the same shape, the second `factor` times as noisy
        let shape = [1.0, -1.0, 1.0, -1.0, 0.0];
        let mk = |cv: f64| -> Vec<f64> {
            let m = shape.iter().sum::<f64>() / 5.0;
            let sd = (shape.iter().map(|r| (r - m).powi(2)).sum::<f64>() / 4.0).sqrt();
            shape.iter().map(|r| 100.0 + (r / sd) * 100.0 * cv).collect()
        };
        let (a, b) = (mk(cv1), mk(cv1 * factor));
        let ca = calibrate(&CalibrationInput { samples: &a, ..Default::default() }).unwrap();
        let cb = calibrate(&CalibrationInput { samples: &b, ..Default::default() }).unwrap();
        prop_assert!(cb.min_gain >= ca.min_gain - 1e-12);
    }

    #[test]
    fn explicit_settings_are_respected(xs in positive_samples(), repeats in 1u64..30, gain in 0.0f64..1.0) {
        let c = calibrate(&CalibrationInput { samples: &xs, repeats: Some(repeats), min_gain: Some(gain), noise_factor: Some(3.0) }).unwrap();
        prop_assert_eq!((c.repeats, c.min_gain, c.noise_factor), (repeats, gain, 3.0));
        prop_assert!(!c.repeats_capped);
    }

    #[test]
    fn nice_gain_rounds_up_to_a_step_and_stays_in_range(x in 0.0f64..3.0) {
        let n = nice_gain(x);
        prop_assert!(n > 0.0 && n <= 1.0);
        if x == 0.0 {
            prop_assert_eq!(n, 0.01); // the floor, for "nothing to resolve"
        } else if x <= 1.0 {
            let step = if x <= 0.02 { 0.005 } else if x <= 0.2 { 0.01 } else { 0.05 };
            prop_assert!(n >= x - 1e-6);
            prop_assert!(n - x <= step + 1e-6);
        }
    }

    // ---------------------------------------------------------------- the ratchet rule

    #[test]
    fn the_verdict_follows_the_gates(
        best in 1.0f64..1000.0,
        deltas in prop::collection::vec(-0.5f64..0.5, 1..6),
        min_gain in 0.0f64..0.2,
        noise in 0.0f64..20.0,
        minimize in any::<bool>(),
        confirm in prop::option::of(prop::collection::vec(-0.5f64..0.5, 0..4)),
        failed_guardrail in any::<bool>(),
        violation in any::<bool>(),
    ) {
        let direction = if minimize { Direction::Minimize } else { Direction::Maximize };
        let input = RatchetInput {
            direction,
            min_gain,
            noise_floor: noise,
            current_best: best,
            candidate: deltas.iter().map(|d| best * (1.0 + d)).collect(),
            confirm: confirm.as_ref().map(|c| c.iter().map(|d| best * (1.0 + d)).collect()),
            confirm_best: None,
            guardrails: vec![GuardrailResult { name: "g".into(), passed: !failed_guardrail, detail: None }],
            protected_violations: if violation { vec![ProtectedViolation { path: "p".into(), pattern: "q".into() }] } else { vec![] },
        };
        let out = evaluate_ratchet(&input).unwrap();
        let candidate_value = median(&input.candidate).unwrap();
        let improvement = improvement_of(direction, best, candidate_value);
        let required = noise.max(min_gain * best.abs());
        prop_assert_eq!(out.required_gain, required);
        prop_assert_eq!(out.improvement, improvement);

        if failed_guardrail || violation {
            // blocked wins over everything, however good the result is
            prop_assert_eq!(out.verdict, Verdict::Blocked);
            prop_assert_eq!(out.reasons.len(), usize::from(failed_guardrail) + usize::from(violation));
        } else if improvement <= required {
            prop_assert_eq!(out.verdict, Verdict::Rejected);
        } else {
            match input.confirm.as_deref().filter(|c| !c.is_empty()) {
                None => prop_assert_eq!(out.verdict, Verdict::PendingConfirm),
                Some(c) => {
                    let ci = improvement_of(direction, best, median(c).unwrap());
                    prop_assert_eq!(out.confirm_improvement, Some(ci));
                    prop_assert_eq!(out.verdict, if ci > 0.0 { Verdict::Verified } else { Verdict::FailedConfirm });
                }
            }
        }
    }

    #[test]
    fn minimizing_and_maximizing_the_negation_agree(
        best in 1.0f64..1000.0,
        deltas in prop::collection::vec(-0.5f64..0.5, 1..5),
        min_gain in 0.0f64..0.2,
        noise in 0.0f64..20.0,
        confirm in prop::option::of(prop::collection::vec(-0.5f64..0.5, 1..4)),
    ) {
        let cand: Vec<f64> = deltas.iter().map(|d| best * (1.0 + d)).collect();
        let conf = confirm.map(|c| c.iter().map(|d| best * (1.0 + d)).collect::<Vec<f64>>());
        let min = RatchetInput {
            direction: Direction::Minimize, min_gain, noise_floor: noise, current_best: best,
            candidate: cand.clone(), confirm: conf.clone(), confirm_best: None, guardrails: vec![], protected_violations: vec![],
        };
        let max = RatchetInput {
            direction: Direction::Maximize, min_gain, noise_floor: noise, current_best: -best,
            candidate: cand.iter().map(|x| -x).collect(), confirm: conf.map(|c| c.iter().map(|x| -x).collect()),
            confirm_best: None, guardrails: vec![], protected_violations: vec![],
        };
        let (a, b) = (evaluate_ratchet(&min).unwrap(), evaluate_ratchet(&max).unwrap());
        prop_assert_eq!(a.verdict, b.verdict);
        prop_assert!((a.improvement - b.improvement).abs() <= 1e-9 * a.improvement.abs().max(1.0));
    }

    // ---------------------------------------------------------------- protected paths

    #[test]
    fn matching_never_panics_on_arbitrary_text(path in ".{0,40}", glob in ".{0,20}") {
        let _ = matches_glob(&path, &glob);
        let _ = normalize_repo_path(&path);
        let _ = find_protected_violations(&[path.as_str()], &[glob.as_str()]);
    }

    #[test]
    fn a_literal_path_protects_itself_and_everything_below_it(segs in prop::collection::vec("[a-z][a-z0-9_.-]{0,6}", 1..5), child in "[a-z]{1,5}") {
        let path = segs.join("/");
        prop_assume!(!segs.iter().any(|s| s == "." || s == ".."));
        let below = format!("{path}/{child}/x");
        let direct_child = format!("{path}/{child}");
        let star = format!("{path}/**");
        let sibling = format!("{path}x");
        prop_assert!(matches_glob(&path, &path));
        prop_assert!(matches_glob(&below, &path));
        prop_assert!(matches_glob(&direct_child, &star));
        // a sibling that merely shares the prefix is not protected
        prop_assert!(!matches_glob(&sibling, &path));
    }

    #[test]
    fn dir_double_star_matches_exactly_what_is_underneath(dir in "[a-z]{1,6}", rest in prop::collection::vec("[a-z.]{1,5}", 1..4)) {
        prop_assume!(!rest.iter().any(|s| s == "." || s == ".."));
        let tail = rest.join("/");
        let under = format!("{dir}/{tail}");
        let elsewhere = format!("src/{under}");
        let lookalike = format!("{dir}x/{tail}");
        let glob = format!("{dir}/**");
        prop_assert!(matches_glob(&under, &glob));
        prop_assert!(!matches_glob(&elsewhere, &glob));
        prop_assert!(!matches_glob(&lookalike, &glob));
    }

    #[test]
    fn a_single_star_never_crosses_a_directory(a in "[a-z]{1,5}", b in "[a-z]{1,5}", c in "[a-z]{1,5}") {
        let glob = format!("{a}/*.ts");
        let flat = format!("{a}/{b}.ts");
        let deep = format!("{a}/{b}/{c}.ts");
        prop_assert!(matches_glob(&flat, &glob));
        prop_assert!(!matches_glob(&deep, &glob));
    }

    #[test]
    fn violations_are_normalised_unique_and_use_a_given_pattern(
        paths in prop::collection::vec("[a-z./]{0,12}", 0..8),
        globs in prop::collection::vec("[a-z*?/.]{1,8}", 0..4),
    ) {
        let v = find_protected_violations(&paths, &globs);
        let mut seen = std::collections::HashSet::new();
        for item in &v {
            prop_assert!(seen.insert(item.path.clone()));
            prop_assert_eq!(&item.path, &normalize_repo_path(&item.path));
            prop_assert!(globs.contains(&item.pattern));
            // the pattern reported is the first one that matches
            let first = globs.iter().find(|g| matches_glob(&item.path, g)).unwrap();
            prop_assert_eq!(&item.pattern, first);
        }
        // adding a glob never removes a violation
        let mut more = globs.clone();
        more.push("**".into());
        prop_assert!(find_protected_violations(&paths, &more).len() >= v.len());
    }

    // ---------------------------------------------------------------- number formatting

    #[test]
    fn numbers_print_and_read_back_the_way_javascript_does(bits in any::<u64>()) {
        let x = f64::from_bits(bits);
        prop_assume!(x.is_finite());
        // String(x) is the shortest text that reads back as x
        prop_assert_eq!(js_number_to_string(x).parse::<f64>().unwrap(), if x == 0.0 { 0.0 } else { x });
    }

    #[test]
    fn to_fixed_and_to_precision_round_to_the_requested_place(x in 0.0f64..1.0e9, d in 0usize..7) {
        let fixed: f64 = to_fixed(x, d).parse().unwrap();
        prop_assert!((fixed - x).abs() <= 0.5 * 10f64.powi(-(d as i32)) * (1.0 + 1e-9) + x * 1e-15);
        if x > 0.0 {
            let p: f64 = to_precision(x, 6).parse().unwrap();
            prop_assert!(((p - x) / x).abs() <= 5.0e-6 * (1.0 + 1e-9));
        }
    }
}
