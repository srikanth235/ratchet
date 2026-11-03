//! Statistics, auto-calibration and benchmark-output parsing.

pub mod calibrate;
pub mod output;
pub mod stats;

pub use calibrate::{
    Calibration, CalibrationInput, CalibrationSource, calibrate as calibrate_baseline,
    explain_calibration, format_percent, nice_gain,
};
pub use output::{MetricError, extract_last_json_object, extract_metric, lookup_path};
pub use stats::{
    NoiseFloorOptions, StatsError, Summary, coefficient_of_variation, mean, median, noise_floor,
    stddev, summarize,
};
