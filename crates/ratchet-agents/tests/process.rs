mod common;

use std::collections::HashMap;
use std::time::Duration;

use ratchet_agents::process::{RunProcessOptions, TokioSpawner, run_process};
use tokio_util::sync::CancellationToken;

fn env() -> HashMap<String, String> {
    common::env_of(&[("PATH", &common::path_env())])
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn streams_stdout_lines_feeds_stdin_and_captures_the_stderr_tail() {
    let mut lines: Vec<String> = Vec::new();
    let mut collect = |l: &str| lines.push(l.to_string());
    let mut options = RunProcessOptions::new(std::env::temp_dir(), env());
    options.stdin = Some("hello\n".to_string());
    options.on_stdout_line = Some(&mut collect);
    let res = run_process(
        &TokioSpawner,
        "sh",
        &args(&["-c", "read x; echo \"got $x\"; echo oops >&2; exit 3"]),
        options,
    )
    .await;
    assert_eq!(lines, vec!["got hello"]);
    assert_eq!(res.code(), Some(3));
    assert!(!res.aborted);
    assert_eq!(res.stderr_tail, "oops");
}

#[tokio::test]
async fn reports_a_missing_binary_as_a_spawn_error() {
    let res = run_process(
        &TokioSpawner,
        "definitely-not-a-binary",
        &[],
        RunProcessOptions::new(std::env::temp_dir(), env()),
    )
    .await;
    assert!(
        res.spawn_error().is_some_and(|e| e.contains("ENOENT")),
        "{res:?}"
    );
    assert_eq!(res.code(), None);
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test]
async fn abort_kills_the_whole_process_group_including_grandchildren_that_ignore_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    // the shell starts a grandchild that ignores SIGTERM, records its pid, then both sleep
    let script = format!(
        "(trap '' TERM; echo $$ > {}; sleep 60) & sleep 60",
        pid_file.display()
    );
    let cancel = CancellationToken::new();
    let task = {
        let (cancel, cwd) = (cancel.clone(), dir.path().to_path_buf());
        tokio::spawn(async move {
            let mut options = RunProcessOptions::new(cwd, env());
            options.cancel = Some(&cancel);
            options.kill_grace = Duration::from_millis(200);
            run_process(&TokioSpawner, "sh", &["-c".to_string(), script], options).await
        })
    };
    for _ in 0..100 {
        if pid_file.exists()
            && !std::fs::read_to_string(&pid_file)
                .unwrap_or_default()
                .trim()
                .is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let grandchild: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(grandchild));
    cancel.cancel();
    let res = task.await.unwrap();
    assert!(res.aborted);
    // SIGKILL after the grace period reaps the SIGTERM-ignoring grandchild too
    let mut still_alive = true;
    for _ in 0..50 {
        if !alive(grandchild) {
            still_alive = false;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!still_alive);
}

#[tokio::test]
async fn multibyte_output_split_across_reads_is_not_corrupted() {
    // 3000 "é" (2 bytes each) printed without newlines: the pipe delivers it in several chunks
    let mut seen = String::new();
    let mut collect = |l: &str| seen.push_str(l);
    let mut options = RunProcessOptions::new(std::env::temp_dir(), env());
    options.on_stdout_line = Some(&mut collect);
    let script = "i=0; while [ $i -lt 3000 ]; do printf 'é'; i=$((i+1)); done; echo";
    run_process(&TokioSpawner, "sh", &args(&["-c", script]), options).await;
    assert_eq!(seen, "é".repeat(3000));
}
