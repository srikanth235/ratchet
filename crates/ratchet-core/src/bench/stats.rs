//! Summary statistics for repeated benchmark runs. All functions are pure.
//!
//! Port of `packages/core/src/bench/stats.ts`. Where the TypeScript threw `RangeError` these return
//! [`StatsError`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Invalid input to a statistics function (a caller bug, not a user-input problem).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StatsError {
    #[error("at least one sample is required")]
    Empty,
    #[error("samples must be finite numbers, got {0}")]
    NonFinite(f64),
    #[error("{0}")]
    InvalidOption(String),
}

/// Summary statistics of a sample set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Summary {
    pub n: usize,
    pub mean: f64,
    pub median: f64,
    /// Sample standard deviation (n - 1 denominator); 0 when n === 1.
    pub stddev: f64,
    /// Coefficient of variation: stddev / |mean|. 0 when stddev is 0; infinity when the mean is 0 but the
    /// stddev is not (not representable in JSON: serialized as `null`).
    pub cv: f64,
    pub min: f64,
    pub max: f64,
}

fn assert_samples(samples: &[f64]) -> Result<(), StatsError> {
    if samples.is_empty() {
        return Err(StatsError::Empty);
    }
    if let Some(s) = samples.iter().find(|s| !s.is_finite()) {
        return Err(StatsError::NonFinite(*s));
    }
    Ok(())
}

/// Arithmetic mean.
pub fn mean(samples: &[f64]) -> Result<f64, StatsError> {
    assert_samples(samples)?;
    Ok(mean_unchecked(samples))
}

fn mean_unchecked(samples: &[f64]) -> f64 {
    let mut sum = 0.0;
    for s in samples {
        sum += s;
    }
    sum / samples.len() as f64
}

/// Median (mean of the two middle values for an even count). Does not modify the input.
pub fn median(samples: &[f64]) -> Result<f64, StatsError> {
    assert_samples(samples)?;
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() >> 1;
    Ok(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    })
}

/// Sample standard deviation (Bessel-corrected). Returns 0 for a single sample.
pub fn stddev(samples: &[f64]) -> Result<f64, StatsError> {
    assert_samples(samples)?;
    if samples.len() == 1 {
        return Ok(0.0);
    }
    let m = mean_unchecked(samples);
    let mut ss = 0.0;
    for s in samples {
        let d = s - m;
        ss += d * d;
    }
    Ok((ss / (samples.len() - 1) as f64).sqrt())
}

/// `stddev / |mean|`; 0 when the stddev is 0, infinity when the mean is 0 but the stddev is not.
pub fn coefficient_of_variation(samples: &[f64]) -> Result<f64, StatsError> {
    let sd = stddev(samples)?;
    if sd == 0.0 {
        return Ok(0.0);
    }
    let m = mean_unchecked(samples).abs();
    Ok(if m == 0.0 { f64::INFINITY } else { sd / m })
}

