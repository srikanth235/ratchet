//! `ratchet baseline [path] -n N` (offline). Port of `packages/cli/src/commands/baseline.ts`.
//!
//! Runs `evaluator.run` N times sequentially, in the directory containing the config file (the repo root).
//! Output contract: the command prints JSON on stdout; the LAST top-level JSON object on stdout is used and any
//! other output is ignored. `objective.metric` is looked up in it by dotted path, e.g. `http_req_duration.p95` ->
//! `{"http_req_duration": {"p95": 12.3}}`.
//!
//! Prints per-run values, summary stats and the noise floor (`max(k * stddev, min_absolute)`, k = 2) or, with
//! `--json`, one JSON document. Exit code 0 on success, 1 on any failure (invalid config, a run exiting non-zero,
//! missing metric, ...).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use ratchet_core::bench::calibrate::calibration::BASELINE_REPEATS;
use ratchet_core::config::{format_config_error, load_config};
use ratchet_core::numfmt::precision_trimmed;
use ratchet_core::{
    CalibrationInput, NoiseFloorOptions, calibrate_baseline, extract_metric, noise_floor, summarize,
};
use serde_json::json;
use tokio::process::Command;

use super::emit_json;
use crate::cli::BaselineArgs;
use crate::io::Io;

pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Run a shell command in `cwd`, capturing stdout/stderr. A timeout kills the shell AND what it started.
pub async fn run_shell(
    command: &str,
    cwd: &Path,
    timeout: Option<Duration>,
) -> std::io::Result<CommandResult> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // own process group, so a timeout can kill the shell AND the bench it spawned
        .process_group(0);
    let child = cmd.spawn()?;
    let pid = child.id();
    let wait = child.wait_with_output();
    let out = match timeout {
        None => wait.await?,
        Some(t) => match tokio::time::timeout(t, wait).await {
            Ok(r) => r?,
            Err(_) => {
                if let Some(pid) = pid {
                    // SAFETY: killpg only signals the process group we created above.
                    unsafe {
                        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
                return Ok(CommandResult {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: true,
                });
            }
        },
    };
    Ok(CommandResult {
        exit_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        timed_out: false,
    })
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.trim_end().split('\n').collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn fmt(n: f64) -> String {
    precision_trimmed(n, 6)
}

pub async fn run_baseline(args: &BaselineArgs, json: bool, io: &dyn Io) -> i32 {
    let file = &args.path;
    let config = match load_config(file) {
        Ok(c) => c,
        Err(errors) => {
            io.err(&format!(
                "{}: invalid ({} error{})\n",
                file.display(),
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            ));
            for e in &errors {
                io.err(&format!("  - {}\n", format_config_error(e)));
            }
            return 1;
        }
    };
    let repeats = args
        .repeats
        .or(config.evaluator.repeats)
        .unwrap_or(BASELINE_REPEATS);
    if repeats < 1 {
        io.err(&format!(
            "--repeats must be a positive integer (got {repeats})\n"
        ));
        return 1;
    }
    let cwd = std::path::absolute(file)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| ".".into());
    let metric = config.objective.metric.clone();
    let run = config.evaluator.run.clone();
    let progress = |s: String| {
        if !json {
            io.out(&s);
        }
    };

    progress(format!(
        "Baseline: running `{run}` {repeats}x in {}\n",
        cwd.display()
    ));
    let mut samples = Vec::new();
    for i in 1..=repeats {
        let r = match run_shell(&run, &cwd, args.timeout.map(Duration::from_millis)).await {
            Ok(r) => r,
            Err(e) => {
                io.err(&format!(
                    "run {i}/{repeats}: could not start bench command: {e}\n"
                ));
                return 1;
            }
        };
        if r.timed_out {
            io.err(&format!(
                "run {i}/{repeats}: timed out after {}ms\n",
                args.timeout.unwrap_or(0)
            ));
            return 1;
        }
        if r.exit_code != Some(0) {
            io.err(&format!(
                "run {i}/{repeats}: bench command exited with status {}\n",
                r.exit_code.map_or("signal".to_owned(), |c| c.to_string())
            ));
            if !r.stderr.trim().is_empty() {
                io.err(&(tail(&r.stderr, 10) + "\n"));
            }
            return 1;
        }
        match extract_metric(&r.stdout, &metric) {
            Ok(v) => {
                samples.push(v);
                progress(format!("  run {i}/{repeats}: {metric} = {}\n", fmt(v)));
            }
            Err(e) => {
                io.err(&format!("run {i}/{repeats}: {e}\n"));
                return 1;
            }
        }
    }

    let k = args.k.or(config.evaluator.noise_factor).unwrap_or(2.0);
    let min_absolute = args.min_absolute.unwrap_or(0.0);
    let floor = match noise_floor(
        &samples,
        NoiseFloorOptions {
            k: Some(k),
            min_absolute: Some(min_absolute),
        },
    ) {
        Ok(f) => f,
        Err(e) => {
            io.err(&format!("{e}\n"));
            return 1;
        }
    };
    let Ok(summary) = summarize(&samples) else {
        io.err("cannot summarize the samples\n");
        return 1;
    };
    let calibration = match calibrate_baseline(&CalibrationInput {
        samples: &samples,
        repeats: config.evaluator.repeats,
        min_gain: config.objective.min_gain,
        noise_factor: Some(k),
    }) {
        Ok(c) => c,
        Err(e) => {
            io.err(&format!("{e}\n"));
            return 1;
        }
    };
    let min_gain = calibration.min_gain;
    let relative_gain = min_gain * summary.median.abs();
    let required_gain = floor.max(relative_gain);

    if json {
        emit_json(
            io,
            &json!({
                "metric": metric,
                "direction": config.objective.direction,
                "command": run,
                "repeats": repeats,
                "samples": samples,
                "summary": summary,
                "noiseFloor": floor,
                "noiseFloorFormula": {"k": k, "minAbsolute": min_absolute},
                "minGain": min_gain,
                "requiredGain": required_gain,
                "calibration": calibration,
            }),
        );
        return 0;
    }

    let mut lines = vec![
        String::new(),
        format!("  runs        {}", summary.n),
        format!("  mean        {}", fmt(summary.mean)),
        format!("  median      {}", fmt(summary.median)),
        format!("  stddev      {}", fmt(summary.stddev)),
        format!(
            "  cv          {}",
            if summary.cv.is_finite() {
                format!("{}%", ratchet_core::numfmt::to_fixed(summary.cv * 100.0, 2))
            } else {
                "inf".to_owned()
            }
        ),
        format!("  min / max   {} / {}", fmt(summary.min), fmt(summary.max)),
        format!(
            "  noise floor {}  (max({} x stddev, {}))",
            fmt(floor),
            ratchet_core::numfmt::js_number_to_string(k),
            ratchet_core::numfmt::js_number_to_string(min_absolute)
        ),
        format!(
            "  required gain {}  (max(noise floor, min_gain {} x median = {}))",
            fmt(required_gain),
            ratchet_core::numfmt::js_number_to_string(min_gain),
            fmt(relative_gain)
        ),
        format!("  calibration {}", calibration.explanation),
    ];
    if repeats < 3 {
        lines.push("  warning: fewer than 3 runs; the noise floor is unreliable".into());
    }
    io.out(&(lines.join("\n") + "\n"));
    0
}
