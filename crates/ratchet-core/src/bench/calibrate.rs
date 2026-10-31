//! Auto-calibrated statistics (docs/SIMPLIFIED_MODEL.md: "no knobs the system can derive").
//!
//! From the baseline's coefficient of variation (CV = stddev / mean of the baseline runs) Ratchet derives what
//! the user used to type:
//!
//! ```text
//! resolution to detect   g       = max(2 x CV, 1%)           the smallest gain worth resolving
//! repeats per candidate  n       = ceil(2 z^2 CV^2 / g^2)    z = 3 (so 18 CV^2 / g^2), clamped to [3, 15]
//! win threshold          minGain = nice(max(g, z sqrt(2) CV / sqrt(n)))
//! ```
//!
//! `z sqrt(2) CV / sqrt(n)` is the gain at which the difference of two n-run medians stands z standard errors
//! above zero, so `n` is "enough repeats that noise cannot fake a win of g". When the cap is hit, `minGain`
//! rises to what the capped repeats can actually resolve instead of pretending. Any of `repeats` / `minGain` /
//! `noiseFactor` set in ratchet.yaml is respected as-is and the other is derived around it.
//!
//! Port of `packages/core/src/bench/calibrate.ts`; behaviour is identical (see the golden tests).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::stats::{StatsError, summarize};
use crate::numfmt::{fixed_trimmed, to_fixed};

/// Tunables of the derivation. One place to look for tests and docs.
pub mod calibration {
    /// Repeats used for the baseline itself when ratchet.yaml gives none; also the sample the CV is measured on.
    pub const BASELINE_REPEATS: u64 = 5;
    pub const MIN_REPEATS: u64 = 3;
    pub const MAX_REPEATS: u64 = 15;
    /// Smallest gain Ratchet ever tries to resolve (a fraction).
    pub const FLOOR_GAIN: f64 = 0.01;
    /// Standard errors a win must stand above zero.
    pub const Z: f64 = 3.0;
    /// `2 x CV` is the "noise" shown to the user: the band that holds ~95% of single runs.
    pub const NOISE_SIGMAS: f64 = 2.0;
    /// Used when the baseline is too small or degenerate to estimate noise from.
    pub const FALLBACK_GAIN: f64 = 0.03;
    /// The default k of the noise floor `k x stddev`.
    pub const NOISE_FACTOR: f64 = 2.0;
}

use calibration as cal;

/// Where a calibrated value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum CalibrationSource {
    /// Set in ratchet.yaml.
    Config,
    /// Derived from the baseline.
    Auto,
    /// The built-in default (noise factor only).
    Default,
}

/// Input of [`calibrate`].
#[derive(Debug, Clone, Default)]
pub struct CalibrationInput<'a> {
    /// The baseline's bench samples (the objective metric of each run).
    pub samples: &'a [f64],
    /// `evaluator.repeats` from ratchet.yaml, when set.
    pub repeats: Option<u64>,
    /// `objective.min_gain` from ratchet.yaml (a fraction), when set.
    pub min_gain: Option<f64>,
    /// `evaluator.noise_factor` from ratchet.yaml, when set.
    pub noise_factor: Option<f64>,
}

/// The calibration record (stored as JSON in `engine_kv` under `opt:<id>:calibration`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    /// Schema version of this record (it is stored as JSON).
    pub version: u8,
    /// Number of baseline runs the noise was measured on.
    pub baseline_runs: u64,
    /// Coefficient of variation of the baseline (fraction); `None` when it cannot be estimated (fewer than
    /// 3 runs, or a zero mean).
    pub cv: Option<f64>,
    /// `2 x CV` as a fraction: "this metric moves by about +/- noise from run to run". `None` with `cv`.
    pub noise: Option<f64>,
    /// Runs per candidate / confirm run.
    pub repeats: u64,
    pub repeats_source: CalibrationSource,
    /// True when the derivation wanted more than the cap allows.
    pub repeats_capped: bool,
    /// The win threshold as a fraction of the current best.
    pub min_gain: f64,
    pub min_gain_source: CalibrationSource,
    /// The gain the chosen repeats can resolve with confidence (a fraction); `min_gain` is never below it
    /// when auto.
    pub resolvable: Option<f64>,
    /// The k of the noise floor `k x stddev(baseline)`.
    pub noise_factor: f64,
    pub noise_factor_source: CalibrationSource,
    /// Plain words for the UI, e.g. "this metric is noisy (±9%); wins must beat 9%; each candidate is
    /// measured 7 times".
    pub explanation: String,
}