/// All of the above in one pass over the validated input.
pub fn summarize(samples: &[f64]) -> Result<Summary, StatsError> {
    assert_samples(samples)?;
    Ok(Summary {
        n: samples.len(),
        mean: mean(samples)?,
        median: median(samples)?,
        stddev: stddev(samples)?,
        cv: coefficient_of_variation(samples)?,
        min: samples.iter().copied().fold(f64::INFINITY, f64::min),
        max: samples.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

/// Options of [`noise_floor`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NoiseFloorOptions {
    /// Multiplier on the baseline standard deviation. Default 2.
    pub k: Option<f64>,
    /// Absolute lower bound for the floor, in metric units. Default 0.
    pub min_absolute: Option<f64>,
}

/// Noise floor derived from baseline runs, in the metric's own units:
///
/// `noise_floor = max(k * stddev(baseline), min_absolute)` (`k` defaults to 2)
///
/// A candidate's improvement (`|best - candidate|`) must exceed this to be distinguishable from run-to-run
/// variance. With one baseline run the stddev is 0, so the floor degenerates to `min_absolute`; use
/// repeats >= 3.
pub fn noise_floor(baseline_samples: &[f64], opts: NoiseFloorOptions) -> Result<f64, StatsError> {
    let k = opts.k.unwrap_or(2.0);
    let min_absolute = opts.min_absolute.unwrap_or(0.0);
    if !k.is_finite() || k < 0.0 {
        return Err(StatsError::InvalidOption(
            "k must be a non-negative finite number".into(),
        ));
    }
    if !min_absolute.is_finite() || min_absolute < 0.0 {
        return Err(StatsError::InvalidOption(
            "minAbsolute must be a non-negative finite number".into(),
        ));
    }
    Ok((k * stddev(baseline_samples)?).max(min_absolute))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} != {b}");
    }

    // stats.test.ts: "mean"
    #[test]
    fn mean_basic() {
        assert_eq!(mean(&[1.0, 2.0, 3.0, 4.0]).unwrap(), 2.5);
        assert_eq!(mean(&[7.0]).unwrap(), 7.0);
    }

    // stats.test.ts: "median, odd/even/unsorted, without mutating input"
    #[test]
    fn median_odd_even_unsorted_without_mutating() {
        let xs = [5.0, 1.0, 3.0];
        assert_eq!(median(&xs).unwrap(), 3.0);
        assert_eq!(xs, [5.0, 1.0, 3.0]);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]).unwrap(), 2.5);
        assert_eq!(median(&[9.0]).unwrap(), 9.0);
    }

    // stats.test.ts: "stddev is the sample (n-1) deviation"
    #[test]
    fn stddev_is_the_sample_deviation() {
        // classic example: mean 5, sum of squares 32 => sqrt(32/7)
        close(
            stddev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap(),
            (32.0f64 / 7.0).sqrt(),
        );
        assert_eq!(stddev(&[3.0, 3.0, 3.0]).unwrap(), 0.0);
        assert_eq!(stddev(&[42.0]).unwrap(), 0.0);
    }

    // stats.test.ts: "coefficient of variation"
    #[test]
    fn coefficient_of_variation_cases() {
        assert_eq!(coefficient_of_variation(&[10.0, 10.0, 10.0]).unwrap(), 0.0);
        close(
            coefficient_of_variation(&[9.0, 11.0]).unwrap(),
            std::f64::consts::SQRT_2 / 10.0,
        );
        assert_eq!(
            coefficient_of_variation(&[-1.0, 1.0]).unwrap(),
            f64::INFINITY
        ); // mean 0, stddev > 0
        assert_eq!(coefficient_of_variation(&[0.0, 0.0]).unwrap(), 0.0);
        assert!(coefficient_of_variation(&[-9.0, -11.0]).unwrap() > 0.0); // uses |mean|
    }

    // stats.test.ts: "summarize"
    #[test]
    fn summarize_basic() {
        let s = summarize(&[10.0, 12.0, 11.0, 13.0, 9.0]).unwrap();
        assert_eq!(
            (s.n, s.mean, s.median, s.min, s.max),
            (5, 11.0, 11.0, 9.0, 13.0)
        );
        close(s.stddev, 2.5f64.sqrt());
        close(s.cv, 2.5f64.sqrt() / 11.0);
    }

    // stats.test.ts: "rejects empty and non-finite samples"
    #[test]
    fn rejects_empty_and_non_finite_samples() {
        assert_eq!(mean(&[]), Err(StatsError::Empty));
        assert_eq!(median(&[]), Err(StatsError::Empty));
        assert_eq!(summarize(&[]), Err(StatsError::Empty));
        assert!(matches!(
            stddev(&[1.0, f64::NAN]),
            Err(StatsError::NonFinite(_))
        ));
        assert!(matches!(
            mean(&[1.0, f64::INFINITY]),
            Err(StatsError::NonFinite(_))
        ));
    }

    // stats.test.ts (noiseFloor): "is 2 x stddev by default"
    #[test]
    fn noise_floor_is_two_stddev_by_default() {
        let xs = [100.0, 102.0, 98.0, 101.0, 99.0];
        close(
            noise_floor(&xs, NoiseFloorOptions::default()).unwrap(),
            2.0 * stddev(&xs).unwrap(),
        );
    }

    // stats.test.ts (noiseFloor): "honours k and the absolute minimum"
    #[test]
    fn noise_floor_honours_k_and_the_absolute_minimum() {
        let xs = [100.0, 102.0, 98.0, 101.0, 99.0];
        close(
            noise_floor(
                &xs,
                NoiseFloorOptions {
                    k: Some(3.0),
                    ..Default::default()
                },
            )
            .unwrap(),
            3.0 * stddev(&xs).unwrap(),
        );
        assert_eq!(
            noise_floor(
                &xs,
                NoiseFloorOptions {
                    min_absolute: Some(50.0),
                    ..Default::default()
                }
            )
            .unwrap(),
            50.0
        );
        assert_eq!(
            noise_floor(
                &[5.0, 5.0, 5.0],
                NoiseFloorOptions {
                    min_absolute: Some(0.25),
                    ..Default::default()
                }
            )
            .unwrap(),
            0.25
        );
    }

    // stats.test.ts (noiseFloor): "is 0 for perfectly stable runs and a single run"
    #[test]
    fn noise_floor_is_zero_for_stable_runs_and_a_single_run() {
        assert_eq!(
            noise_floor(&[5.0, 5.0, 5.0], NoiseFloorOptions::default()).unwrap(),
            0.0
        );
        assert_eq!(
            noise_floor(&[5.0], NoiseFloorOptions::default()).unwrap(),
            0.0
        );
    }

    // stats.test.ts (noiseFloor): "validates options"
    #[test]
    fn noise_floor_validates_options() {
        assert!(matches!(
            noise_floor(
                &[1.0, 2.0],
                NoiseFloorOptions {
                    k: Some(-1.0),
                    ..Default::default()
                }
            ),
            Err(StatsError::InvalidOption(_))
        ));
        assert!(matches!(
            noise_floor(
                &[1.0, 2.0],
                NoiseFloorOptions {
                    min_absolute: Some(-1.0),
                    ..Default::default()
                }
            ),
            Err(StatsError::InvalidOption(_))
        ));
        assert_eq!(
            noise_floor(&[], NoiseFloorOptions::default()),
            Err(StatsError::Empty)
        );
    }
}
