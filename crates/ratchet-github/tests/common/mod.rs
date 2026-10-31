//! Shared helpers for the integration tests: real git on a scratch bare remote built from `examples/demo-search`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use ratchet_github::{
    BareRemote, CreateBareRemoteOptions, LocalGitHost, LocalGitHostOptions, create_bare_remote,
};

pub fn demo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo-search")
}

/// `git -c commit.gpgsign=false <args>` in `cwd`, as an identified author; returns trimmed stdout.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Agent")
        .env("GIT_AUTHOR_EMAIL", "agent@example.com")
        .env("GIT_COMMITTER_NAME", "Agent")
        .env("GIT_COMMITTER_EMAIL", "agent@example.com")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Like [`git`] but returns stdout untrimmed, for byte-exact content checks.
pub fn git_raw(git_dir: &Path, args: &[&str]) -> String {
    let mut full = vec!["--git-dir", git_dir.to_str().unwrap()];
    full.extend_from_slice(args);
    let out = Command::new("git").args(&full).output().expect("run git");
    assert!(
        out.status.success(),
        "git {full:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn commit(work_dir: &Path, file: &str, content: &str, message: &str) -> String {
    std::fs::write(work_dir.join(file), content).unwrap();
    git(work_dir, &["add", "-A"]);
    git(work_dir, &["commit", "-q", "-m", message]);
    git(work_dir, &["rev-parse", "HEAD"])
}

pub struct Fixture {
    pub scratch: tempfile::TempDir,
    pub remote: BareRemote,
    pub host: LocalGitHost,
    /// c1, plain, mentions, no, c2, flipped
    pub shas: HashMap<&'static str, String>,
}

impl Fixture {
    pub fn path(&self, name: &str) -> PathBuf {
        self.scratch.path().join(name)
    }

    pub fn sha(&self, name: &str) -> &str {
        &self.shas[name]
    }

    pub fn remote_git(&self, args: &[&str]) -> String {
        git(&self.remote.remote_dir, args)
    }
}

/// The remote of `source` (a bare repo with `main`) without any candidate branches.
pub async fn bare_fixture_from(source: &Path) -> Fixture {
    let scratch = tempfile::tempdir().unwrap();
    let remote = create_bare_remote(
        source,
        CreateBareRemoteOptions {
            remote_dir: Some(scratch.path().join("remote.git")),
            work_dir: Some(scratch.path().join("work")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let host = LocalGitHost::new(
        LocalGitHostOptions::new(&remote.remote_dir).pr_dir(scratch.path().join("prs")),
    )
    .unwrap();
    Fixture {
        scratch,
        remote,
        host,
        shas: HashMap::new(),
    }
}

/// local.test.ts `beforeAll`: the demo repo plus branch `ratchet/o1/claude` with six commits.
pub async fn candidate_fixture() -> Fixture {
    let mut f = bare_fixture_from(&demo_dir()).await;
    let w = f.remote.work_dir.clone();
    git(&w, &["checkout", "-q", "-b", "ratchet/o1/claude"]);
    let mut shas = HashMap::new();
    shas.insert(
        "c1",
        commit(
            &w,
            "src/a.txt",
            "one\n",
            "Cache tokenizer\n\nWarm the cache.\n\nRatchet-Candidate: true\nRatchet-Agent: claude",
        ),
    );
    shas.insert(
        "plain",
        commit(&w, "notes.txt", "wip\n", "wip: not a candidate"),
    );
    shas.insert(
        "mentions",
        commit(
            &w,
            "src/m.txt",
            "m\n",
            "Mentions Ratchet-Candidate: true in prose\n\nThe trailer below is not real.\nRatchet-Candidate: true\n\nTrailing paragraph.",
        ),
    );
    shas.insert(
        "no",
        commit(
            &w,
            "src/n.txt",
            "n\n",
            "Explicitly not\n\nRatchet-Candidate: false",
        ),
    );
    shas.insert(
        "c2",
        commit(&w, "src/a.txt", "two\n", "Index postings\n\nSigned-off-by: A <a@example.com>\nratchet-candidate: true\nRatchet-Agent: claude"),
    );
    shas.insert(
        "flipped",
        commit(
            &w,
            "src/f.txt",
            "f\n",
            "Repeated trailer\n\nRatchet-Candidate: true\nRatchet-Candidate: false",
        ),
    );
    git(&w, &["push", "-q", "origin", "ratchet/o1/claude"]);
    f.shas = shas;
    f
}