/// Round a fraction up to a readable step: 0.5% below 2%, 1% below 20%, 5% above (never above 100%).
pub fn nice_gain(x: f64) -> f64 {
    if !x.is_finite() || x <= 0.0 {
        return cal::FLOOR_GAIN;
    }
    let step = if x <= 0.02 {
        0.005
    } else if x <= 0.2 {
        0.01
    } else {
        0.05
    };
    let v = (x / step - 1e-9).ceil() * step;
    // `Number(v.toFixed(6))`
    let rounded: f64 = to_fixed(v, 6).parse().unwrap_or(v);
    rounded.min(1.0)
}

/// "9%", "0.9%", "1.5%": whole percent from 5% up, one decimal below, trailing ".0" dropped.
pub fn format_percent(fraction: f64) -> String {
    let p = fraction * 100.0;
    if p >= 5.0 {
        return format!("{}%", p.round());
    }
    format!("{}%", fixed_trimmed(p, 1))
}

/// How many runs of n it takes for the median-vs-median difference to stand z standard errors above `gain`.
/// Returns a float so that a huge answer is capped by the caller instead of overflowing an integer.
fn repeats_for(cv: f64, gain: f64) -> f64 {
    let z = cal::Z;
    let n = ((2.0 * z * z * cv * cv) / (gain * gain) - 1e-9).ceil();
    n.max(cal::MIN_REPEATS as f64)
}

/// The gain n runs per side resolve at z standard errors.
fn resolvable_gain(cv: f64, n: u64) -> f64 {
    (cal::Z * std::f64::consts::SQRT_2 * cv) / (n as f64).sqrt()
}

fn noise_word(noise: f64) -> &'static str {
    if noise < 0.01 {
        "stable"
    } else if noise < 0.04 {
        "slightly noisy"
    } else if noise < 0.1 {
        "noisy"
    } else {
        "very noisy"
    }
}

/// Derive repeats, win threshold and noise factor from the baseline runs. Pure. Explicit settings win; the
/// rest is derived. With fewer than 3 baseline runs (or a zero mean) there is nothing to measure noise on: the
/// old default of 3% is used for the threshold and the explanation says so.
///
/// Fails only when a sample is not a finite number.
pub fn calibrate(input: &CalibrationInput<'_>) -> Result<Calibration, StatsError> {
    let samples = input.samples;
    let baseline_runs = samples.len() as u64;
    let noise_factor = input.noise_factor.unwrap_or(cal::NOISE_FACTOR);
    let noise_factor_source = if input.noise_factor.is_none() {
        CalibrationSource::Default
    } else {
        CalibrationSource::Config
    };
    let repeats_source = if input.repeats.is_none() {
        CalibrationSource::Auto
    } else {
        CalibrationSource::Config
    };
    let min_gain_source = if input.min_gain.is_none() {
        CalibrationSource::Auto
    } else {
        CalibrationSource::Config
    };

    let sum = if baseline_runs > 0 {
        Some(summarize(samples)?)
    } else {
        None
    };
    let cv = match &sum {
        Some(s) if baseline_runs >= cal::MIN_REPEATS && s.mean != 0.0 && s.cv.is_finite() => {
            Some(s.cv)
        }
        _ => None,
    };

    let repeats: u64;
    let mut capped = false;
    let min_gain: f64;
    let mut resolvable: Option<f64> = None;

    match cv {
        None => {
            repeats = input.repeats.unwrap_or(cal::MIN_REPEATS + 2); // 5: the old default
            min_gain = input.min_gain.unwrap_or(cal::FALLBACK_GAIN);
        }
        Some(cv) => {
            let target = input
                .min_gain
                .unwrap_or_else(|| (cal::NOISE_SIGMAS * cv).max(cal::FLOOR_GAIN));
            repeats = match input.repeats {
                None => {
                    let wanted = repeats_for(cv, target.max(1e-9));
                    capped = wanted > cal::MAX_REPEATS as f64;
                    wanted.min(cal::MAX_REPEATS as f64) as u64
                }
                Some(r) => r,
            };
            let res = resolvable_gain(cv, repeats);
            resolvable = Some(res);
            min_gain = input.min_gain.unwrap_or_else(|| nice_gain(target.max(res)));
        }
    }

    let noise = cv.map(|cv| cal::NOISE_SIGMAS * cv);
    let mut result = Calibration {
        version: 1,
        baseline_runs,
        cv,
        noise,
        repeats,
        repeats_source,
        repeats_capped: capped,
        min_gain,
        min_gain_source,
        resolvable,
        noise_factor,
        noise_factor_source,
        explanation: String::new(),
    };
    result.explanation = explain_calibration(&result);
    Ok(result)
}

