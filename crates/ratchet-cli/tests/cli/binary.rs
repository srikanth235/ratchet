//! The real `ratchet` binary: exit codes, the default config path, and the socket / token environment variables.

use std::process::Stdio;

use crate::common::{Env, RepoOpts, make_repo};

fn bin() -> tokio::process::Command {
    let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_ratchet"));
    c.env_remove("RATCHET_SOCKET")
        .env_remove("RATCHET_TOKEN")
        .env_remove("RATCHET_TOKEN_FILE")
        .env_remove("RATCHET_HOME")
        .stdin(Stdio::null());
    c
}

#[tokio::test]
async fn validate_defaults_to_ratchet_yaml_in_the_current_directory() {
    let repo = make_repo(RepoOpts::new(&[1.0]));
    let out = bin()
        .arg("validate")
        .current_dir(repo.path())
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("ratchet.yaml: OK"));
    let empty = tempfile::tempdir().unwrap();
    let out = bin()
        .arg("validate")
        .current_dir(empty.path())
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot find ratchet.yaml"));
}

#[tokio::test]
async fn exit_codes_are_0_1_and_2() {
    let out = bin().arg("--version").output().await.unwrap();
    assert_eq!(out.status.code(), Some(0));
    let out = bin().arg("frobnicate").output().await.unwrap();
    assert_eq!(out.status.code(), Some(2), "usage errors are 2");
    let out = bin()
        .args(["baseline", "--repeats", "abc"])
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = bin()
        .args(["validate", "/nonexistent.yaml"])
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // no daemon at all
    let dir = tempfile::tempdir().unwrap();
    let out = bin()
        .env("RATCHET_HOME", dir.path())
        .arg("status")
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("ratchet: "));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_and_the_token_come_from_the_environment() {
    let env = Env::start().await;
    // RATCHET_SOCKET, with the token file found next to the socket
    let out = bin()
        .env("RATCHET_SOCKET", env.socket())
        .args(["status", "--json"])
        .output()
        .await
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["daemon"]["apiVersion"], "v1");
    // RATCHET_HOME is where the daemon keeps ratchetd.sock and ratchetd.token
    let out = bin()
        .env("RATCHET_HOME", env.dir.path())
        .args(["list", "--json"])
        .output()
        .await
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["optimizations"].as_array().unwrap().len(), 4);
    // RATCHET_TOKEN overrides the file
    let out = bin()
        .env("RATCHET_SOCKET", env.socket())
        .env("RATCHET_TOKEN", "not-it")
        .arg("status")
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("wrong bearer token"));
    env.stop().await;
}