/// The one-line, plain-words summary the UI shows next to the metric.
pub fn explain_calibration(c: &Calibration) -> String {
    let mut parts: Vec<String> = Vec::new();
    let set = |s: CalibrationSource| {
        if s == CalibrationSource::Config {
            " (set in ratchet.yaml)"
        } else {
            ""
        }
    };
    match c.noise {
        None => {
            parts.push(format!(
                "the noise of this metric could not be measured ({} baseline run{}, at least {} needed)",
                c.baseline_runs,
                if c.baseline_runs == 1 { "" } else { "s" },
                cal::MIN_REPEATS
            ));
            parts.push(format!(
                "wins must beat {}{}",
                format_percent(c.min_gain),
                set(c.min_gain_source)
            ));
        }
        Some(noise) => {
            parts.push(format!(
                "this metric is {} (±{})",
                noise_word(noise),
                format_percent(noise)
            ));
            parts.push(format!(
                "wins must beat {}{}",
                format_percent(c.min_gain),
                set(c.min_gain_source)
            ));
        }
    }
    parts.push(format!(
        "each candidate is measured {} times{}",
        c.repeats,
        set(c.repeats_source)
    ));
    if let (true, Some(res)) = (c.repeats_capped, c.resolvable) {
        parts.push(format!(
            "{} runs is the cap, so gains under {} cannot be told from noise",
            cal::MAX_REPEATS,
            format_percent(res)
        ));
    } else if c.min_gain_source == CalibrationSource::Config
        && let Some(res) = c.resolvable
        && res > c.min_gain + 1e-9
    {
        parts.push(format!(
            "with {} runs gains under {} may still be noise",
            c.repeats,
            format_percent(res)
        ));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` samples around 100 with the given relative standard deviation (deterministic, exact sample stddev).
    fn samples_with_cv(cv: f64, n: usize) -> Vec<f64> {
        // alternate +-1 around the mean: scale so the CV is exact
        let mut raw: Vec<f64> = (0..n)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        if n % 2 == 1 {
            raw[n - 1] = 0.0;
        }
        let mean = raw.iter().sum::<f64>() / n as f64;
        let sd = (raw.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt();
        raw.into_iter()
            .map(|r| 100.0 + (r / sd) * 100.0 * cv)
            .collect()
    }

    fn cal_of(samples: &[f64]) -> Calibration {
        calibrate(&CalibrationInput {
            samples,
            ..Default::default()
        })
        .unwrap()
    }

    fn close(a: f64, b: f64, eps: f64) {
        assert!((a - b).abs() < eps, "{a} != {b}");
    }

    // calibrate.test.ts "niceGain / formatPercent": rounds up to readable steps and never below the floor
    #[test]
    fn nice_gain_rounds_up_to_readable_steps() {
        assert_eq!(nice_gain(0.01), 0.01);
        assert_eq!(nice_gain(0.0101), 0.015);
        assert_eq!(nice_gain(0.0862), 0.09);
        assert_eq!(nice_gain(0.2), 0.2);
        assert_eq!(nice_gain(0.21), 0.25);
        assert_eq!(nice_gain(3.0), 1.0);
        assert_eq!(nice_gain(0.0), cal::FLOOR_GAIN);
        assert_eq!(nice_gain(f64::NAN), cal::FLOOR_GAIN);
    }

    // "formats fractions as short percentages"
    #[test]
    fn formats_fractions_as_short_percentages() {
        assert_eq!(format_percent(0.09), "9%");
        assert_eq!(format_percent(0.0086), "0.9%");
        assert_eq!(format_percent(0.015), "1.5%");
        assert_eq!(format_percent(0.123), "12%");
        assert_eq!(format_percent(0.01), "1%");
    }

    // "a stable metric needs the minimum repeats and a 1% threshold"
    #[test]
    fn a_stable_metric_needs_the_minimum_repeats_and_a_one_percent_threshold() {
        let c = cal_of(&samples_with_cv(0.001, 5));
        close(c.cv.unwrap(), 0.001, 1e-6);
        assert_eq!(c.repeats, cal::MIN_REPEATS);
        assert_eq!(c.min_gain, 0.01);
        assert_eq!(c.repeats_source, CalibrationSource::Auto);
        assert_eq!(c.min_gain_source, CalibrationSource::Auto);
        assert!(!c.repeats_capped);
        assert_eq!(
            c.explanation,
            "this metric is stable (±0.2%); wins must beat 1%; each candidate is measured 3 times"
        );
    }

    // "a noisy metric raises the threshold to max(2 x CV, 1%) and the repeats to resolve it"
    #[test]
    fn a_noisy_metric_raises_the_threshold_and_the_repeats() {
        let c = cal_of(&samples_with_cv(0.043, 5));
        // 2 x CV = 8.6% -> 9%; n = ceil(18 cv^2 / g^2) with g = 8.6% -> 5 runs
        close(c.noise.unwrap(), 0.086, 1e-6);
        assert_eq!(c.min_gain, 0.09);
        assert_eq!(c.repeats, 5);
        assert_eq!(
            c.explanation,
            "this metric is noisy (±9%); wins must beat 9%; each candidate is measured 5 times"
        );
    }

    // "threshold is never below what the chosen repeats can resolve"
    #[test]
    fn threshold_is_never_below_what_the_chosen_repeats_can_resolve() {
        for cv in [0.002, 0.006, 0.01, 0.03, 0.08, 0.2] {
            let c = cal_of(&samples_with_cv(cv, 5));
            assert!(c.min_gain >= (2.0 * cv).max(0.01) - 1e-9);
            assert!(c.min_gain >= c.resolvable.unwrap() - 1e-9);
            assert!(c.repeats >= cal::MIN_REPEATS);
            assert!(c.repeats <= cal::MAX_REPEATS);
        }
    }

    // "is monotone: a noisier baseline never gets a lower threshold"
    #[test]
    fn is_monotone() {
        let mut prev = 0.0;
        for cv in [0.001, 0.004, 0.01, 0.02, 0.05, 0.1, 0.3] {
            let g = cal_of(&samples_with_cv(cv, 5)).min_gain;
            assert!(g >= prev);
            prev = g;
        }
    }

    // "a constant baseline (a deterministic metric) has CV 0"
    #[test]
    fn a_constant_baseline_has_cv_zero() {
        let c = cal_of(&[43.5, 43.5, 43.5]);
        assert_eq!(c.cv, Some(0.0));
        assert_eq!(c.min_gain, 0.01);
        assert_eq!(c.repeats, 3);
    }

    // "repeats set: used as-is, threshold covers what they can resolve"
    #[test]
    fn repeats_set_is_used_as_is() {
        let s = samples_with_cv(0.043, 5);
        let c = calibrate(&CalibrationInput {
            samples: &s,
            repeats: Some(3),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.repeats, 3);
        assert_eq!(c.repeats_source, CalibrationSource::Config);
        // z sqrt(2) cv / sqrt(3) = 10.5% > 2 x CV
        close(
            c.resolvable.unwrap(),
            (3.0 * std::f64::consts::SQRT_2 * 0.043) / 3f64.sqrt(),
            1e-6,
        );
        assert_eq!(c.min_gain, 0.11);
    }

    // "min_gain set: used as-is, repeats are derived to resolve it, capped"
    #[test]
    fn min_gain_set_is_used_as_is_and_repeats_are_capped() {
        let s = samples_with_cv(0.05, 5);
        let c = calibrate(&CalibrationInput {
            samples: &s,
            min_gain: Some(0.03),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.min_gain, 0.03);
        assert_eq!(c.min_gain_source, CalibrationSource::Config);
        assert_eq!(c.repeats, cal::MAX_REPEATS); // 18 x 0.0025 / 0.0009 = 50 -> cap
        assert!(c.repeats_capped);
        assert!(c.explanation.contains("(set in ratchet.yaml)"));
        assert!(c.explanation.contains("15 runs is the cap, so gains under"));
    }

    // "min_gain set and loose: fewer repeats suffice"
    #[test]
    fn min_gain_set_and_loose_needs_fewer_repeats() {
        let s = samples_with_cv(0.01, 5);
        let c = calibrate(&CalibrationInput {
            samples: &s,
            min_gain: Some(0.1),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.repeats, 3);
        assert!(!c.repeats_capped);
    }

    // "both set: nothing is derived, but a too-optimistic pair is called out"
    #[test]
    fn both_set_calls_out_a_too_optimistic_pair() {
        let s = samples_with_cv(0.05, 5);
        let c = calibrate(&CalibrationInput {
            samples: &s,
            min_gain: Some(0.03),
            repeats: Some(5),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.repeats, 5);
        assert_eq!(c.min_gain, 0.03);
        let re = c
            .explanation
            .split("with 5 runs gains under ")
            .nth(1)
            .expect("warning present");
        assert!(re.ends_with("% may still be noise"), "{}", c.explanation);
    }

    // "noise_factor is carried through"
    #[test]
    fn noise_factor_is_carried_through() {
        let s = [1.0, 1.1, 0.9];
        let c = calibrate(&CalibrationInput {
            samples: &s,
            noise_factor: Some(3.0),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            (c.noise_factor, c.noise_factor_source),
            (3.0, CalibrationSource::Config)
        );
        let c = cal_of(&s);
        assert_eq!(
            (c.noise_factor, c.noise_factor_source),
            (2.0, CalibrationSource::Default)
        );
    }

    // "fewer than 3 runs cannot measure noise: falls back to 3% and says so"
    #[test]
    fn fewer_than_three_runs_falls_back_to_three_percent() {
        let c = cal_of(&[10.0, 11.0]);
        assert_eq!(c.cv, None);
        assert_eq!(c.noise, None);
        assert_eq!(c.min_gain, cal::FALLBACK_GAIN);
        assert!(c.explanation.contains("could not be measured"));
        assert!(c.explanation.contains("wins must beat 3%"));
    }

    // "a zero mean (CV undefined) falls back the same way"
    #[test]
    fn a_zero_mean_falls_back_the_same_way() {
        let c = cal_of(&[-1.0, 0.0, 1.0]);
        assert_eq!(c.cv, None);
        assert_eq!(c.min_gain, cal::FALLBACK_GAIN);
    }

    // "a single run with repeats set keeps them"
    #[test]
    fn a_single_run_with_repeats_set_keeps_them() {
        let s = [5.0];
        let c = calibrate(&CalibrationInput {
            samples: &s,
            repeats: Some(7),
            min_gain: Some(0.02),
            ..Default::default()
        })
        .unwrap();
        assert_eq!((c.repeats, c.min_gain, c.cv), (7, 0.02, None));
    }

    // "a maximize-style metric near zero stays finite and capped"
    #[test]
    fn a_metric_near_zero_stays_finite_and_capped() {
        let c = cal_of(&[0.01, 0.03, 0.02, 0.05, 0.005]);
        assert!(c.min_gain.is_finite());
        assert!(c.min_gain <= 1.0);
        assert!(c.repeats <= cal::MAX_REPEATS);
    }

    // "names the noise level in words"
    #[test]
    fn names_the_noise_level_in_words() {
        assert!(
            cal_of(&samples_with_cv(0.015, 5))
                .explanation
                .contains("slightly noisy (±3%)")
        );
        assert!(
            cal_of(&samples_with_cv(0.2, 5))
                .explanation
                .contains("very noisy (±40%)")
        );
    }

    // "is stable under a round trip through JSON (it is stored as JSON)"
    #[test]
    fn explanation_is_stable_under_a_json_round_trip() {
        let c = cal_of(&samples_with_cv(0.043, 5));
        let back: Calibration = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(explain_calibration(&back), c.explanation);
        assert_eq!(back, c);
    }

    #[test]
    fn json_shape_matches_the_typescript_record() {
        let c = cal_of(&[10.0, 11.0]);
        let v = serde_json::to_value(&c).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "version",
                "baselineRuns",
                "cv",
                "noise",
                "repeats",
                "repeatsSource",
                "repeatsCapped",
                "minGain",
                "minGainSource",
                "resolvable",
                "noiseFactor",
                "noiseFactorSource",
                "explanation"
            ]
        );
        assert_eq!(v["repeatsSource"], "auto");
        assert_eq!(v["noiseFactorSource"], "default");
        assert!(v["cv"].is_null());
    }

    #[test]
    fn non_finite_samples_are_an_error() {
        assert!(
            calibrate(&CalibrationInput {
                samples: &[1.0, f64::NAN, 3.0],
                ..Default::default()
            })
            .is_err()
        );
    }
}
